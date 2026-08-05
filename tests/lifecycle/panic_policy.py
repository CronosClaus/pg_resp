#!/usr/bin/env python3
"""S6: panic blast radius and the watchdog, verified by deliberate failure.

pg_resp has two panic paths with deliberately different consequences, and the
difference is the whole design:

  * **Per-connection panic** — contained by the fence in `service_connection`'s
    caller. Exactly one connection dies; bystanders and new connections are
    unaffected.
  * **Top-level panic** — the server thread is gone, so the process exits and
    the postmaster restarts the worker. This *replaced* a catch-and-linger
    policy that was the worst possible outcome: Postgres healthy, worker
    process alive, `SELECT 1` fine, and the RESP service permanently dead with
    nothing anywhere reporting a problem (bible §13).

The claim being tested is not "it does not crash" but "it fails in the right
shape, and recovers by itself". Also answered here, empirically rather than
from memory: whether the worker's FATAL exit (status 1) restarts just the
worker, or drags the whole cluster through crash recovery — a difference every
operator cares about, since the latter drops every SQL connection.

Requires the extension built with `--features debug_panic`, which adds the
`DEBUG PANIC-CONNECTION` / `DEBUG PANIC-TOPLEVEL` commands. They do not exist
in a default build.

Usage:
    python3 tests/lifecycle/panic_policy.py --psql <path> --pg-port 28818 \\
        [--host 127.0.0.1] [--port 6379] [--password secret123] \\
        [--logfile /path/to/pg.log]
"""

import argparse
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


def connect(host, port, password, timeout=5):
    sock = socket.create_connection((host, port), timeout=timeout)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    buf = b""
    if password:
        sock.sendall(build_command("AUTH", password))
        reply, buf = read_one_reply(sock, buf)
        if not reply.startswith(b"+"):
            raise RuntimeError(f"AUTH failed: {reply!r}")
    return sock, buf


def ping(sock, buf):
    sock.sendall(build_command("PING"))
    reply, buf = read_one_reply(sock, buf)
    return reply, buf


def wait_for_service(host, port, password, deadline_s):
    """Wait until a fresh connection can complete a PING."""
    started = time.monotonic()
    last = None
    while time.monotonic() - started < deadline_s:
        try:
            sock, buf = connect(host, port, password, timeout=2)
            reply, _ = ping(sock, buf)
            sock.close()
            if reply.startswith(b"+PONG"):
                return time.monotonic() - started, None
        except Exception as exc:  # noqa: BLE001
            last = f"{type(exc).__name__}: {exc}"
        time.sleep(0.25)
    return None, last


def sql(psql, port, query):
    proc = subprocess.run(
        [psql, "-p", str(port), "-d", "postgres", "-tA", "-c", query],
        capture_output=True, text=True,
    )
    return proc.returncode, proc.stdout.strip(), proc.stderr.strip()


def worker_pid():
    out = subprocess.run(["pgrep", "-f", "postgres: pg_resp"],
                         capture_output=True, text=True).stdout.split()
    return out[0] if out else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--psql", required=True)
    ap.add_argument("--pg-port", type=int, required=True)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=6379)
    ap.add_argument("--password", default=None)
    ap.add_argument("--logfile", default=None)
    args = ap.parse_args()

    # ---- 1. per-connection panic is contained ----
    print("\nS6a — a per-connection panic costs exactly one connection")
    victim, v_buf = connect(args.host, args.port, args.password)
    bystander, b_buf = connect(args.host, args.port, args.password)
    reply, b_buf = ping(bystander, b_buf)
    check("bystander healthy before the panic", reply.startswith(b"+PONG"))

    victim.sendall(build_command("DEBUG", "PANIC-CONNECTION"))
    victim_died = False
    try:
        data = victim.recv(4096)
        victim_died = (data == b"")
    except Exception:  # noqa: BLE001
        victim_died = True
    check("the panicking connection is dropped", victim_died)

    time.sleep(0.5)
    try:
        reply, b_buf = ping(bystander, b_buf)
        bystander_ok = reply.startswith(b"+PONG")
        detail = repr(reply[:40])
    except Exception as exc:  # noqa: BLE001
        bystander_ok, detail = False, f"{type(exc).__name__}: {exc}"
    check("a bystander connection survives it", bystander_ok, detail)

    try:
        fresh, f_buf = connect(args.host, args.port, args.password)
        reply, _ = ping(fresh, f_buf)
        fresh.close()
        new_ok = reply.startswith(b"+PONG")
    except Exception as exc:  # noqa: BLE001
        new_ok = False
    check("new connections are still accepted", new_ok)
    bystander.close()

    pid_before = worker_pid()
    check("the worker process did not restart", pid_before is not None)

    # ---- 2. top-level panic: exit and self-heal ----
    print("\nS6b — a top-level panic exits the worker, which then restarts")
    rc, sql_before, _ = sql(args.psql, args.pg_port, "SELECT 1")
    check("SQL is healthy before the panic", rc == 0 and sql_before == "1")

    # A long-lived SQL session, held open across the worker's death. If the
    # worker's exit triggered cluster-wide crash recovery, this session would be
    # terminated — which is the difference between "one worker restarted" and
    # "every client in the cluster dropped".
    held = subprocess.Popen(
        [args.psql, "-p", str(args.pg_port), "-d", "postgres", "-tA", "-c",
         "SELECT pg_sleep(20); SELECT 'session-survived'"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    time.sleep(1.0)

    trigger, t_buf = connect(args.host, args.port, args.password)
    trigger.sendall(build_command("DEBUG", "PANIC-TOPLEVEL"))
    try:
        read_one_reply(trigger, t_buf)
    except Exception:  # noqa: BLE001
        pass
    trigger.close()

    # The loop panics on its next poll tick; the main thread notices within
    # WATCHDOG_INTERVAL (3s) and exits, then bgw_restart_time (1s) applies.
    elapsed, err = wait_for_service(args.host, args.port, args.password, deadline_s=45)
    check("the RESP service comes back by itself", elapsed is not None,
          err or "")
    if elapsed is not None:
        print(f"  note  recovered in {elapsed:.1f}s "
              f"(watchdog interval 3s + bgw_restart_time 1s)")

    pid_after = worker_pid()
    check("the worker really is a new process",
          pid_after is not None and pid_after != pid_before,
          f"before={pid_before} after={pid_after}")

    # ---- 3. blast radius: did the cluster survive? ----
    print("\nS6c — blast radius of the worker's FATAL exit")
    rc, out, err = sql(args.psql, args.pg_port, "SELECT 1")
    check("SQL still works after the worker restarted", rc == 0 and out == "1", err)

    try:
        held_out, held_err = held.communicate(timeout=40)
    except subprocess.TimeoutExpired:
        held.kill()
        held_out, held_err = "", "timed out"
    session_survived = "session-survived" in held_out
    check("a SQL session held open across the restart survived",
          session_survived,
          (held_err or held_out).strip()[:200])
    print(f"  ANSWER  the worker's FATAL exit "
          f"{'does NOT' if session_survived else 'DOES'} force cluster-wide "
          f"crash recovery")

    if args.logfile:
        try:
            with open(args.logfile, "r", errors="replace") as fh:
                tail = fh.read()[-8000:]
            saw_fatal = "restarting the pg_resp background worker" in tail
            check("the exit is logged loudly at FATAL", saw_fatal)
            if "server thread is gone" in tail or "watchdog probe" in tail:
                print("  note  watchdog/thread-death message present in the log")
        except OSError as exc:
            print(f"  note  could not read logfile: {exc}")

    if FAILURES:
        print("\nFAILURES:")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    print("\nall panic-policy checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
