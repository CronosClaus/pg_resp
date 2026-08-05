// node-redis compat check against pg_resp's T0 command set.
//
// `RESP: 2` is REQUIRED here, not optional — same root cause as the
// redis-py script's `protocol=2`: node-redis's `RedisClientOptions` type
// defaults `RESP extends RespVersions = 3`, so a default-constructed client
// always attempts RESP3 first and does not gracefully fall back to RESP2 on
// any error reply. Confirmed by actually running this against a real
// pg_resp instance (bible §5 Phase 1 compat gate) before this fix was added.
//
// Usage: node test_t0.js [host] [port]
// Exit code 0 = all checks passed, 1 = at least one failed.
const { createClient } = require("redis");

async function main() {
  const host = process.argv[2] || "127.0.0.1";
  const port = parseInt(process.argv[3] || "6379", 10);
  const client = createClient({ socket: { host, port }, RESP: 2 });
  await client.connect();

  const results = [];
  const check = (desc, actual, expected) => {
    const ok = JSON.stringify(actual) === JSON.stringify(expected);
    results.push([desc, ok, actual, expected]);
  };

  check("ping", await client.ping(), "PONG");
  check("set", await client.set("k", "v"), "OK");
  check("get", await client.get("k"), "v");
  check("get missing", await client.get("missing"), null);
  check("set nx on existing", await client.set("k", "v2", { NX: true }), null);
  check("set xx on missing", await client.set("missingxx", "v", { XX: true }), null);
  check("del", await client.del("k"), 1);
  check("exists after del", await client.exists("k"), 0);
  await client.mSet({ a: "1", b: "2" });
  check("mget", await client.mGet(["a", "z", "b"]), ["1", null, "2"]);
  check("incr", await client.incr("ctr"), 1);
  check("incr again", await client.incr("ctr"), 2);
  await client.set("ek", "v");
  // In RESP2 mode, EXPIRE's reply is the raw integer (:1/:0) — node-redis
  // does not coerce it to a boolean the way it does under RESP3. Found by
  // actually running this: comparing against `true` always failed even
  // though the server's reply was correct.
  check("expire", await client.expire("ek", 10), 1);
  check("ttl", await client.ttl("ek"), 10);

  const failures = results.filter(([, ok]) => !ok);
  for (const [desc, ok, actual, expected] of results) {
    console.log(`${ok ? "OK" : "FAIL"} ${desc}: got=${JSON.stringify(actual)} expected=${JSON.stringify(expected)}`);
  }
  console.log(`\n${results.length - failures.length}/${results.length} passed`);

  // client.quit() sends the wire QUIT command (T1 tier, bible §3.4 — not
  // implemented yet, correctly, since this matrix only tests T0). Found by
  // actually running this: it crashed with "unknown command 'QUIT'" instead
  // of a clean exit. destroy() just closes the socket, no wire command,
  // matching how the go-redis/jedis scripts tear down (plain socket close).
  client.destroy();
  process.exit(failures.length ? 1 : 0);
}

main().catch((err) => {
  console.error("connection or protocol error:", err);
  process.exit(1);
});
