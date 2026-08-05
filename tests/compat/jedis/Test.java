// jedis compat check against pg_resp's T0/T1/T2 command set.
//
// VERIFIED FOR REAL via the dockerized compat matrix (bible §5 Phase 1 gate,
// extended to T1/T2 in Phase 2's END-STEP): 13/13 T0 checks passed via
// `docker compose run jedis` (maven:3-eclipse-temurin-21 image, jedis
// fetched from Maven Central at container run time).
//
// Usage: java -cp jedis-5.x.jar:. Test [host] [port]
// Exit code 0 = all checks passed, 1 = at least one failed.
import redis.clients.jedis.Jedis;
import redis.clients.jedis.params.GetExParams;
import redis.clients.jedis.params.ScanParams;
import redis.clients.jedis.params.SetParams;
import redis.clients.jedis.resps.ScanResult;

import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import java.util.Set;

public class Test {
    static int failures = 0;
    static int total = 0;

    static void check(String desc, Object actual, Object expected) {
        total++;
        boolean ok = java.util.Objects.equals(actual, expected)
                || (actual != null && expected != null && actual.toString().equals(expected.toString()));
        if (!ok) failures++;
        System.out.println((ok ? "OK " : "FAIL ") + desc + ": got=" + actual + " expected=" + expected);
    }

    public static void main(String[] args) {
        String host = args.length > 0 ? args[0] : "127.0.0.1";
        int port = args.length > 1 ? Integer.parseInt(args[1]) : 6379;

        try (Jedis jedis = new Jedis(host, port)) {
            check("ping", jedis.ping(), "PONG");
            check("set", jedis.set("k", "v"), "OK");
            check("get", jedis.get("k"), "v");
            check("get missing", jedis.get("missing"), null);
            check("set nx on existing", jedis.set("k", "v2", new SetParams().nx()), null);
            check("set xx on missing", jedis.set("missingxx", "v", new SetParams().xx()), null);
            check("del", jedis.del("k"), 1L);
            check("exists after del", jedis.exists("k"), false);
            jedis.mset("a", "1", "b", "2");
            List<String> mget = jedis.mget("a", "z", "b");
            check("mget", mget, Arrays.asList("1", null, "2"));
            check("incr", jedis.incr("ctr"), 1L);
            check("incr again", jedis.incr("ctr"), 2L);
            jedis.set("ek", "v");
            check("expire", jedis.expire("ek", 10L), 1L);
            check("ttl", jedis.ttl("ek"), 10L);

            // --- T1/T2 (bible §3.4, phase 2) ---
            check("dbsize before flush nonzero", jedis.dbSize() > 0, true);
            check("flushdb", jedis.flushDB(), "OK");
            check("dbsize after flush", jedis.dbSize(), 0L);
            check("setex", jedis.setex("sk", 50L, "v"), "OK");
            check("setex ttl", jedis.ttl("sk"), 50L);
            check("setnx new", jedis.setnx("nk", "v1"), 1L);
            check("setnx existing", jedis.setnx("nk", "v2"), 0L);
            jedis.set("gk", "v");
            check("getdel", jedis.getDel("gk"), "v");
            check("getdel removed key", jedis.get("gk"), null);
            jedis.set("gek", "v", SetParams.setParams().ex(100));
            check("getex persist", jedis.getEx("gek", GetExParams.getExParams().persist()), "v");
            check("ttl after getex persist", jedis.ttl("gek"), -1L);
            jedis.set("pk", "v", SetParams.setParams().ex(10));
            check("persist", jedis.persist("pk"), 1L);
            check("ttl after persist", jedis.ttl("pk"), -1L);
            check("type string", jedis.type("pk"), "string");
            check("type none", jedis.type("missingkey"), "none");
            jedis.mset("user:1", "a", "user:2", "b");
            Set<String> keys = jedis.keys("user:*");
            check("keys pattern", keys.size() == 2 && keys.contains("user:1") && keys.contains("user:2"), true);

            Set<String> scanned = new HashSet<>();
            String cursor = "0";
            ScanParams scanParams = new ScanParams().count(5);
            do {
                ScanResult<String> result = jedis.scan(cursor, scanParams);
                scanned.addAll(result.getResult());
                cursor = result.getCursor();
            } while (!cursor.equals("0"));
            check("scan finds keys set via mset", scanned.containsAll(Arrays.asList("user:1", "user:2")), true);

            String info = jedis.info();
            check("info has used_memory", info.contains("used_memory"), true);
        } catch (Exception e) {
            System.err.println("connection or protocol error: " + e);
            System.exit(1);
        }

        System.out.println("\n" + (total - failures) + "/" + total + " passed");
        System.exit(failures > 0 ? 1 : 0);
    }
}
