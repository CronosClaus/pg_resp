#!/usr/bin/env python3
"""Phase 3 SQL-surface gate harness.

Runs the two automatable bible §5 Phase 3 gates against a live instance, plus
the semantics assertions that back them up:

  G1  rollback safety   — resp.set + ROLLBACK leaves no key; + COMMIT does
  G4  stats consistency — resp.stats() equals INFO's stats/memory numbers

Why a script instead of `#[pg_test]`: these gates are only meaningful against a
running *bgworker*, which means an instance started with
`shared_preload_libraries = 'pg_resp'`. pgrx's own test harness spins up its
database without that, so a `#[pg_test]` here could only ever assert "the cache
is unreachable". Driving a real instance through psql is the honest way to test
a feature whose whole substance is crossing the process boundary.

Usage:
    python3 tests/sql_surface/gates.py \\
        --psql ~/.pgrx/18.4/pgrx-install/bin/psql --pg-port 28818 \\
        --resp-host 127.0.0.1 --resp-port 6379 [--resp-password secret123] \\
        [--db postgres]

Exit code 0 = every gate and assertion passed.
"""

import argparse
import socket
import subprocess
import sys

# Reuse the differential harness's RESP client rather than writing a third one.
sys.path.insert(0, __file__.rsplit("/", 2)[0] + "/differential")
from generate_and_compare import build_command, read_one_reply  # noqa: E402

FAILURES = []
CHECKS = 0


def check(label, actual, expected):
    global CHECKS
    CHECKS += 1
    if actual != expected:
        FAILURES.append(f"{label}: expected {expected!r}, got {actual!r}")
        print(f"  FAIL  {label}: expected {expected!r}, got {actual!r}")
    else:
        print(f"  ok    {label}")


class Pg:
    def __init__(self, psql, port, db):
        self.psql, self.port, self.db = psql, port, db

    def run(self, sql, expect_error=False):
        """Run SQL, returning stripped stdout. Raises on unexpected error."""
        proc = subprocess.run(
            [self.psql, "-p", str(self.port), "-d", self.db, "-tA", "-v",
             "ON_ERROR_STOP=1", "-c", sql],
            capture_output=True, text=True,
        )
        if proc.returncode != 0 and not expect_error:
            raise RuntimeError(f"psql failed on {sql!r}:\n{proc.stderr}")
        if expect_error:
            return proc.stderr.strip()
        return proc.stdout.strip()

    def scalar(self, expr):
        return self.run(f"SELECT {expr}")

    def txn_value(self, statements, expr):
        """Run `statements` then read `expr`, all in one session.

        psql -tA still echoes command tags (BEGIN, COMMIT, ...) on stdout, so
        picking the last output line is not reliable. Tag the value instead.
        """
        out = self.run(f"{statements} SELECT 'RESULT:' || COALESCE(({expr})::text, '<NULL>')")
        for line in out.splitlines():
            if line.startswith("RESULT:"):
                return line[len("RESULT:"):]
        raise RuntimeError(f"no RESULT line in psql output:\n{out}")


def resp_command(host, port, password, *args):
    sock = socket.create_connection((host, port), timeout=5)
    try:
        buf = b""
        if password:
            sock.sendall(build_command("AUTH", password))
            reply, buf = read_one_reply(sock, buf)
            if not reply.startswith(b"+"):
                raise RuntimeError(f"AUTH failed: {reply!r}")
        sock.sendall(build_command(*args))
        reply, buf = read_one_reply(sock, buf)
        return reply
    finally:
        sock.close()


def parse_info(raw):
    """INFO comes back as a bulk string: `$<len>\\r\\n<payload>`."""
    payload = raw.split(b"\r\n", 1)[1] if raw.startswith(b"$") else raw
    fields = {}
    for line in payload.decode("utf-8", "replace").splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        for sep in (":", "="):
            if sep in line:
                key, _, value = line.partition(sep)
                # db0:keys=N -> the inner key wins
                if "=" in value:
                    inner_key, _, inner_val = value.partition("=")
                    fields[f"{key}:{inner_key}"] = inner_val.strip()
                else:
                    fields[key.strip()] = value.strip()
                break
    return fields


