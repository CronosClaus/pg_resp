#!/usr/bin/env python3
"""Run one memtier_benchmark cell against one arm, and commit a raw artifact
that can be audited without trusting this script's summary.

Why this exists in this shape
-----------------------------
Three separate incidents in Phases 2 and 3 produced numbers that were wrong or
unreproducible, and every one of them was preventable by a check that costs
milliseconds. This harness makes those checks structural rather than
remembered:

1. **The raw file did not record its own invocation.** `bench/results/ENV.md`
   §3 had to *reconstruct* the Phase 2 soak's command line from Little's law
   and per-flag provenance tables. Every raw file this script writes begins
   with a verbatim, copy-pasteable header stating the exact argv.

2. **A run against an auth-enabled server measured the cost of refusing
   commands.** memtier neither failed nor warned: it reported a plausible
   176k ops/sec having never executed one GET, in 820 MB of repeated
   `-NOAUTH` lines (ENV.md §4). So: `--authenticate` is mandatory whenever a
   password is configured, the password is verified against the live server
   *before* the run with a real command, and the output is scanned for NOAUTH
   afterwards. A run that trips either check is marked VOID in its own header
   and refuses to produce a summary row.

3. **A benchmark with a 0% hit rate is measuring the wrong thing** and is the
   symptom that exposed (2). A run whose GET hit rate is zero is refused.

On top of that, the Phase 4 environment amendment: WSL2 is a *development*
environment on this project (no cpu governor, hypervisor overhead, client
co-resident with server). Numbers produced there are usable for validating
this harness and nothing else, so `--env-class wsl2-dev` stamps
DEVELOPMENT-ONLY into the raw header and into every generated table row. Only
`--env-class dedicated` produces figures allowed into a published document.

Usage
-----
    python3 bench/harness/sweep.py \
        --arm P-opt --host 127.0.0.1 --port 6379 \
        --data-size 1024 --pipeline 16 --clients 8 --threads 8 \
        --env-class wsl2-dev --auth-from-pg \
        --psql ~/.pgrx/18.4/pgrx-install/bin/psql --pg-port 28818 --pg-host ~/.pgrx

Emits `bench/results/<date>-<arm>-<workload>.txt` (raw + header + verdict) and
appends a row to `bench/results/<date>-<arm>.md`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_MEMTIER = REPO / "ref" / "memtier_benchmark" / "memtier_benchmark"
RESULTS = REPO / "bench" / "results"

ARMS = {
    "R-def": "Redis, stock config (RDB snapshots on — what most people run)",
    "R-opt": 'Redis, cache-tuned: save "", appendonly no, maxmemory + allkeys-lru',
    "V-opt": "Valkey, same tuning as R-opt (the license-clean incumbent)",
    "K-pg": "Redka server on the same PG instance (the SQL-translation architecture)",
    "P-def": "pg_resp on stock postgresql.conf",
    "P-opt": "pg_resp with documented tuning only (max_memory sized)",
}

# An arm whose numbers are only meaningful if the store actually holds data.
ENV_CLASSES = {
    "wsl2-dev": "DEVELOPMENT DATA ONLY — WSL2, no cpu governor, client co-resident. NOT PUBLISHABLE.",
    "dedicated": "publishable — dedicated-vCPU Linux box per Phase 4 environment amendment",
}


class Void(Exception):
    """A run that must not produce a number."""


@dataclass
class Cell:
    arm: str
    data_size: int
    pipeline: int
    clients: int
    threads: int
    test_time: int
    run_count: int

    @property
    def total_conns(self) -> int:
        # --clients is per-thread; they multiply. The single most common memtier
        # misconfiguration (bench-harness skill §3).
        return self.clients * self.threads

    @property
    def workload_id(self) -> str:
        return f"d{self.data_size}-p{self.pipeline}-c{self.total_conns}"


@dataclass
class Result:
    arm: str
    workload_id: str
    env_class: str
    publishable: bool
    total_conns: int
    ops_sec: float
    p50: float
    p99: float
    p999: float
    avg_latency: float
    hits_sec: float
    misses_sec: float
    hit_rate_pct: float
    raw_file: str
    rerun: str


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def verify_server_answers(args, cell: Cell) -> str:
    """Prove the arm executes a real command before measuring it.

    This is the generalisation of the NOAUTH lesson: the failure mode was not
    "we forgot a flag", it was "nothing checked that the server was doing the
    work we were about to time". A SET followed by a matching GET is the
    cheapest possible proof, and it fails loudly for a wrong password, a wrong
    port, a server that is up but not serving, and a store that silently
    discards writes.
    """
    probe_key = f"pg_resp_bench_probe_{cell.workload_id}"
    probe_val = "probe-ok"
    cli = [
        "redis-cli",
        "-h",
        args.host,
        "-p",
        str(args.port),
        "--no-raw",
    ]
    if args.password:
        cli += ["-a", args.password]

    setp = run(cli + ["SET", probe_key, probe_val])
    getp = run(cli + ["GET", probe_key])
    combined = (setp.stdout + setp.stderr + getp.stdout + getp.stderr).strip()

    if "NOAUTH" in combined or "WRONGPASS" in combined:
        raise Void(
            f"pre-run probe was refused by the server: {combined!r}. "
            "The password is wrong or missing — this is exactly the condition "
            "that produced ENV.md §4's three void runs."
        )
    if probe_val not in getp.stdout:
        raise Void(
            f"pre-run probe did not read back what it wrote (SET={setp.stdout.strip()!r} "
            f"GET={getp.stdout.strip()!r}). The arm is not serving; refusing to "
            "measure it."
        )
    run(cli + ["DEL", probe_key])
    return f"SET/GET probe OK ({probe_val!r} read back)"


def password_from_pg(args) -> str | None:
    """Read pg_resp.password from the live instance, per the memtier trap's
    'SHOW pg_resp.password; before' check (ENV.md §4)."""
    psql = [
        args.psql,
        "-h",
        os.path.expanduser(args.pg_host),
        "-p",
        str(args.pg_port),
        "-d",
        "postgres",
        "-tAqc",
        "SHOW pg_resp.password;",
    ]
    proc = run(psql)
    if proc.returncode != 0:
        raise Void(
            f"could not SHOW pg_resp.password (rc={proc.returncode}): "
            f"{proc.stderr.strip()}. Refusing to guess whether auth is needed — "
            "guessing is what voided ENV.md §4's first attempt."
        )
    value = proc.stdout.strip()
    return value or None


def build_memtier_argv(args, cell: Cell) -> list[str]:
    argv = [
        str(args.memtier),
        f"--host={args.host}",
        f"--port={args.port}",
        "--protocol=redis",
        "--ratio=1:10",
        f"--data-size={cell.data_size}",
        "--key-pattern=G:G",
        "--key-maximum=1000000",
        f"--pipeline={cell.pipeline}",
        f"--clients={cell.clients}",
        f"--threads={cell.threads}",
        f"--test-time={cell.test_time}",
        f"--run-count={cell.run_count}",
        "--print-percentiles=50,99,99.9",
        "--hide-histogram",
    ]
    if args.password:
        # Mandatory whenever a password exists. Never omitted "because the
        # last run worked" — the last run may have been measuring refusals.
        argv.append(f"--authenticate={args.password}")
    return argv


STATS_ROW = re.compile(
    r"^Totals\s+([\d.]+)\s+([\d.]+|---)\s+([\d.]+|---)\s+"
    r"([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)"
)


def parse_totals(output: str) -> dict:
    """Take memtier's own Totals row. Never recompute what memtier computed —
    its run_stats.cpp is the source of truth (bench-harness skill §5)."""
    for line in output.splitlines():
        m = STATS_ROW.match(line.strip())
        if m:
            g = [None if v == "---" else float(v) for v in m.groups()]
            return {
                "ops_sec": g[0],
                "hits_sec": g[1] or 0.0,
                "misses_sec": g[2] or 0.0,
                "avg_latency": g[3],
                "p50": g[4],
                "p99": g[5],
                "p999": g[6],
            }
    raise Void(
        "no Totals row in memtier output — the run did not complete. The raw "
        "file is committed anyway so the failure is inspectable."
    )


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--arm", required=True, choices=sorted(ARMS))
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6379)
    ap.add_argument("--data-size", type=int, required=True)
    ap.add_argument("--pipeline", type=int, required=True)
    ap.add_argument("--clients", type=int, required=True)
    ap.add_argument("--threads", type=int, required=True)
    ap.add_argument("--test-time", type=int, default=60)
    ap.add_argument("--run-count", type=int, default=3)
    ap.add_argument(
        "--env-class",
        required=True,
        choices=sorted(ENV_CLASSES),
        help="wsl2-dev stamps DEVELOPMENT-ONLY into every artifact; only "
        "'dedicated' output may enter a published document",
    )
    ap.add_argument("--memtier", default=str(DEFAULT_MEMTIER))
    ap.add_argument("--password", default=None, help="explicit server password")
    ap.add_argument(
        "--auth-from-pg",
        action="store_true",
        help="read pg_resp.password off the live instance (P-* arms)",
    )
    ap.add_argument("--psql", default="psql")
    ap.add_argument("--pg-port", type=int, default=28818)
    ap.add_argument("--pg-host", default="~/.pgrx")
    ap.add_argument("--server-note", default="", help="version/image digest of the arm under test")
    ap.add_argument("--out-dir", default=str(RESULTS))
    ap.add_argument("--dry-run", action="store_true", help="print the argv and exit")
    args = ap.parse_args()

    cell = Cell(
        arm=args.arm,
        data_size=args.data_size,
        pipeline=args.pipeline,
        clients=args.clients,
        threads=args.threads,
        test_time=args.test_time,
        run_count=args.run_count,
    )

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    day = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    raw_path = out_dir / f"{day}-{cell.arm}-{cell.workload_id}.txt"

    verdict: list[str] = []
    void_reason: str | None = None

    try:
        if args.auth_from_pg:
            args.password = password_from_pg(args)
            verdict.append(
                f"SHOW pg_resp.password -> {'set' if args.password else 'unset'} "
                "(read from the live instance, not assumed)"
            )
        if not args.dry_run:
            verdict.append(verify_server_answers(args, cell))
    except Void as e:
        void_reason = str(e)

    argv = build_memtier_argv(args, cell)
    rerun = " ".join(shlex.quote(a) for a in sys.argv)

    if args.dry_run:
        print(" ".join(shlex.quote(a) for a in argv))
        return 0

    started = datetime.now(timezone.utc).isoformat()
    output = ""
    if void_reason is None:
        proc = run(argv)
        output = proc.stdout + proc.stderr
        if proc.returncode != 0:
            void_reason = f"memtier exited {proc.returncode}"

    stats: dict | None = None
    if void_reason is None:
        noauth = output.count("NOAUTH")
        verdict.append(f"NOAUTH occurrences in output: {noauth} (must be 0)")
        if noauth:
            void_reason = (
                f"{noauth} NOAUTH lines in output — this run measured the "
                "throughput of refusing commands, not of serving them (ENV.md §4)"
            )
        else:
            try:
                stats = parse_totals(output)
                total = stats["hits_sec"] + stats["misses_sec"]
                hit_rate = (stats["hits_sec"] / total * 100.0) if total else 0.0
                verdict.append(f"GET hit rate: {hit_rate:.2f}% (must be > 0)")
                if hit_rate <= 0.0:
                    void_reason = (
                        "GET hit rate is 0% — the store served nothing. This is the "
                        "symptom that exposed ENV.md §4's void runs; refusing to "
                        "emit a summary row."
                    )
            except Void as e:
                void_reason = str(e)

    publishable = args.env_class == "dedicated" and void_reason is None

    header = [
        "=" * 78,
        f"pg_resp benchmark raw output — arm {cell.arm} — workload {cell.workload_id}",
        "=" * 78,
        f"arm            : {cell.arm}  ({ARMS[cell.arm]})",
        f"workload       : data-size={cell.data_size} pipeline={cell.pipeline} "
        f"clients={cell.clients} threads={cell.threads} "
        f"=> {cell.total_conns} total connections",
        f"protocol       : 3 runs x {cell.test_time}s, ratio 1:10 SET:GET, "
        "key-pattern G:G, key-maximum 1000000 (bible §10)",
        f"started (UTC)  : {started}",
        f"server note    : {args.server_note or '(none given)'}",
        f"env class      : {args.env_class} — {ENV_CLASSES[args.env_class]}",
        f"publishable    : {'YES' if publishable else 'NO'}",
        "",
        "memtier invocation (verbatim — this line is the contract, iron rule 7):",
        "  " + " ".join(shlex.quote(a) for a in argv),
        "",
        "regenerate this exact file with:",
        "  " + rerun,
        "",
        "pre-run and post-run checks:",
    ]
    header += [f"  - {v}" for v in verdict] or ["  - (none ran)"]
    if void_reason:
        header += [
            "",
            "*" * 78,
            "VOID — THIS RUN PRODUCED NO USABLE NUMBER",
            f"reason: {void_reason}",
            "The output below is retained so the failure is inspectable, which is",
            "why 820 MB of NOAUTH lines were once deleted instead of committed.",
            "*" * 78,
        ]
    header += ["", "=" * 78, "raw memtier output follows", "=" * 78, ""]

    raw_path.write_text("\n".join(header) + output)

    if void_reason:
        print(f"VOID  {cell.arm} {cell.workload_id}: {void_reason}", file=sys.stderr)
        print(f"raw (retained): {raw_path.relative_to(REPO)}", file=sys.stderr)
        return 2

    assert stats is not None
    total = stats["hits_sec"] + stats["misses_sec"]
    result = Result(
        arm=cell.arm,
        workload_id=cell.workload_id,
        env_class=args.env_class,
        publishable=publishable,
        total_conns=cell.total_conns,
        ops_sec=stats["ops_sec"],
        p50=stats["p50"],
        p99=stats["p99"],
        p999=stats["p999"],
        avg_latency=stats["avg_latency"],
        hits_sec=stats["hits_sec"],
        misses_sec=stats["misses_sec"],
        hit_rate_pct=(stats["hits_sec"] / total * 100.0) if total else 0.0,
        raw_file=str(raw_path.relative_to(REPO)),
        rerun=rerun,
    )

    json_path = out_dir / f"{day}-{cell.arm}-{cell.workload_id}.json"
    json_path.write_text(json.dumps(asdict(result), indent=2) + "\n")

    tag = "" if publishable else "  **[dev-only]**"
    print(
        f"{cell.arm:6s} {cell.workload_id:18s} "
        f"{result.ops_sec:12,.2f} ops/s  p50 {result.p50:7.3f}  "
        f"p99 {result.p99:7.3f}  p99.9 {result.p999:8.3f}  "
        f"hit {result.hit_rate_pct:5.2f}%{tag}"
    )
    print(f"raw:  {result.raw_file}")
    print(f"json: {json_path.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
