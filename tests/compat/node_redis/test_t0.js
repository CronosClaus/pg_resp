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

  // --- T1/T2 (bible §3.4, phase 2) ---
  check("dbsize before flush", (await client.dbSize()) >= 1, true);
  check("flushdb", await client.flushDb(), "OK");
  check("dbsize after flush", await client.dbSize(), 0);
  await client.setEx("sk", 50, "v");
  check("setex ttl", await client.ttl("sk"), 50);
  // In RESP2 mode, SETNX/SET NX returns integer (1 for success, 0 for already
  // exists), not boolean. This is the raw server reply, not coerced to bool
  // like in RESP3. Noted by running against pg_resp.
  check("setnx new", await client.setNX("nk", "v1"), 1);
  check("setnx existing", await client.setNX("nk", "v2"), 0);
  await client.set("gk", "v");
  check("getdel", await client.getDel("gk"), "v");
  check("getdel removed key", await client.get("gk"), null);
  await client.set("gek", "v", { EX: 100 });
  check("getex persist", await client.getEx("gek", { PERSIST: true }), "v");
  check("getex ttl after persist", await client.ttl("gek"), -1);
  await client.set("pk", "v", { EX: 10 });
  // In RESP2 mode, PERSIST returns integer (1 for success, 0 for key doesn't
  // exist or had no TTL), not boolean. Raw server reply, not RESP3-coerced.
  check("persist", await client.persist("pk"), 1);
  check("ttl after persist", await client.ttl("pk"), -1);
  check("type string", await client.type("pk"), "string");
  check("type none", await client.type("missingkey"), "none");
  await client.mSet({ "user:1": "a", "user:2": "b" });
  const keys = await client.keys("user:*");
  check("keys pattern", keys.slice().sort(), ["user:1", "user:2"]);
  // node-redis's SCAN command requires cursor to already be a string/Buffer
  // (its internal parser.push(cursor) has no numeric-argument path) — found
  // by reading the installed package's SCAN.js: parseScanArguments() does
  // `parser.push(cursor)` with no .toString(), unlike its own now-unused
  // pushScanArguments() helper which does convert. A JS number literal here
  // throws "arguments[1] must be of type string | Buffer" before any command
  // reaches the wire. transformReply() returns cursor back as a string, so
  // only the initial value needs to be a string.
  let scanned = new Set();
  let cursor = "0";
  do {
    const result = await client.scan(cursor, { COUNT: 5 });
    cursor = result.cursor;
    for (const k of result.keys) scanned.add(k);
  } while (cursor !== "0");
  check(
    "scan finds keys set via mset",
    ["user:1", "user:2"].every((k) => scanned.has(k)),
    true
  );
  const info = await client.info();
  check("info has used_memory", info.includes("used_memory"), true);

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
