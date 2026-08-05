#!/usr/bin/env python3
"""G2: measure the commit-to-eviction staleness window.

Bible §5 Phase 3 gate: "measured commit->eviction latency histogram published;
p99 < 5 ms on dev box".

WHAT IS ACTUALLY BEING MEASURED, and why the naive version would be
self-flattering
-----------------------------------------------------------------------------
pg_resp's post-commit callback runs *inside* Postgres's commit path — after the
transaction is marked committed, but before `COMMIT` returns to the client. So
the obvious experiment ("commit, then poll the cache until the key is gone")
measures approximately zero, because by the time the writer learns its commit
succeeded, the eviction has already been issued. That number would be true and
useless.

The window that actually exists, and the one an application can observe, opens
when the new row becomes visible to *other* transactions (at commit, when the
transaction is marked committed in clog) and closes when the eviction reaches
the store. A concurrent reader can genuinely see the new row while the cache
still holds the old value in that gap.

This harness measures that window conservatively:

  * `t_commit_start` is taken immediately before `COMMIT` is sent. Row
    visibility cannot happen any earlier than this, so using it as the window's
    start can only *overstate* staleness.
  * `t_evicted` is when a tight RESP polling loop first observes the key gone.
  * **staleness = t_evicted - t_commit_start** — an upper bound on the real
    window.

Only the cache is polled in the hot loop (one RESP round trip, ~50-100us), not
SQL, so resolution is not limited by a SQL round trip. `t_evicted - t_commit_done`
is also reported: it is frequently negative, which is not an error but the
direct evidence that the eviction usually lands before the committing client is
even told the commit succeeded.

Part A separately isolates what the applier costs the committing transaction, by
comparing COMMIT duration for transactions that do and do not queue an
invalidation. That is the price the writer pays; the staleness window above is
what readers see.

Usage:
    python3 bench/harness/staleness.py --psql <path> --pg-port 28818 \\
        [--resp-password secret123] [--iterations 1000] [--out results.md]
"""

import argparse
import json
import socket
import statistics
import subprocess
import sys
import threading
import time

sys.path.insert(0, __file__.rsplit("/", 3)[0] + "/tests/differential")
from generate_and_compare import build_command, read_one_reply  # noqa: E402

KEY = "staleness:product:1"
MARK = "__DONE__"


