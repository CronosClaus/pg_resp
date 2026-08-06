#!/usr/bin/env python3
"""The Phase 0 spike-S1 lifecycle gate, kept forever (bible §5, §9).

A background worker that ignores SIGTERM blocks `pg_ctl stop` and will — rightly
— get the extension labelled dangerous. So this table is re-run after any change
that touches the worker's lifecycle, not just once in Phase 0:

| check                        | pass condition                                  |
|------------------------------|-------------------------------------------------|
| PING                         | returns PONG                                    |
| pg_ctl stop -m fast          | exits in < 2s, no orphan process, port released |
| pg_ctl restart x N           | zero failures, port rebinds every time          |
| kill -9 the worker           | postmaster restarts it; Postgres itself fine    |

Usage:
    python3 tests/lifecycle/lifecycle.py --pgbin ~/.pgrx/18.4/pgrx-install/bin \\
        --datadir ~/.pgrx/data-18 --pg-port 28818 \\
        [--resp-port 6379] [--password secret123] [--restarts 20]
"""

import argparse
import os
import socket
import subprocess
import sys
import time

sys.path.insert(0, __file__.rsplit("/", 2)[0] + "/differential")
from generate_and_compare import build_command, read_one_reply  # noqa: E402

FAILURES = []


def check(label, ok, detail=""):
    if ok:
        print(f"  ok    {label}")
    else:
        FAILURES.append(f"{label}{': ' + detail if detail else ''}")
        print(f"  FAIL  {label}{': ' + detail if detail else ''}")


def resp_ping(port, password, timeout=3):
    """One PING round trip. Returns the reply bytes, or None if unreachable."""
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=timeout)
    except OSError:
        return None
    try:
        buf = b""
        if password:
            sock.sendall(build_command("AUTH", password))
            _, buf = read_one_reply(sock, buf)
        sock.sendall(build_command("PING"))
        reply, _ = read_one_reply(sock, buf)
        return reply
    except Exception:  # noqa: BLE001
        return None
    finally:
        sock.close()


def port_bound(port):
    try:
        sock = socket.create_connection(("127.0.0.1", port), timeout=1)
        sock.close()
        return True
    except OSError:
        return False


def worker_pids(datadir):
    """PIDs of pg_resp workers belonging to THIS data directory.

    Matching on the process title alone is not enough: `pgrep -f 'postgres:
    pg_resp'` also matches the invoking shell's own command line, and would
    match a containerized pg_resp from a docker-based test running at the same
    time. Checking each candidate's cwd against the data directory removes both
    false positives.
    """
    out = subprocess.run(["pgrep", "-f", "postgres: pg_resp"],
                         capture_output=True, text=True).stdout.split()
    real = []
    for pid in out:
        try:
            if os.path.realpath(f"/proc/{pid}/cwd") == os.path.realpath(datadir):
                real.append(pid)
        except OSError:
            continue
    return real


def pg_ctl(pgbin, datadir, *args, logfile=None):
    cmd = [f"{pgbin}/pg_ctl", "-D", datadir, *args]
    if logfile:
        cmd += ["-l", logfile]
    started = time.monotonic()
    proc = subprocess.run(cmd, capture_output=True, text=True)
    return time.monotonic() - started, proc.returncode, proc.stdout + proc.stderr


