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
import socket
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
    spread_pct: float | None
    n_runs: int
    spread_ok: bool
    raw_file: str
    rerun: str


def rel(p: Path) -> str:
    """Repo-relative when possible, absolute otherwise (--out-dir may be a
    scratch directory outside the tree during harness validation)."""
    try:
        return str(p.relative_to(REPO))
    except ValueError:
        return str(p)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    return subprocess.run(cmd, capture_output=True, text=True, **kw)




def _resp_cmd(*parts: bytes) -> bytes:
    out = b"*%d\r\n" % len(parts)
    for p in parts:
        out += b"$%d\r\n%s\r\n" % (len(p), p)
    return out


def verify_server_answers(args, cell: Cell) -> str:
    """Prove the arm executes a real command, over the path memtier will use.

    This is the generalisation of the NOAUTH lesson: the failure mode was not
    "we forgot a flag", it was "nothing checked that the server was doing the
    work we were about to time". A SET followed by a matching GET is the
    cheapest possible proof, and it fails loudly for a wrong password, a wrong
    port, a server that is up but not serving, and a store that silently
    discards writes.

    **It deliberately uses a raw socket from this process rather than a
    redis-cli**, because the probe must traverse exactly the network path
    memtier_benchmark will traverse. An earlier version shelled out to a
    containerised redis-cli and was therefore able to "verify" an arm that
    memtier could not reach at all: under WSL2 with Docker Desktop,
    `--network host` places a container in the Docker VM's network namespace,
    not the WSL2 distro's, so a containerised client and a native memtier see
    two different sets of reachable servers. A probe that does not use the
    measurement path is not a probe.
    """
    probe_key = f"pg_resp_bench_probe_{cell.workload_id}".encode()
    probe_val = b"probe-ok"

    try:
        sock = socket.create_connection((args.host, args.port), timeout=5)
    except OSError as e:
        raise Void(
            f"cannot reach {args.host}:{args.port} from this process "
            f"({type(e).__name__}: {e}). memtier runs from here, so if this "
            "connection fails the benchmark cannot measure this arm — check "
            "whether the server is in a different network namespace (see the "
            "docstring above)."
        ) from e

    with sock:
        sock.settimeout(5)
        payload = b""
        if args.password:
            payload += _resp_cmd(b"AUTH", args.password.encode())
        payload += _resp_cmd(b"SET", probe_key, probe_val)
        payload += _resp_cmd(b"GET", probe_key)
        payload += _resp_cmd(b"DEL", probe_key)
        sock.sendall(payload)

        buf = b""
        deadline_reads = 0
        while probe_val not in buf and deadline_reads < 20:
            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                break
            if not chunk:
                break
            buf += chunk
            deadline_reads += 1

    text = buf.decode("utf-8", "replace")
    if "NOAUTH" in text or "WRONGPASS" in text:
        raise Void(
            f"pre-run probe was refused by the server: {text.strip()!r}. The "
            "password is wrong or missing — exactly the condition that produced "
            "ENV.md §4's three void runs."
        )
    if probe_val not in buf:
        raise Void(
            f"pre-run probe did not read back what it wrote (got {text.strip()!r}). "
            "The arm is not serving; refusing to measure it."
        )
    return f"SET/GET probe OK over the measurement path ({probe_val!r} read back)"


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
    argv = taskset_prefix(args) + [
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
    if cell.run_count > 1:
        # Needed to compute the median run and the spread ourselves; memtier's
        # own AGGREGATED AVERAGE is a MEAN, and bible §10 asks for medians.
        argv.append("--print-all-runs")
    if args.password:
        # Mandatory whenever a password exists. Never omitted "because the
        # last run worked" — the last run may have been measuring refusals.
        argv.append(f"--authenticate={args.password}")
    return argv


def taskset_prefix(args) -> list[str]:
    """Pin memtier to the client cores (ENV.md §9). Absent = unpinned, and the
    raw header says so, because an unpinned client on a shared box is a
    different experiment from a pinned one.

    On the official box memtier runs inside a container (the host is frozen as
    bootstrapped and has no compiler), so the confinement is applied by
    `docker --cpuset-cpus` in the wrapper rather than by taskset here. Wrapping
    the *docker CLI* in taskset would pin an idle client process and leave
    memtier itself free — a header that said `taskset -c 4-7` would then be
    describing something that never happened.
    """
    if args.client_cpus and args.client_pin_mechanism == "taskset":
        return ["taskset", "-c", args.client_cpus]
    return []


def client_pinning_note(args) -> str:
    """What the raw header records about client confinement — mechanism
    included, because the mechanism is the part a reader cannot re-derive."""
    if not args.client_cpus:
        return "UNPINNED (see ENV.md §9)"
    if args.client_pin_mechanism == "taskset":
        return f"taskset -c {args.client_cpus}"
    return (
        f"docker --cpuset-cpus {args.client_cpus} "
        "(memtier runs containerised; see bench/harness/box/memtier_benchmark)"
    )


ARM_PORTS = {"P-def": 6379, "P-opt": 6379, "R-def": 6380, "R-opt": 6381,
             "V-opt": 6382, "K-pg": 6383}


def check_exclusive(args, cell: Cell) -> str:
    """Fail if any arm other than the one under test is still answering.

    Co-resident arms contend for cores, page cache and memory bandwidth, and an
    idle Redis still holds its whole `maxmemory` allocation — so a run with a
    second arm up measures a different machine than a run without one, with no
    visible sign in the output. ENV.md §8.
    """
    live = []
    for arm, port in ARM_PORTS.items():
        if port == ARM_PORTS[cell.arm]:
            continue  # the arm under test (P-def/P-opt share a port)
        try:
            with socket.create_connection((args.host, port), timeout=1):
                live.append(f"{arm}:{port} (answering)")
        except OSError:
            pass

    # Reachability alone is not enough. Under WSL2 a containerised arm sits in
    # the Docker VM's network namespace, so it is unreachable from here while
    # still running on the same physical CPUs and holding its whole `maxmemory`
    # allocation. The socket probe above cannot see that; docker can.
    ps = run(["docker", "ps", "--format", "{{.Names}}"])
    if ps.returncode == 0:
        running = set(ps.stdout.split())
        for arm in ARM_PORTS:
            if arm == cell.arm or ARM_PORTS[arm] == ARM_PORTS[cell.arm]:
                continue
            cname = "pg_resp_bench_" + arm.replace("-", "_")
            if cname in running:
                live.append(f"{arm} (container {cname} running)")
        for extra in ("kpg_redka", "kpg_pg"):
            if extra in running and cell.arm != "K-pg":
                live.append(f"K-pg support container {extra} running")

    if live:
        raise Void(
            "other arms are still live: " + ", ".join(live) + ". Stop them "
            "first (bench/harness/arms.sh exclusive " + cell.arm + ") — a "
            "co-resident arm changes the machine under test without changing "
            "the output."
        )
    return "exclusivity: no other arm port answers"


def build_warmup_argv(args, cell: Cell) -> list[str]:
    """SET-only pass over the same keyspace, to populate before measuring.

    ENV.md §4's valid saturation runs were taken "against a store already warm
    and at its 256MB budget" and landed at ~37% hit rate; a run against a cold
    store reports a hit rate near zero, and a GET miss returns 5 bytes where a
    hit returns the value. Throughput is therefore not comparable between a
    cold and a warm arm, in a direction that flatters whichever arm was colder.
    Equal warm-up per arm is what makes a cell a comparison rather than a
    coincidence.
    """
    argv = taskset_prefix(args) + [
        str(args.memtier),
        f"--host={args.host}",
        f"--port={args.port}",
        "--protocol=redis",
        "--ratio=1:0",  # SET only
        f"--data-size={cell.data_size}",
        "--key-pattern=G:G",
        "--key-maximum=1000000",
        f"--pipeline={cell.pipeline}",
        f"--clients={cell.clients}",
        f"--threads={cell.threads}",
        f"--test-time={args.warmup_time}",
        "--run-count=1",
        "--hide-histogram",
    ]
    if args.password:
        argv.append(f"--authenticate={args.password}")
    return argv


STATS_ROW = re.compile(
    r"^Totals\s+([\d.]+)\s+([\d.]+|---)\s+([\d.]+|---)\s+"
    r"([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)"
)


RUN_HEADER = re.compile(r"^RUN #(\d+) RESULTS")


def parse_per_run_totals(output: str) -> list[dict]:
    """Totals row of each `RUN #N RESULTS` section, in order.

    Only the RUN #N sections — memtier also prints BEST / WORST / AGGREGATED
    AVERAGE blocks, each with its own Totals row, and mixing those in would
    double-count. Returns [] for a single-run output, which has no RUN sections
    at all (just ALL STATS).
    """
    runs: list[dict] = []
    current: int | None = None
    for line in output.splitlines():
        stripped = line.strip()
        if RUN_HEADER.match(stripped):
            current = int(RUN_HEADER.match(stripped).group(1))
            continue
        if stripped.startswith(("BEST RUN", "WORST RUN", "AGGREGATED")):
            current = None
            continue
        if current is not None:
            m = STATS_ROW.match(stripped)
            if m:
                g = [None if v == "---" else float(v) for v in m.groups()]
                runs.append({
                    "run": current,
                    "ops_sec": g[0],
                    "hits_sec": g[1] or 0.0,
                    "misses_sec": g[2] or 0.0,
                    "avg_latency": g[3],
                    "p50": g[4],
                    "p99": g[5],
                    "p999": g[6],
                })
                current = None
    return runs


def median_run_and_spread(runs: list[dict]) -> tuple[dict, float]:
    """The MEDIAN run by ops/sec, plus (max-min)/median as a percentage.

    bible §10 says "medians reported"; memtier's AGGREGATED AVERAGE block is an
    arithmetic mean, so it is deliberately not used. Reporting the median *run*
    (rather than a median of each column independently) keeps every figure in a
    published row belonging to one real run.
    """
    ordered = sorted(runs, key=lambda r: r["ops_sec"])
    med = ordered[len(ordered) // 2]
    lo, hi = ordered[0]["ops_sec"], ordered[-1]["ops_sec"]
    spread = (hi - lo) / med["ops_sec"] * 100.0 if med["ops_sec"] else float("inf")
    return med, spread


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
    ap.add_argument(
        "--warmup-time",
        type=int,
        default=0,
        help="seconds of SET-only load to populate the keyspace before the "
        "measured run. Cross-arm comparisons REQUIRE an equal, non-zero value: "
        "a cold store returns misses (5-byte replies) where a warm one returns "
        "values, so a cold arm and a warm arm are not measuring the same work.",
    )
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
    ap.add_argument(
        "--spread-threshold",
        type=float,
        default=8.0,
        help="max acceptable (max-min)/median across the 3 runs of a cell, in "
        "percent (official-box acceptance criterion). Exceeded => the cell is "
        "flagged, and re-run once if --rerun-on-spread is set.",
    )
    ap.add_argument(
        "--rerun-on-spread",
        action="store_true",
        help="if the spread exceeds the threshold, run the cell once more and "
        "keep whichever attempt is tighter; both attempts are recorded.",
    )
    ap.add_argument(
        "--require-exclusive",
        action="store_true",
        help="refuse to run if any OTHER arm's port answers (ENV.md §8). "
        "Mandatory for every comparison run; omit only for harness debugging.",
    )
    ap.add_argument(
        "--client-cpus",
        default=None,
        help="CPU list memtier is confined to, e.g. 6-11 (ENV.md §9). Recorded "
        "in the raw header.",
    )
    ap.add_argument(
        "--client-pin-mechanism",
        default="taskset",
        choices=["taskset", "docker-cpuset"],
        help="HOW --client-cpus is enforced. 'taskset' for a native memtier "
        "binary (dev box). 'docker-cpuset' when memtier runs containerised (the "
        "official box is frozen as bootstrapped and cannot build it): the "
        "wrapper passes --cpuset-cpus, and taskset is NOT prepended, because it "
        "would pin the docker CLI and leave memtier unconfined while the header "
        "claimed otherwise.",
    )
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
        if args.require_exclusive and not args.dry_run:
            verdict.append(check_exclusive(args, cell))
        elif not args.dry_run:
            verdict.append(
                "exclusivity: NOT CHECKED (--require-exclusive omitted) — "
                "NOT valid for a cross-arm comparison"
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
    warmup_argv: list[str] = []
    if void_reason is None and args.warmup_time > 0:
        warmup_argv = build_warmup_argv(args, cell)
        wproc = run(warmup_argv)
        if wproc.returncode != 0:
            void_reason = f"warm-up pass exited {wproc.returncode}"
        else:
            verdict.append(
                f"warm-up: {args.warmup_time}s SET-only over the same keyspace"
            )
    elif void_reason is None:
        verdict.append(
            "warm-up: NONE (--warmup-time 0) — valid for harness validation, "
            "NOT valid for a cross-arm comparison"
        )
    if void_reason is None:
        proc = run(argv)
        output = proc.stdout + proc.stderr
        if proc.returncode != 0:
            void_reason = f"memtier exited {proc.returncode}"

    stats: dict | None = None
    spread_pct: float | None = None
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
                per_run = parse_per_run_totals(output)
                if per_run:
                    stats, spread_pct = median_run_and_spread(per_run)
                    verdict.append(
                        "per-run ops/sec: "
                        + ", ".join(f"#{r['run']} {r['ops_sec']:,.0f}" for r in per_run)
                    )
                    verdict.append(
                        f"median run: #{stats['run']} ({stats['ops_sec']:,.2f} ops/s) "
                        "— median, NOT memtier's AGGREGATED AVERAGE (a mean)"
                    )
                    verdict.append(
                        f"spread (max-min)/median: {spread_pct:.2f}% "
                        f"(threshold {args.spread_threshold:.1f}%) -> "
                        + ("OK" if spread_pct <= args.spread_threshold else "EXCEEDED")
                    )
                else:
                    stats = parse_totals(output)
                    spread_pct = None
                    verdict.append(
                        "single run — no spread available; a 1-run cell is not "
                        "acceptable official data (see ENV.md §12)"
                    )
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

    spread_acceptable = (
        void_reason is None
        and spread_pct is not None
        and spread_pct <= args.spread_threshold
    )
    publishable = (
        args.env_class == "dedicated" and void_reason is None and spread_acceptable
    )

    header = [
        "=" * 78,
        f"pg_resp benchmark raw output — arm {cell.arm} — workload {cell.workload_id}",
        "=" * 78,
        f"arm            : {cell.arm}  ({ARMS[cell.arm]})",
        f"workload       : data-size={cell.data_size} pipeline={cell.pipeline} "
        f"clients={cell.clients} threads={cell.threads} "
        f"=> {cell.total_conns} total connections",
        f"protocol       : {cell.run_count} run(s) x {cell.test_time}s, ratio 1:10 "
        "SET:GET, key-pattern G:G, key-maximum 1000000 (bible §10)",
        f"started (UTC)  : {started}",
        f"server note    : {args.server_note or '(none given)'}",
        f"client pinning : {client_pinning_note(args)}",
        f"env class      : {args.env_class} — {ENV_CLASSES[args.env_class]}",
        f"publishable    : {'YES' if publishable else 'NO'}",
        "",
        "credential note: any --authenticate value below is recorded verbatim on",
        "purpose — iron rule 7 requires a command line that actually reruns. Per the",
        "Phase 4 standing rule the benchmark box uses a freshly generated throwaway",
        "password and is destroyed after the sweep; never reuse it anywhere.",
        "",
        "memtier invocation (verbatim — this line is the contract, iron rule 7):",
        "  " + " ".join(shlex.quote(a) for a in argv),
        "",
        "warm-up invocation (verbatim):",
        "  " + (" ".join(shlex.quote(a) for a in warmup_argv) if warmup_argv else "(none — see checks below)"),
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
        print(f"raw (retained): {rel(raw_path)}", file=sys.stderr)
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
        spread_pct=spread_pct,
        n_runs=cell.run_count,
        spread_ok=(spread_pct is not None and spread_pct <= args.spread_threshold),
        raw_file=rel(raw_path),
        rerun=rerun,
    )

    json_path = out_dir / f"{day}-{cell.arm}-{cell.workload_id}.json"
    json_path.write_text(json.dumps(asdict(result), indent=2) + "\n")

    # Say WHY a cell is unpublishable, not just that it is. This used to print a
    # blanket "[dev-only]", which on the dedicated box was actively misleading:
    # a smoke cell there is unpublishable because it is a single run with no
    # spread to check (§12), not because it came from WSL2. A reader who saw
    # "dev-only" against a dedicated-box number would draw the wrong conclusion
    # about where it was measured.
    if publishable:
        tag = ""
    elif args.env_class != "dedicated":
        tag = "  **[dev-only: not the official box]**"
    elif spread_pct is None:
        tag = "  **[unpublishable: single run, no spread]**"
    else:
        tag = f"  **[unpublishable: spread {spread_pct:.2f}% > {args.spread_threshold:.0f}%]**"
    print(
        f"{cell.arm:6s} {cell.workload_id:18s} "
        f"{result.ops_sec:12,.2f} ops/s  p50 {result.p50:7.3f}  "
        f"p99 {result.p99:7.3f}  p99.9 {result.p999:8.3f}  "
        f"hit {result.hit_rate_pct:5.2f}%{tag}"
    )
    print(f"raw:  {result.raw_file}")
    print(f"json: {rel(json_path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
