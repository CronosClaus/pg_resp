// jedis compat check against pg_resp's T0 command set.
//
// PARTIALLY VERIFIED: `java` is present in this environment but the `jedis`
// jar and its dependencies are not, and fetching them (Maven Central) was
// not attempted given the effort/value tradeoff versus the other 4 clients
// already covering the same T0 surface from different client libraries
// (see reports/phase1.md). Written carefully against Jedis 5.x's documented
// API; run for real via the dockerized compat matrix
// (`docker compose run jedis`) or any machine with Maven/Jedis available.
//
// Usage: java -cp jedis-5.x.jar:. Test [host] [port]
// Exit code 0 = all checks passed, 1 = at least one failed.
import redis.clients.jedis.Jedis;
import redis.clients.jedis.params.SetParams;

import java.util.Arrays;
import java.util.List;

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
        } catch (Exception e) {
            System.err.println("connection or protocol error: " + e);
            System.exit(1);
        }

        System.out.println("\n" + (total - failures) + "/" + total + " passed");
        System.exit(failures > 0 ? 1 : 0);
    }
}
