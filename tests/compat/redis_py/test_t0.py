#!/usr/bin/env python3
"""redis-py compat check against pg_resp's T0 command set.

`protocol=2` is REQUIRED here, not optional: redis-py's `Redis()` defaults
`protocol=None`, which resolves internally to RESP3
(`check_protocol_version`'s `DEFAULT_RESP_VERSION`). A default-constructed
client always sends `HELLO 3` on connect and, found by actually running this
against a real pg_resp instance (bible §5 Phase 1 compat gate), treats ANY
error reply to it as fatal — it does not downgrade-and-retry in RESP2,
regardless of whether the server's reply is a spec-correct `-NOPROTO` or a
plain `-ERR unknown command`. Since bible D9 forbids implementing real RESP3
in v0.1, the only real fix is here, client-side: pin `protocol=2` explicitly,
which is redis-py's documented, correct way to talk to a RESP2-only server.

Usage: python3 test_t0.py [host] [port]
Exit code 0 = all checks passed, 1 = at least one failed.
"""
import sys

import redis

host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
port = int(sys.argv[2]) if len(sys.argv) > 2 else 6379

r = redis.Redis(host=host, port=port, decode_responses=True, socket_timeout=5, protocol=2)

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