def gate_g1_rollback_safety(pg):
    print("\nG1 — rollback safety (bible §5 Phase 3)")
    pg.run("BEGIN; SELECT resp.del('g1a'), resp.del('g1b'), resp.del('g1c'); COMMIT")

    pg.run("BEGIN; SELECT resp.set('g1a','v'); ROLLBACK")
    check("ROLLBACK leaves the key absent", pg.scalar("resp.get('g1a') IS NULL"), "t")

    pg.run("BEGIN; SELECT resp.set('g1b','v'); COMMIT")
    check("COMMIT makes the key present", pg.scalar("resp.get('g1b')"), "v")

    # The shape that actually proves the callback is not leaking across
    # transactions: a committed txn followed by a rolled-back one in the SAME
    # session. A per-transaction queue that failed to reset would apply the
    # second txn's write here.
    value = pg.txn_value(
        "BEGIN; SELECT resp.set('g1c','first'); COMMIT; "
        "BEGIN; SELECT resp.set('g1c','second'); ROLLBACK;",
        "resp.get('g1c')")
    check("two txns in one session: rollback does not overwrite", value, "first")

    # Savepoint / EXCEPTION-block handling (Scope call A).
    pg.run("BEGIN; SELECT resp.del('g1sp'), resp.del('g1ex'); COMMIT")
    pg.run("BEGIN; SAVEPOINT s; SELECT resp.set('g1sp','x'); ROLLBACK TO SAVEPOINT s; COMMIT")
    check("ROLLBACK TO SAVEPOINT discards the subxact's write",
          pg.scalar("resp.get('g1sp') IS NULL"), "t")

    pg.run("BEGIN; SAVEPOINT s; SELECT resp.set('g1sp','x'); RELEASE SAVEPOINT s; ROLLBACK")
    check("RELEASE then outer ROLLBACK still discards (re-parenting)",
          pg.scalar("resp.get('g1sp') IS NULL"), "t")

    pg.run("""BEGIN;
        DO $$ BEGIN
          PERFORM resp.set('g1ex','outer');
          BEGIN
            PERFORM resp.set('g1exinner','inner');
            RAISE EXCEPTION 'deliberate';
          EXCEPTION WHEN OTHERS THEN NULL; END;
        END $$;
        COMMIT""")
    check("plpgsql EXCEPTION block discards its own write",
          pg.scalar("resp.get('g1exinner') IS NULL"), "t")
    check("plpgsql EXCEPTION block keeps the outer write",
          pg.scalar("resp.get('g1ex')"), "outer")


def gate_g4_stats_consistency(pg, host, port, password):
    print("\nG4 — stats consistency: resp.stats() == INFO (bible §5 Phase 3)")
    # Generate a little traffic so the counters are non-zero and a mismatch
    # would actually show up (comparing 0 == 0 proves nothing).
    pg.run("BEGIN; SELECT resp.set('g4hit','v'); COMMIT")
    for _ in range(3):
        pg.scalar("resp.get('g4hit')")
        pg.scalar("resp.get('g4miss-nonexistent')")

    info = parse_info(resp_command(host, port, password, "INFO"))
    row = pg.run(
        "SELECT keys, used_bytes, max_memory_bytes, keyspace_hits, "
        "keyspace_misses, evicted_keys, invalidations_lost FROM resp.stats()"
    ).split("|")
    stats = dict(zip(
        ["keys", "used_bytes", "max_memory_bytes", "keyspace_hits",
         "keyspace_misses", "evicted_keys", "invalidations_lost"], row))

    # INFO field name -> resp.stats() column name
    pairs = [
        ("db0:keys", "keys"),
        ("used_memory", "used_bytes"),
        ("maxmemory", "max_memory_bytes"),
        ("keyspace_hits", "keyspace_hits"),
        ("keyspace_misses", "keyspace_misses"),
        ("evicted_keys", "evicted_keys"),
        ("invalidations_lost", "invalidations_lost"),
    ]
    for info_key, col in pairs:
        check(f"{col} == INFO {info_key}", stats[col], info.get(info_key))

    # Sanity: the traffic above must have moved the counters, otherwise this
    # gate is comparing two zeroes and proving nothing.
    check("hits counter is non-zero (gate is comparing real numbers)",
          int(stats["keyspace_hits"]) > 0, True)
    check("misses counter is non-zero", int(stats["keyspace_misses"]) > 0, True)


def semantics_assertions(pg):
    print("\nDocumented semantics (docs/semantics.md)")
    pg.run("BEGIN; SELECT resp.set('sem','old'); COMMIT")
    value = pg.txn_value("BEGIN; SELECT resp.set('sem','new');", "resp.get('sem')")
    check("a txn does not read its own uncommitted write", value, "old")
    pg.run("BEGIN; SELECT resp.set('sem','new'); COMMIT")
    check("after commit the new value is visible", pg.scalar("resp.get('sem')"), "new")

    check("TTL sentinel: missing key is -2", pg.scalar("resp.ttl('sem-absent')"), "-2")
    pg.run("BEGIN; SELECT resp.set('sem-nottl','v'); COMMIT")
    check("TTL sentinel: key with no expiry is -1", pg.scalar("resp.ttl('sem-nottl')"), "-1")

    pg.run("BEGIN; SELECT resp.set('sem-empty',''); COMMIT")
    check("empty value round-trips as '' and not NULL",
          pg.scalar("resp.get('sem-empty') = ''"), "t")
    check("absent key is NULL, distinct from empty",
          pg.scalar("resp.get('sem-absent') IS NULL"), "t")

    err = pg.run("SELECT resp.set('bad','v', 0)", expect_error=True)
    check("non-positive ttl is rejected", "invalid expire time" in err, True)


