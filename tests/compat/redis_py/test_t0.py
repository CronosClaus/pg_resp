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

# --- T1/T2 (bible §3.4, phase 2) ---
check("dbsize before flush", r.dbsize() >= 1, True)
check("flushdb", r.flushdb(), True)
check("dbsize after flush", r.dbsize(), 0)
# redis-py's SELECT response callback coerces the +OK simple-string reply to
# bool True, same convention as flushdb()/set() above — not the literal 'OK'.
check("select 0", r.execute_command("SELECT", 0), True)
check("setex", (r.setex("sk", 50, "v"), r.ttl("sk")), (True, 50))
check("setnx new", r.setnx("nk", "v1"), True)
check("setnx existing", r.setnx("nk", "v2"), False)
check("getdel", (r.set("gk", "v"), r.getdel("gk"), r.get("gk")), (True, "v", None))
check(
    "getex persist",
    (r.set("gek", "v", ex=100), r.getex("gek", persist=True), r.ttl("gek")),
    (True, "v", -1),
)
check("persist", (r.set("pk", "v", ex=10), r.persist("pk"), r.ttl("pk")), (True, True, -1))
check("type string", r.type("pk"), "string")
check("type none", r.type("missingkey"), "none")
r.mset({"user:1": "a", "user:2": "b"})
check("keys pattern", sorted(r.keys("user:*")), ["user:1", "user:2"])
scanned = set()
cursor = 0
while True:
    cursor, batch = r.scan(cursor, count=5)
    scanned.update(batch)
    if cursor == 0:
        break
check("scan full iteration finds keys set via mset", {"user:1", "user:2"} <= scanned, True)
info = r.info()
check("info has used_memory", "used_memory" in info, True)

failures = [row for row in results if not row[1]]
for desc, ok, actual, expected in results:
    print(f"{'OK' if ok else 'FAIL'} {desc}: got={actual!r} expected={expected!r}")
print(f"\n{len(results) - len(failures)}/{len(results)} passed")

r.close()
sys.exit(1 if failures else 0)
