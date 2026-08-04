#!/usr/bin/env python3
"""redis-py compat check against pg_resp's T0 command set.

Verified locally (no docker) against a real pg_resp instance on 2026-08-05:
12/12 passed. `redis` was installed via `pip install --target` into a
non-system path to avoid touching this machine's system/Anaconda Python
(see reports/phase1.md for why). In the dockerized compat matrix this same
script runs inside a plain `python:3-slim` + `pip install redis` container.

Usage: python3 test_t0.py [host] [port]
Exit code 0 = all checks passed, 1 = at least one failed.
"""
import sys

import redis

host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
port = int(sys.argv[2]) if len(sys.argv) > 2 else 6379

r = redis.Redis(host=host, port=port, decode_responses=True, socket_timeout=5)

results = []


def check(desc, actual, expected):
    results.append((desc, actual == expected, actual, expected))


check("ping", r.ping(), True)
check("set", r.set("k", "v"), True)
check("get", r.get("k"), "v")
check("get missing", r.get("missing"), None)
check("set nx on existing", r.set("k", "v2", nx=True), None)
check("set xx on missing", r.set("missingxx", "v", xx=True), None)
check("del", r.delete("k"), 1)
check("exists after del", r.exists("k"), 0)
check(
    "mset/mget",
    (r.mset({"a": "1", "b": "2"}), r.mget(["a", "z", "b"])),
    (True, ["1", None, "2"]),
)
check("incr", r.incr("ctr"), 1)
check("incr again", r.incr("ctr"), 2)
check("expire+ttl", (r.set("ek", "v"), r.expire("ek", 10), r.ttl("ek")), (True, True, 10))

failures = [row for row in results if not row[1]]
for desc, ok, actual, expected in results:
    print(f"{'OK' if ok else 'FAIL'} {desc}: got={actual!r} expected={expected!r}")
print(f"\n{len(results) - len(failures)}/{len(results)} passed")

r.close()
sys.exit(1 if failures else 0)
