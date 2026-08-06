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
import threading
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
    # Explicit client configuration. Stage A's artifacts lack these and curve.py
    # falls back to parsing them out of `rerun`; recording them directly means a
    # consumer never has to parse a command line to know what ran.
    data_size: int = 0
    pipeline: int = 0
    clients: int = 0
    threads: int = 0
    # Warm-up protocol identity. A published table must not mix v1 and v2
    # (phase4-night2.md), and the only way to enforce that downstream is for each
    # cell to carry which protocol produced it.
    warmup_protocol: str = "unknown"
    warmup_keys: int = 0
    warmup_time: int = 0
    # Saturation evidence (ENV.md §21.3).
    cpu_peak_pct: float | None = None
    cpu_median_pct: float | None = None
    cpu_samples: int = 0
    saturated: bool = False
    saturation_enforced: bool = False


def rel(p: Path) -> str:
    """Repo-relative when possible, absolute otherwise (--out-dir may be a
    scratch directory outside the tree during harness validation)."""
    try:
        return str(p.relative_to(REPO))
    except ValueError:
        return str(p)


def run(cmd: list[str], **kw) -> subprocess.CompletedProcess:
    """Run a command, turning a missing binary into a normal non-zero result.

    Without this, a missing `psql` raised FileNotFoundError out of the live-config
    capture and killed an otherwise valid run. A configuration snapshot that
    cannot be taken is a gap to record in the artifact, not grounds to discard a
    measurement — so the failure has to arrive as data, like every other failed
    check here.
    """
    try:
        return subprocess.run(cmd, capture_output=True, text=True, **kw)
    except (FileNotFoundError, PermissionError) as e:
        return subprocess.CompletedProcess(cmd, 127, "", f"{type(e).__name__}: {e}")




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

    TWO PROTOCOLS, AND v2 IS THE PUBLISHED ONE
    ------------------------------------------
    **v1 — `--warmup-time S`: a fixed DURATION.** What Stage A used, and the
    reason its hit rates swung from 36% to 100% across cells of the same arm: 60 s
    at pipeline 16 with 64 connections writes vastly more data than 60 s at
    pipeline 1 with 1 connection, so every cell began from a different degree of
    fill. The arms' hit-rate difference spanned a ~65-point band
    (-27.6 to +37.0 pts). Equal *duration* is not equal *treatment* when the
    client configuration is the thing that varies along the curve.

    **v2 — `--warmup-keys N`: a fixed NUMBER OF WRITES.** Every cell of every arm
    pre-populates with the same number of SETs over the same key space and access
    pattern the measured run will use, at the cell's own payload size. The written
    volume is then a property of the protocol rather than of the cell's
    concurrency, which is what makes hit rates comparable across the curve.

    Pre-population deliberately uses the measured run's `G:G` gaussian pattern
    over the full 1M key space rather than a sequential fill of a smaller range:
    a sequential fill of keys 1..N would populate a region the gaussian read
    pattern (centred mid-range) largely does not visit, producing a low hit rate
    on every arm for a reason that has nothing to do with either design.

    What v2 does NOT do is equalise hit rate *between* arms. A capped arm evicts
    and an uncapped one does not (ENV.md §20); that difference is architectural,
    is the thing under test, and is reported per cell rather than engineered away.
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
        "--run-count=1",
        "--hide-histogram",
    ]
    if args.warmup_keys:
        # --requests is PER CLIENT, and clients multiply by threads, so the
        # divisor is total connections. Rounded up so the pass never falls short
        # of the declared key count; at least 1 request per connection.
        per_client = max(1, -(-args.warmup_keys // cell.total_conns))
        argv.append(f"--requests={per_client}")
    else:
        argv.append(f"--test-time={args.warmup_time}")
    if args.password:
        argv.append(f"--authenticate={args.password}")
    return argv


def warmup_protocol_note(args, cell: Cell) -> str:
    """One line for the raw header saying which warm-up protocol produced the
    state this cell was measured against. A published table must not mix the
    two, so the artifact has to say which it is."""
    if args.warmup_keys:
        per_client = max(1, -(-args.warmup_keys // cell.total_conns))
        actual = per_client * cell.total_conns
        return (
            f"v2 fixed key-count — {args.warmup_keys} requested, {actual} actual "
            f"({per_client} per connection x {cell.total_conns} connections), "
            f"SET-only, G:G over 1M keys, {cell.data_size} B values"
        )
    if args.warmup_time:
        return (
            f"v1 fixed duration — {args.warmup_time}s SET-only at this cell's own "
            "client config. SUPERSEDED: written volume varies with concurrency, "
            "so hit rate is not comparable across cells (see phase4-night2.md)"
        )
    return "NONE — valid for harness validation only, never for a comparison"


SERVER_CONTAINERS = {
    "P-def": "pg_resp_bench_pg",
    "P-opt": "pg_resp_bench_pg",
    "R-def": "pg_resp_bench_R_def",
    "R-opt": "pg_resp_bench_R_opt",
    "V-opt": "pg_resp_bench_V_opt",
    # K-pg is two processes: redka translates, PostgreSQL executes. Both are
    # "the server" for saturation purposes, and reporting only one of them would
    # understate how hard the arm is working.
    "K-pg": "pg_resp_bench_K_pg,pg_resp_bench_pg",
}


class CpuSampler(threading.Thread):
    """Sample the server container(s)' CPU every ~1s for the duration of a run.

    ENV.md §21.3 requires this: a throughput comparison in which one server was
    never core-saturated is measuring the harness, not the servers. The samples
    are committed beside the raw output so the claim is checkable rather than
    asserted.

    `docker stats` reports CPU as a percentage of ONE core, so 100% means one
    logical CPU fully busy. That is the relevant ceiling here because both
    pg_resp (D4) and Redis execute commands on a single thread — a
    single-threaded server pinned to four CPUs saturates at ~100%, not ~400%,
    and a rule expecting 400% would void every honest cell.
    """

    def __init__(self, arm: str, interval: float = 1.0):
        super().__init__(daemon=True)
        self.names = SERVER_CONTAINERS.get(arm, "").split(",")
        self.names = [n for n in self.names if n]
        self.interval = interval
        self.samples: list[float] = []
        self.raw: list[str] = []
        # NOT `self._stop`: threading.Thread has a private _stop() method that
        # join() calls, and shadowing it with an Event makes join() raise
        # "TypeError: 'Event' object is not callable" after the run completes —
        # i.e. it breaks the harness at the moment it tries to record results.
        self._stop_evt = threading.Event()

    def run(self) -> None:
        while not self._stop_evt.is_set():
            total = 0.0
            parts = []
            for name in self.names:
                proc = subprocess.run(
                    ["docker", "stats", "--no-stream", "--format", "{{.CPUPerc}}", name],
                    capture_output=True, text=True,
                )
                val = proc.stdout.strip().rstrip("%")
                try:
                    pct = float(val)
                except ValueError:
                    continue
                total += pct
                parts.append(f"{name}={pct:.1f}%")
            if parts:
                self.samples.append(total)
                self.raw.append(f"{total:8.1f}%  " + "  ".join(parts))
            # docker stats --no-stream itself costs ~0.5-1.5s, so this is a
            # best-effort ~1s cadence, not a guaranteed one. The sample count is
            # recorded so a reader can see the real cadence.
            self._stop_evt.wait(self.interval)

    def stop(self) -> None:
        self._stop_evt.set()

    def verdict(self, threshold: float) -> tuple[bool, str]:
        if not self.samples:
            return False, "no CPU samples collected (docker stats unavailable?)"
        s = sorted(self.samples)
        med = s[len(s) // 2]
        peak = s[-1]
        line = (
            f"{len(s)} samples over ~{len(s) * self.interval:.0f}s — "
            f"min {s[0]:.1f}%  median {med:.1f}%  peak {peak:.1f}%  "
            f"(100% = one logical CPU fully busy)"
        )
        return peak >= threshold, line


def live_config(args, cell: Cell) -> list[str]:
    """Ask the RUNNING server what it is configured as, and put the answer in the
    raw header (ENV.md §21.3).

    Not the config file — the server. A file that was mounted but never parsed,
    or an image whose defaults differ from what the file assumes, is invisible to
    any check that reads the file. The smoke stage already produced exactly that
    class of error: pg_resp was preloaded from postgresql.conf.sample while the
    arm's own conf said nothing about it (ENV.md §19 item 2).
    """
    out: list[str] = []
    if cell.arm.startswith("P-"):
        for guc in (
            "pg_resp.max_memory", "pg_resp.eviction", "pg_resp.bind_address",
            "shared_preload_libraries", "shared_buffers", "synchronous_commit",
        ):
            proc = run([
                args.psql, "-h", os.path.expanduser(args.pg_host), "-p", str(args.pg_port),
                "-d", "postgres", "-tAqc", f"SHOW {guc};",
            ])
            val = proc.stdout.strip() if proc.returncode == 0 else f"<query failed rc={proc.returncode}>"
            out.append(f"  SHOW {guc} = {val}")
        return out

    # Redis/Valkey/redka: ask over RESP, on the same loopback path the
    # measurement uses.
    params = ["save", "appendonly", "io-threads", "maxmemory", "maxmemory-policy"]
    for p in params:
        reply = resp_command(args, ["CONFIG", "GET", p])
        out.append(f"  CONFIG GET {p} -> {reply}")
    return out


def resp_command(args, words: list[str]) -> str:
    """One RESP command over a raw socket, reply rendered for the header.

    Raw socket rather than a containerised redis-cli, for the same reason the
    pre-run probe uses one: a check that does not traverse the measurement path
    can pass for a server the benchmark cannot reach (progress report §D.2).
    """
    try:
        with socket.create_connection((args.host, args.port), timeout=5) as sock:
            payload = b""
            if args.password:
                payload += _resp_cmd(b"AUTH", args.password.encode())
            payload += _resp_cmd(*[w.encode() for w in words])
            sock.sendall(payload)
            sock.settimeout(5)
            buf = b""
            deadline = 40
            while len(buf) < 4096 and deadline > 0:
                try:
                    chunk = sock.recv(4096)
                except socket.timeout:
                    break
                if not chunk:
                    break
                buf += chunk
                deadline -= 1
                if buf.endswith(b"\r\n"):
                    break
    except OSError as e:
        return f"<unreachable: {e}>"
    text = buf.decode("utf-8", "replace").replace("\r\n", " ").strip()
    return text or "<empty reply>"


STATS_ROW = re.compile(
    r"^Totals\s+([\d.]+)\s+([\d.]+|---)\s+([\d.]+|---)\s+"
    r"([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)\s+([\d.]+|---)"
)


RUN_HEADER = re.compile(r"^RUN #(\d+) RESULTS")


# The retry above is deliberately single-shot. ENV.md §12 says "re-run ONCE",
# and a cell that is unstable twice is a finding to publish, not a dice roll to
# repeat until it passes. Kept as a named constant so the intent is explicit
# rather than implied by the absence of a loop.
_is_retry = False


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
        "--warmup-keys",
        type=int,
        default=0,
        help="WARM-UP v2: pre-populate with this many SET requests (a fixed "
        "NUMBER OF WRITES) instead of a fixed duration. Takes precedence over "
        "--warmup-time. v2 is the published protocol — a fixed duration writes "
        "different volumes at different concurrencies, which is what made Stage "
        "A's hit rate swing 36%%-100%% across cells of one arm.",
    )
    ap.add_argument(
        "--cpu-sample-interval",
        type=float,
        default=1.0,
        help="seconds between server CPU samples during the measured run "
        "(ENV.md §21.3). Samples are always taken and always committed.",
    )
    ap.add_argument(
        "--saturation-threshold",
        type=float,
        default=90.0,
        help="peak server CPU%% at or above which the arm counts as core "
        "saturated. 100%% = one logical CPU fully busy; both pg_resp (D4) and "
        "Redis execute commands single-threaded, so ~100%% is the real ceiling "
        "and a 400%% expectation would void every honest cell.",
    )
    ap.add_argument(
        "--require-saturation",
        action="store_true",
        help="VOID the cell if the server never reached --saturation-threshold. "
        "Use for anomaly-comparison cells (ENV.md §21.3), NOT for the whole grid: "
        "low-load cells are legitimate latency-probing points on the "
        "throughput-vs-p99 curve and are meant to be unsaturated.",
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
    if void_reason is None and (args.warmup_keys > 0 or args.warmup_time > 0):
        warmup_argv = build_warmup_argv(args, cell)
        wproc = run(warmup_argv)
        if wproc.returncode != 0:
            void_reason = f"warm-up pass exited {wproc.returncode}"
        else:
            verdict.append("warm-up: " + warmup_protocol_note(args, cell))
    elif void_reason is None:
        verdict.append(
            "warm-up: NONE — valid for harness validation, "
            "NOT valid for a cross-arm comparison"
        )

    # Captured AFTER warm-up and BEFORE the measured run: this is the
    # configuration the timed run actually executes under (ENV.md §21.3).
    config_lines: list[str] = []
    if void_reason is None:
        config_lines = live_config(args, cell)

    sampler: CpuSampler | None = None
    sat_line = "not sampled"
    saturated = False
    if void_reason is None:
        sampler = CpuSampler(cell.arm, interval=args.cpu_sample_interval)
        sampler.start()
        proc = run(argv)
        sampler.stop()
        sampler.join(timeout=5)
        output = proc.stdout + proc.stderr
        if proc.returncode != 0:
            void_reason = f"memtier exited {proc.returncode}"
        saturated, sat_line = sampler.verdict(args.saturation_threshold)
        if args.require_saturation and void_reason is None and not saturated:
            void_reason = (
                f"server never reached core saturation ({sat_line}); "
                f"--require-saturation was set because this cell is part of an "
                "anomaly comparison, and a comparison where one server is not "
                "core-saturated measures the harness rather than the servers "
                "(ENV.md §21.3)"
            )

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

                    # ENV.md §12: "If a cell exceeds 8%, it is RE-RUN ONCE and
                    # the outcome flagged either way; both attempts stay
                    # committed."
                    #
                    # That behaviour was documented in the protocol and declared
                    # as a flag, but --rerun-on-spread was never read by this
                    # script — an accepted no-op. Stage A never noticed because
                    # every one of its 18 cells passed the gate on the first
                    # attempt. It was found when a grid cell reported 9.91% and
                    # produced no second attempt.
                    #
                    # The retry is a genuine repeat: fresh CPU samples, fresh
                    # parse. Whichever attempt is TIGHTER is kept, and both raw
                    # outputs are retained in this artifact so the discarded
                    # attempt is auditable rather than invisible.
                    if (
                        spread_pct > args.spread_threshold
                        and args.rerun_on_spread
                        and not _is_retry
                    ):
                        verdict.append(
                            "spread exceeded threshold -> RE-RUNNING ONCE "
                            "(ENV.md §12); both attempts are recorded below"
                        )
                        retry_sampler = CpuSampler(
                            cell.arm, interval=args.cpu_sample_interval
                        )
                        retry_sampler.start()
                        retry_proc = run(argv)
                        retry_sampler.stop()
                        retry_sampler.join(timeout=5)
                        retry_out = retry_proc.stdout + retry_proc.stderr
                        retry_runs = (
                            parse_per_run_totals(retry_out)
                            if retry_proc.returncode == 0
                            else []
                        )
                        if retry_runs:
                            r_stats, r_spread = median_run_and_spread(retry_runs)
                            verdict.append(
                                "attempt 2 per-run ops/sec: "
                                + ", ".join(
                                    f"#{r['run']} {r['ops_sec']:,.0f}" for r in retry_runs
                                )
                            )
                            verdict.append(
                                f"attempt 2 spread: {r_spread:.2f}% -> "
                                + ("OK" if r_spread <= args.spread_threshold else "EXCEEDED")
                            )
                            if r_spread < spread_pct:
                                verdict.append(
                                    f"KEEPING ATTEMPT 2 (spread {r_spread:.2f}% < "
                                    f"{spread_pct:.2f}%); attempt 1 raw output retained below"
                                )
                                output = (
                                    "=== ATTEMPT 1 (DISCARDED — wider spread) ===\n"
                                    + output
                                    + "\n=== ATTEMPT 2 (KEPT) ===\n"
                                    + retry_out
                                )
                                stats, spread_pct = r_stats, r_spread
                                sat2, sat_line2 = retry_sampler.verdict(
                                    args.saturation_threshold
                                )
                                saturated, sat_line = sat2, sat_line2
                                sampler = retry_sampler
                            else:
                                verdict.append(
                                    f"KEEPING ATTEMPT 1 (spread {spread_pct:.2f}% <= "
                                    f"{r_spread:.2f}%); attempt 2 raw output retained below"
                                )
                                output = (
                                    "=== ATTEMPT 1 (KEPT) ===\n"
                                    + output
                                    + "\n=== ATTEMPT 2 (DISCARDED — wider spread) ===\n"
                                    + retry_out
                                )
                        else:
                            verdict.append(
                                f"attempt 2 produced no parseable runs "
                                f"(rc={retry_proc.returncode}); keeping attempt 1"
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
        f"warm-up        : {warmup_protocol_note(args, cell)}",
        f"server CPU     : {sat_line}",
        f"saturated      : {'YES' if saturated else 'NO'} "
        f"(threshold {args.saturation_threshold:.0f}% peak; "
        f"{'enforced' if args.require_saturation else 'recorded only, not enforced for this cell'})",
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
    header += ["", "live server configuration, read from the RUNNING server (ENV.md §21.3):"]
    header += config_lines or ["  (not captured)"]
    if sampler is not None and sampler.raw:
        header += ["", f"server CPU samples, ~{args.cpu_sample_interval:g}s apart (total, then per container):"]
        header += [f"  {line}" for line in sampler.raw]
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
        data_size=cell.data_size,
        pipeline=cell.pipeline,
        clients=cell.clients,
        threads=cell.threads,
        warmup_protocol=("v2" if args.warmup_keys else ("v1" if args.warmup_time else "none")),
        warmup_keys=args.warmup_keys,
        warmup_time=args.warmup_time,
        cpu_peak_pct=(max(sampler.samples) if sampler and sampler.samples else None),
        cpu_median_pct=(
            sorted(sampler.samples)[len(sampler.samples) // 2]
            if sampler and sampler.samples else None
        ),
        cpu_samples=(len(sampler.samples) if sampler else 0),
        saturated=saturated,
        saturation_enforced=args.require_saturation,
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