def trigger_assertions(pg):
    print("\nresp.evict() trigger helper")
    pg.run("DROP TABLE IF EXISTS gate_products")
    pg.run("CREATE TABLE gate_products (id int PRIMARY KEY, price numeric)")
    pg.run("CREATE TRIGGER ev AFTER UPDATE OR DELETE ON gate_products "
           "FOR EACH ROW EXECUTE FUNCTION resp.evict('gp:', 'id')")
    pg.run("INSERT INTO gate_products VALUES (1, 1.0), (2, 2.0)")

    pg.run("BEGIN; SELECT resp.set('gp:1','cached'), resp.set('gp:2','cached'); COMMIT")
    pg.run("BEGIN; UPDATE gate_products SET price=9 WHERE id=1; COMMIT")
    check("UPDATE evicts its row's key", pg.scalar("resp.get('gp:1') IS NULL"), "t")
    check("UPDATE leaves other rows' keys alone", pg.scalar("resp.get('gp:2')"), "cached")

    pg.run("BEGIN; UPDATE gate_products SET price=8 WHERE id=2; ROLLBACK")
    check("a rolled-back UPDATE does not evict", pg.scalar("resp.get('gp:2')"), "cached")

    # Identity change: the entry cached under the OLD key would be stranded if
    # only the new key were evicted.
    pg.run("BEGIN; SELECT resp.set('gp:2','c'), resp.set('gp:3','c'); COMMIT")
    pg.run("BEGIN; UPDATE gate_products SET id=3 WHERE id=2; COMMIT")
    check("key-column change evicts the old key", pg.scalar("resp.get('gp:2') IS NULL"), "t")
    check("key-column change evicts the new key", pg.scalar("resp.get('gp:3') IS NULL"), "t")

    pg.run("BEGIN; SELECT resp.set('gp:3','c'); COMMIT")
    pg.run("BEGIN; DELETE FROM gate_products WHERE id=3; COMMIT")
    check("DELETE evicts", pg.scalar("resp.get('gp:3') IS NULL"), "t")

    for sql, needle, label in [
        ("CREATE TRIGGER bad AFTER UPDATE ON gate_products FOR EACH STATEMENT "
         "EXECUTE FUNCTION resp.evict('gp:', 'id')", "FOR EACH ROW", "statement-level misuse"),
        ("CREATE TRIGGER bad AFTER UPDATE ON gate_products FOR EACH ROW "
         "EXECUTE FUNCTION resp.evict('gp:', 'nope')", "does not have", "unknown column"),
        ("CREATE TRIGGER bad AFTER UPDATE ON gate_products FOR EACH ROW "
         "EXECUTE FUNCTION resp.evict('gp:')", "exactly 2", "wrong arg count"),
        ("CREATE TRIGGER bad AFTER UPDATE ON gate_products FOR EACH ROW "
         "EXECUTE FUNCTION resp.evict('gp:', 'price')", "cannot build a cache key",
         "unsupported key type"),
    ]:
        pg.run(sql)
        err = pg.run("UPDATE gate_products SET price=1 WHERE id=1", expect_error=True)
        check(f"{label} raises a clear error", needle in err, True)
        pg.run("DROP TRIGGER bad ON gate_products")

    pg.run("DROP TABLE gate_products")


