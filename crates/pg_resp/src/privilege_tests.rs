//! `#[pg_test]`s for privilege semantics (`D12`), in a SQL schema named
//! `tests` because pgrx's test runner requires exactly that name.

//! Scope note: only the privilege questions live here, and deliberately so.
//! They are answerable entirely within one transaction and never need the cache
//! to be reachable — `CREATE TRIGGER` and `EXECUTE` checks are catalog
//! operations, and the queued cache write is discarded when the test's
//! transaction rolls back. The gates that *do* need a live cache round trip
//! (G1 rollback safety, G4 stats consistency) stay in
//! `tests/sql_surface/gates.py`.
//!
//! Two naming traps, both of which cost a run to discover:
//!
//! 1. The module MUST be named `tests`. `#[pg_schema]` turns the module name
//!    into a real SQL schema, and pgrx's test runner invokes every `#[pg_test]`
//!    as `"tests"."<fn>"()` with that schema hard-coded — any other name fails
//!    with `schema "tests" does not exist`.
//! 2. It must not be named `pg_tests`: Postgres reserves the `pg_` prefix and
//!    rejects it with `unacceptable schema name`. (The same trap bites role
//!    names — see `tests/sql_surface/gates.py`.)
//!
//! The test instance is configured by `crate::pg_test::postgresql_conf_options`,
//! which preloads pg_resp (mandatory since `_PG_init` refuses to load
//! otherwise) and moves the worker to port 6399 so it does not collide with a
//! developer's own instance on 6379.

#[cfg(any(test, feature = "pg_test"))]
#[pgrx::pg_schema]
mod tests {
    use pgrx::prelude::*;

    /// Fresh role, table, and cached row for one privilege test.
    fn fixture(role: &str) {
        Spi::run(&format!(
            "DROP TABLE IF EXISTS priv_t;
             CREATE TABLE priv_t (id int PRIMARY KEY, price numeric);
             INSERT INTO priv_t VALUES (1, 1.0);
             CREATE ROLE {role};
             GRANT ALL ON priv_t TO {role};"
        ))
        .expect("fixture setup");
    }

    #[pg_test]
    fn resp_functions_are_revoked_from_public() {
        // The D12 default posture. If this ever flips, every cached value
        // becomes readable by every role that can connect.
        Spi::run("CREATE ROLE priv_default_role").expect("create role");
        let can_execute: bool = Spi::get_one(
            "SELECT has_function_privilege('priv_default_role', 'resp.get(text)', 'EXECUTE')",
        )
        .expect("query ran")
        .expect("not null");
        assert!(
            !can_execute,
            "resp.get must not be executable by a role granted nothing"
        );

        let has_usage: bool =
            Spi::get_one("SELECT has_schema_privilege('priv_default_role', 'resp', 'USAGE')")
                .expect("query ran")
                .expect("not null");
        assert!(!has_usage, "schema resp must not grant USAGE to PUBLIC");
    }

    #[pg_test(error = "permission denied for schema resp")]
    fn create_trigger_without_schema_usage_is_refused() {
        // The half of the grant recipe that is easy to forget. Pinned as a test
        // so the ops docs cannot drift from the real behaviour.
        fixture("priv_no_usage");
        Spi::run(
            "SET ROLE priv_no_usage;
             CREATE TRIGGER t AFTER UPDATE ON priv_t
             FOR EACH ROW EXECUTE FUNCTION resp.evict('p:', 'id');",
        )
        .expect("this should have raised");
    }

    #[pg_test]
    fn execute_on_evict_is_checked_at_create_trigger_time_not_at_fire_time() {
        // THE question this test exists to settle. It decides what the grant
        // recipe in docs/ops.md must warn about: if the privilege were
        // re-checked when the trigger fires, revoking it would disarm existing
        // triggers; since it is not, revoking is not a way to turn one off.
        fixture("priv_timing");
        Spi::run(
            "GRANT USAGE ON SCHEMA resp TO priv_timing;
             GRANT EXECUTE ON FUNCTION resp.evict() TO priv_timing;
             SET ROLE priv_timing;
             CREATE TRIGGER t AFTER UPDATE ON priv_t
             FOR EACH ROW EXECUTE FUNCTION resp.evict('p:', 'id');
             RESET ROLE;",
        )
        .expect("create trigger with grants");

        // Take the privilege away again, then fire the trigger.
        Spi::run(
            "REVOKE EXECUTE ON FUNCTION resp.evict() FROM priv_timing;
             SET ROLE priv_timing;
             UPDATE priv_t SET price = 2 WHERE id = 1;
             RESET ROLE;",
        )
        .expect(
            "the UPDATE must still succeed: EXECUTE on a trigger function is \
             checked at CREATE TRIGGER time, not on each firing",
        );

        let price: f64 = Spi::get_one("SELECT price::float8 FROM priv_t WHERE id = 1")
            .expect("query ran")
            .expect("not null");
        assert_eq!(price, 2.0, "the update itself must have applied");
    }
}