def wait_for_resp(port, password, deadline_s):
    started = time.monotonic()
    while time.monotonic() - started < deadline_s:
        if resp_ping(port, password) is not None:
            return time.monotonic() - started
        time.sleep(0.2)
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pgbin", required=True)
    ap.add_argument("--datadir", required=True)
    ap.add_argument("--pg-port", type=int, required=True)
    # pgrx-started instances use unix_socket_directories=~/.pgrx, while a plain
    # `pg_ctl start` uses postgresql.conf's default (/tmp here). Passing the
    # directory explicitly avoids "No such file or directory" on the socket —
    # see the pgrx-patterns skill §8.6.
    ap.add_argument("--pg-host", default=None,
                    help="socket directory or host (e.g. ~/.pgrx for a pgrx-started instance)")
    ap.add_argument("--resp-port", type=int, default=6379)
    ap.add_argument("--password", default=None)
    ap.add_argument("--restarts", type=int, default=20)
    ap.add_argument("--logfile", default="/tmp/pg_resp_lifecycle.log")
    args = ap.parse_args()
    pgbin = os.path.expanduser(args.pgbin)
    datadir = os.path.expanduser(args.datadir)

    # Make sure we start from a running instance.
    if not port_bound(args.resp_port):
        pg_ctl(pgbin, datadir, "start", "-w", logfile=args.logfile)
        time.sleep(2)

    print("\nR4a — PING")
    reply = resp_ping(args.resp_port, args.password)
    check("PING returns PONG", reply is not None and reply.startswith(b"+PONG"),
          repr(reply))

    print("\nR4b — pg_ctl stop -m fast")
    elapsed, rc, out = pg_ctl(pgbin, datadir, "-m", "fast", "stop", "-w")
    check(f"stop succeeds ({elapsed:.2f}s)", rc == 0, out.strip()[:200])
    check("stop completes in under 2s (bible §5 Phase 0 gate)", elapsed < 2.0,
          f"{elapsed:.2f}s")
    time.sleep(0.5)
    orphans = worker_pids(datadir)
    check("no orphaned pg_resp worker for this data directory",
          not orphans, f"pids={orphans}")
    check("the RESP port is released", not port_bound(args.resp_port))

    print(f"\nR4c — restart x{args.restarts}")
    failures, slowest = 0, 0.0
    for i in range(args.restarts):
        _, rc, out = pg_ctl(pgbin, datadir, "start", "-w", logfile=args.logfile)
        if rc != 0:
            failures += 1
            print(f"  start #{i + 1} failed: {out.strip()[:160]}")
            continue
        took = wait_for_resp(args.resp_port, args.password, deadline_s=15)
        if took is None:
            failures += 1
            print(f"  start #{i + 1}: RESP port never became reachable")
        else:
            slowest = max(slowest, took)
        elapsed, rc, out = pg_ctl(pgbin, datadir, "-m", "fast", "stop", "-w")
        if rc != 0 or elapsed >= 2.0:
            failures += 1
            print(f"  stop #{i + 1}: rc={rc} elapsed={elapsed:.2f}s")
    check(f"{args.restarts} start/stop cycles with zero failures", failures == 0,
          f"{failures} failed")
    print(f"  note  slowest time from start to a serving RESP port: {slowest:.2f}s")

    print("\nR4d — kill -9 the worker (postmaster must bring it back)")
    pg_ctl(pgbin, datadir, "start", "-w", logfile=args.logfile)
    if wait_for_resp(args.resp_port, args.password, deadline_s=20) is None:
        check("instance came back up before the kill test", False)
    else:
        before = worker_pids(datadir)
        check("worker is running before the kill", len(before) == 1, f"pids={before}")
        if before:
            subprocess.run(["kill", "-9", before[0]], capture_output=True)
            took = wait_for_resp(args.resp_port, args.password, deadline_s=60)
            check("the RESP service comes back after SIGKILL", took is not None,
                  "never returned")
            if took is not None:
                print(f"  note  recovered in {took:.1f}s")
            after = worker_pids(datadir)
            check("the worker is a new process", after and after != before,
                  f"before={before} after={after}")
            # An external SIGKILL of a shmem-attached worker forces Postgres's
            # own crash-recovery cycle, which is loud and drops every client —
            # documented in docs/ops.md. What matters here is that it recovers
            # without intervention.
            rc = subprocess.run(
                [f"{pgbin}/psql", *(["-h", args.pg_host] if args.pg_host else []),
                 "-p", str(args.pg_port), "-d", "postgres",
                 "-tAc", "SELECT 1"], capture_output=True, text=True)
            check("SQL is usable again afterwards",
                  rc.returncode == 0 and rc.stdout.strip() == "1",
                  rc.stderr.strip()[:160])

    if FAILURES:
        print("\nFAILURES:")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    print("\nall lifecycle checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
