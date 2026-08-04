// node-redis compat check against pg_resp's T0 command set.
//
// NOT LOCALLY VERIFIED: this WSL2 environment has no `node` runtime
// installed and none could be added without going outside this run's
// allowed write paths (see reports/phase1.md). Written carefully against
// node-redis's documented v4 API; run for real via the dockerized compat
// matrix (`docker compose run node-redis`) or any machine with Node.
//
// Usage: node test_t0.js [host] [port]
// Exit code 0 = all checks passed, 1 = at least one failed.
const { createClient } = require("redis");

async function main() {
  const host = process.argv[2] || "127.0.0.1";
  const port = parseInt(process.argv[3] || "6379", 10);
  const client = createClient({ socket: { host, port } });
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
  check("expire", await client.expire("ek", 10), true);
  check("ttl", await client.ttl("ek"), 10);

  const failures = results.filter(([, ok]) => !ok);
  for (const [desc, ok, actual, expected] of results) {
    console.log(`${ok ? "OK" : "FAIL"} ${desc}: got=${JSON.stringify(actual)} expected=${JSON.stringify(expected)}`);
  }
  console.log(`\n${results.length - failures.length}/${results.length} passed`);

  await client.quit();
  process.exit(failures.length ? 1 : 0);
}

main().catch((err) => {
  console.error("connection or protocol error:", err);
  process.exit(1);
});