class Psql:
    """A persistent psql session driven over pipes.

    Deliberately dependency-free: no Python Postgres driver is installed here,
    and a benchmark that is committed to the repo should not require one. Each
    request is terminated by an echoed sentinel so reads are unambiguous.
    """

    def __init__(self, psql, port, db="postgres", host=None):
        self.proc = subprocess.Popen(
            [psql, *(["-h", host] if host else []),
             "-p", str(port), "-d", db, "-tAq", "--no-psqlrc",
             "-P", "pager=off", "-v", "ON_ERROR_STOP=0"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
        )

    def send(self, sql):
        """Run SQL and block until it has actually completed.

        The sentinel is a `SELECT`, not psql's `\\echo`. That matters: `\\echo`
        is written straight to stdout by psql's own command loop and can appear
        *before* the query results it was meant to terminate, so reads returned
        empty and — far worse for a benchmark — returned before the statement
        had finished, which would have silently corrupted every timing here. A
        SELECT sentinel is a query result like any other and cannot overtake the
        results it follows.
        """
        self.proc.stdin.write(f"{sql}\nSELECT '{MARK}';\n")
        self.proc.stdin.flush()
        out = []
        for line in self.proc.stdout:
            stripped = line.rstrip("\n")
            if stripped == MARK:
                break
            out.append(stripped)
        return out

    def close(self):
        try:
            self.proc.stdin.write("\\q\n")
            self.proc.stdin.flush()
            self.proc.wait(timeout=5)
        except Exception:  # noqa: BLE001
            self.proc.kill()


class RespPoller(threading.Thread):
    """Polls one key as fast as it can, timestamping every observation."""

    def __init__(self, host, port, password):
        super().__init__(daemon=True)
        self.host, self.port, self.password = host, port, password
        self.lock = threading.Lock()
        self.present = None
        self.changed_at = None
        self.running = True
        self.samples = 0

    def run(self):
        sock = socket.create_connection((self.host, self.port), timeout=10)
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        buf = b""
        if self.password:
            sock.sendall(build_command("AUTH", self.password))
            _, buf = read_one_reply(sock, buf)
        while self.running:
            sock.sendall(build_command("EXISTS", KEY))
            reply, buf = read_one_reply(sock, buf)
            now = time.monotonic()
            present = reply.strip() == b":1"
            with self.lock:
                self.samples += 1
                if present != self.present:
                    self.present = present
                    self.changed_at = now
        sock.close()

    def state(self):
        with self.lock:
            return self.present, self.changed_at

    def wait_for(self, want, timeout=5.0):
        """Wait until the key's presence is `want`; return the sample time."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            present, at = self.state()
            if present == want:
                return at
            time.sleep(0.0002)
        return None


def percentiles(values):
    if not values:
        return {}
    ordered = sorted(values)

    def pct(p):
        idx = min(len(ordered) - 1, int(round(p / 100.0 * (len(ordered) - 1))))
        return ordered[idx]

    return {
        "n": len(ordered),
        "min_ms": ordered[0] * 1000,
        "p50_ms": pct(50) * 1000,
        "p99_ms": pct(99) * 1000,
        "p999_ms": pct(99.9) * 1000,
        "max_ms": ordered[-1] * 1000,
        "mean_ms": statistics.fmean(ordered) * 1000,
    }


def part_a_commit_cost(pg, iterations):
    """What the applier costs the committing transaction."""
    print(f"\nPart A — COMMIT duration, with vs without a queued invalidation "
          f"(n={iterations} each)")
    pg.send("DROP TABLE IF EXISTS staleness_products;")
    pg.send("CREATE TABLE staleness_products (id int PRIMARY KEY, price numeric);")
    pg.send("INSERT INTO staleness_products VALUES (1, 0);")

    baseline, with_invalidation = [], []
    for i in range(iterations):
        # Baseline: an identical UPDATE with no trigger and nothing queued.
        pg.send("BEGIN;")
        pg.send(f"UPDATE staleness_products SET price={i} WHERE id=1;")
        t0 = time.monotonic()
        pg.send("COMMIT;")
        baseline.append(time.monotonic() - t0)

        # With a queued cache write, which the commit callback must deliver.
        pg.send("BEGIN;")
        pg.send(f"UPDATE staleness_products SET price={i} WHERE id=1;")
        pg.send(f"SELECT resp.del('{KEY}');")
        t0 = time.monotonic()
        pg.send("COMMIT;")
        with_invalidation.append(time.monotonic() - t0)

    return percentiles(baseline), percentiles(with_invalidation)


def part_b_staleness(pg, poller, iterations):
    """The window a concurrent reader can observe."""
    print(f"\nPart B — commit->eviction staleness window (n={iterations})")
    pg.send("DROP TABLE IF EXISTS staleness_products2;")
    pg.send("CREATE TABLE staleness_products2 (id int PRIMARY KEY, price numeric);")
    pg.send("INSERT INTO staleness_products2 VALUES (1, 0);")
    pg.send("DROP TRIGGER IF EXISTS staleness_evict ON staleness_products2;")
    pg.send("CREATE TRIGGER staleness_evict AFTER UPDATE ON staleness_products2 "
            f"FOR EACH ROW EXECUTE FUNCTION resp.evict('{KEY.rsplit(':', 1)[0]}:', 'id');")

    upper_bound, after_commit_returned = [], []
    skipped = 0
    for i in range(iterations):
        # Seed the cache entry and wait until the poller can see it, so the
        # transition we time afterwards is unambiguous.
        pg.send("BEGIN;")
        pg.send(f"SELECT resp.set('{KEY}', 'cached-{i}');")
        pg.send("COMMIT;")
        if poller.wait_for(True, timeout=5.0) is None:
            skipped += 1
            continue

        pg.send("BEGIN;")
        pg.send(f"UPDATE staleness_products2 SET price={i + 1} WHERE id=1;")
        t_commit_start = time.monotonic()
        pg.send("COMMIT;")
        t_commit_done = time.monotonic()

        t_evicted = poller.wait_for(False, timeout=5.0)
        if t_evicted is None:
            skipped += 1
            continue
        upper_bound.append(t_evicted - t_commit_start)
        after_commit_returned.append(t_evicted - t_commit_done)

    return percentiles(upper_bound), after_commit_returned, skipped


def fmt(name, p):
    return (f"| {name} | {p['n']} | {p['min_ms']:.3f} | {p['p50_ms']:.3f} | "
            f"{p['p99_ms']:.3f} | {p['p999_ms']:.3f} | {p['max_ms']:.3f} | "
            f"{p['mean_ms']:.3f} |")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--psql", required=True)
    ap.add_argument("--pg-port", type=int, required=True)
    # pgrx-started instances use unix_socket_directories=~/.pgrx, while a plain
    # `pg_ctl start` uses postgresql.conf's default (/tmp here). Passing the
    # directory explicitly avoids "No such file or directory" on the socket —
    # see the pgrx-patterns skill §8.6.
    ap.add_argument("--pg-host", default=None,
                    help="socket directory or host (e.g. ~/.pgrx for a pgrx-started instance)")
    ap.add_argument("--resp-host", default="127.0.0.1")
    ap.add_argument("--resp-port", type=int, default=6379)
    ap.add_argument("--resp-password", default=None)
    ap.add_argument("--iterations", type=int, default=1000)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    pg = Psql(args.psql, args.pg_port, host=args.pg_host)
    poller = RespPoller(args.resp_host, args.resp_port, args.resp_password)
    poller.start()
    time.sleep(0.3)

    base, with_inv = part_a_commit_cost(pg, min(args.iterations, 500))
    window, after_return, skipped = part_b_staleness(pg, poller, args.iterations)

    poller.running = False
    time.sleep(0.2)

    if not window:
        print("\nERROR: no staleness transitions were observed — nothing to report. "
              "Check that the cache is reachable and the trigger fired.")
        return 1

    header = ("| measurement | n | min | p50 | p99 | p99.9 | max | mean |\n"
              "|---|---|---|---|---|---|---|---|")
    lines = [
        "## G2 — commit-to-eviction staleness",
        "",
        "All figures in milliseconds.",
        "",
        "### Part A — what the applier costs the committing transaction",
        "",
        header,
        fmt("COMMIT, no invalidation queued", base),
        fmt("COMMIT, one invalidation queued", with_inv),
        "",
        f"Added cost at p99: **{with_inv['p99_ms'] - base['p99_ms']:.3f} ms** "
        f"(p50: {with_inv['p50_ms'] - base['p50_ms']:.3f} ms).",
        "",
        "### Part B — the window a concurrent reader can observe",
        "",
        header,
        fmt("staleness window (upper bound)", window),
        "",
        f"- Measured as `t_evicted - t_commit_start`, where `t_commit_start` is "
        f"taken immediately before `COMMIT` is sent. Row visibility cannot "
        f"precede that instant, so this **overstates** the true window.",
        f"- Polling resolution: {poller.samples} cache samples taken during the "
        f"run (one RESP round trip each; SQL is not polled in the hot loop).",
        f"- Iterations skipped because a transition was not observed within "
        f"5s: {skipped}.",
    ]
    if after_return:
        negative = sum(1 for v in after_return if v <= 0)
        lines += [
            f"- `t_evicted - t_commit_done` was <= 0 in "
            f"**{negative}/{len(after_return)}** iterations "
            f"({100.0 * negative / len(after_return):.1f}%), i.e. the eviction "
            f"had already landed before the committing client was told its "
            f"commit succeeded. This is the structural reason the naive "
            f"measurement would have reported ~0.",
        ]
    gate = window["p99_ms"] < 5.0
    lines += [
        "",
        f"**Gate (p99 < 5 ms): {'PASS' if gate else 'FAIL'}** — "
        f"measured p99 {window['p99_ms']:.3f} ms.",
    ]
    report = "\n".join(lines)
    print("\n" + report)

    if args.out:
        with open(args.out, "w") as fh:
            fh.write(report + "\n")
        with open(args.out.replace(".md", ".json"), "w") as fh:
            json.dump({"part_a_baseline": base, "part_a_with_invalidation": with_inv,
                       "part_b_window": window, "skipped": skipped,
                       "cache_samples": poller.samples}, fh, indent=2)
        print(f"\nwrote {args.out}")

    pg.close()
    return 0 if gate else 1


if __name__ == "__main__":
    sys.exit(main())