def privilege_assertions(pg):
    """D12: pin exactly which privileges are needed, and WHEN they are checked.

    The open question was whether `EXECUTE` on `resp.evict()` is enforced when
    the trigger is CREATEd or when it FIRES. That distinction decides what the
    grant recipe has to say, and guessing it would be how a docs page ends up
    wrong, so it is answered here by experiment.
    """
    print("\nD12 — privileges: what is needed, and when it is checked")
    pg.run("DROP TABLE IF EXISTS priv_products")
    if pg.scalar("count(*) FROM pg_roles WHERE rolname='resp_gate_user'") != "0":
        pg.run("REVOKE ALL ON SCHEMA resp FROM resp_gate_user")
        pg.run("REVOKE ALL ON ALL FUNCTIONS IN SCHEMA resp FROM resp_gate_user")
        pg.run("DROP ROLE resp_gate_user")
    pg.run("CREATE ROLE resp_gate_user LOGIN")
    pg.run("CREATE TABLE priv_products (id int PRIMARY KEY, price numeric)")
    pg.run("GRANT ALL ON priv_products TO resp_gate_user")
    pg.run("INSERT INTO priv_products VALUES (1, 1.0)")

    # 1. The default posture: nothing is reachable by a plain role.
    check("resp.get is not executable by PUBLIC by default",
          pg.scalar("has_function_privilege('resp_gate_user', 'resp.get(text)', 'EXECUTE')"),
          "f")
    check("schema resp has no USAGE for PUBLIC by default",
          pg.scalar("has_schema_privilege('resp_gate_user', 'resp', 'USAGE')"), "f")

    # 2. CREATE TRIGGER without privileges must be refused. This is the first
    #    half of the answer: the check happens at creation time.
    err = pg.run("SET ROLE resp_gate_user; "
                 "CREATE TRIGGER priv_ev AFTER UPDATE ON priv_products "
                 "FOR EACH ROW EXECUTE FUNCTION resp.evict('pp:', 'id')",
                 expect_error=True)
    check("CREATE TRIGGER is refused without schema USAGE", "permission denied" in err, True)

    # 3. Apply the documented recipe. USAGE ON SCHEMA is the easy-to-forget
    #    half — without it the failure above says "permission denied for schema
    #    resp", which does not obviously point at a GRANT USAGE.
    pg.run("GRANT USAGE ON SCHEMA resp TO resp_gate_user")
    pg.run("GRANT EXECUTE ON FUNCTION resp.get(text), resp.set(text,text,bigint), "
           "resp.del(text), resp.evict() TO resp_gate_user")
    pg.run("SET ROLE resp_gate_user; "
           "CREATE TRIGGER priv_ev AFTER UPDATE ON priv_products "
           "FOR EACH ROW EXECUTE FUNCTION resp.evict('pp:', 'id')")
    check("CREATE TRIGGER succeeds once USAGE + EXECUTE are granted",
          pg.scalar("count(*) FROM pg_trigger WHERE tgname='priv_ev'"), "1")

    pg.run("BEGIN; SELECT resp.set('pp:1','cached'); COMMIT")
    value = pg.txn_value(
        "SET ROLE resp_gate_user; BEGIN; UPDATE priv_products SET price=3 WHERE id=1; COMMIT;",
        "resp.get('pp:1') IS NULL")
    check("a non-superuser's trigger evicts after the documented grants", value, "true")
    value = pg.txn_value("SET ROLE resp_gate_user;", "resp.get('pp:1') IS NULL")
    check("a non-superuser can call resp.get after the documented grants", value, "true")

    # 4. The second half of the answer: REVOKE EXECUTE *after* the trigger
    #    exists, then fire it. If it still works, the privilege was only ever
    #    checked at CREATE TRIGGER time — which is what the recipe must say,
    #    because it means revoking later does not disarm existing triggers.
    pg.run("BEGIN; SELECT resp.set('pp:1','cached-again'); COMMIT")
    pg.run("REVOKE EXECUTE ON FUNCTION resp.evict() FROM resp_gate_user")
    err = pg.run("SET ROLE resp_gate_user; UPDATE priv_products SET price=4 WHERE id=1",
                 expect_error=True)
    fires_after_revoke = (err == "")
    if fires_after_revoke:
        evicted = pg.scalar("resp.get('pp:1') IS NULL")
        check("EXECUTE is checked at CREATE TRIGGER time, not at fire time "
              "(trigger still fires and evicts after REVOKE)", evicted, "t")
    else:
        check("EXECUTE is re-checked at fire time (trigger refused after REVOKE)",
              "permission denied" in err, True)
    print(f"  ANSWER  resp.evict() EXECUTE is enforced at "
          f"{'CREATE TRIGGER time only' if fires_after_revoke else 'fire time as well'}")

    pg.run("DROP TABLE priv_products")
    # A role holding granted privileges cannot be dropped until they are
    # revoked — the grants themselves are dependent objects.
    pg.run("REVOKE ALL ON SCHEMA resp FROM resp_gate_user")
    pg.run("REVOKE ALL ON ALL FUNCTIONS IN SCHEMA resp FROM resp_gate_user")
    pg.run("DROP ROLE resp_gate_user")
    return fires_after_revoke


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--psql", required=True)
    ap.add_argument("--pg-port", type=int, required=True)
    ap.add_argument("--db", default="postgres")
    ap.add_argument("--resp-host", default="127.0.0.1")
    ap.add_argument("--resp-port", type=int, default=6379)
    ap.add_argument("--resp-password", default=None)
    args = ap.parse_args()

    pg = Pg(args.psql, args.pg_port, args.db)
    gate_g1_rollback_safety(pg)
    gate_g4_stats_consistency(pg, args.resp_host, args.resp_port, args.resp_password)
    semantics_assertions(pg)
    trigger_assertions(pg)
    privilege_assertions(pg)

    print(f"\n{CHECKS - len(FAILURES)}/{CHECKS} checks passed")
    if FAILURES:
        print("\nFAILURES:")
        for f in FAILURES:
            print(f"  - {f}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
