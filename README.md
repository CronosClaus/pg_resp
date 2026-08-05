# pg_resp

> **DRAFT — PENDING HUMAN REVIEW. Not announced anywhere.**
> The first screen below is a draft for external review (Phase 4 W7/W14).
> Benchmark figures marked `PENDING` are deliberately empty: the measured grid is
> running, and a placeholder number in a README is the one thing this project
> cannot afford. Nothing here is published until the numbers are in and reviewed.

**A Redis-protocol (RESP2) cache server that runs inside a PostgreSQL background
worker.** `redis-cli` connects to your database and it answers. One fewer service
to run, patch, monitor and page someone about.

```bash
redis-cli -p 6379 SET greeting hello
redis-cli -p 6379 GET greeting
# → "hello"      ... served from inside PostgreSQL
```

It is **in-process**: not a proxy, not a sidecar, not a translation layer that
turns every `GET` into a `SELECT`. The cache lives in the background worker's own
heap, and PostgreSQL is the process that hosts it.

## Why this instead of the alternatives

| | what it is | the trade you are making |
|---|---|---|
| **Redis / Valkey** | the incumbent, a separate service | Faster than pg_resp at raw ops/sec — **we do not compete on that and say so with numbers.** You run, secure, patch and monitor a second stateful service, and your cache cannot see your transactions |
| **Redka** | a Redis-compatible Go server with a PostgreSQL backend | Durable, and its author is explicit that raw performance is not the goal. Every operation is a transaction against indexed tables, which costs ~2 row inserts and ~4 index scans per `SET`. Structurally slower per operation — see [BENCHMARKS](docs/BENCHMARKS.md) |
| **`UNLOGGED` table + `pg_cron`** | the blog-post cache | No eviction policy, no TTL semantics, a full SQL round trip per operation, and an application rewrite to use it |
| **pg_resp** | RESP2 server in a bgworker | Ephemeral by design, single-threaded, no data structures beyond strings and TTLs. In exchange: your existing Redis client works unchanged, and cache invalidation can be a database trigger |

Redka is the closest neighbour and the honest comparison; it is durable where
pg_resp is not, and those row inserts are what durability costs.

## The part only an in-database cache can do

Cache invalidation is normally an application concern, which means it is correct
only as long as every code path remembers. Ours can be a property of the schema:

```sql
CREATE TRIGGER invalidate_price
  AFTER UPDATE ON products
  FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

**The framing that matters is bounded versus unbounded staleness.**
Application-side invalidation has *unbounded* staleness: when one write path
forgets to invalidate, the cache serves a stale value until somebody notices — an
hour, a week, or until a customer complains. There is no upper bound, because the
bound is "when the bug is found". Trigger-based invalidation is *bounded*: the
eviction is attached to the write itself and applied at commit, so staleness has a
ceiling measured in milliseconds and set by the database rather than by code
review.

That difference is what [demo 2](demos/2-trigger-invalidation/) measures: the same
application, one arm invalidating in application code with one realistically
forgotten path, the other using a trigger, and a stale-serve count under a write
storm. `PENDING — measured figures`

## Do **not** use pg_resp if

Five filters. If any one of them describes you, stop here — the honest answer is
that this is the wrong tool, and finding that out from the README is cheaper than
finding it out in production.

1. **You cannot restart PostgreSQL or grant superuser.** pg_resp loads through
   `shared_preload_libraries`, so installing it is a server-level act requiring a
   restart. There is no way around this and no plan to add one.
2. **You need the cache to survive a restart.** It will not. pg_resp is ephemeral
   by design (D5) — a restart, a crash, or a worker bounce empties it. Everything
   in it must be reconstructible from the database. If you need durability, use
   Redka; that is precisely the product it is.
3. **You need more than strings and TTLs.** No hashes, lists, sets, sorted sets,
   pub/sub, streams, RESP3 or `MULTI`/`EXEC`. Deliberately out of scope for 0.1,
   not overlooked — data structures are where cache scope goes to die.
4. **You need isolation between tenants or roles.** The cache is one cluster-wide
   namespace. Any role that can call `resp.get` can read anything any other role
   cached, and any client that reaches the RESP port can read the whole keyspace.
   There is no per-key or per-role access control.
5. **You need more throughput than one CPU core can serve, or you need the cache
   reachable from other machines.** Command execution is single-threaded and not
   sharded, and there is no TLS in 0.1 — a bind address widened beyond loopback
   sends your password and your values in cleartext.

A sixth, which is a judgement call rather than a disqualifier: consolidating your
cache into your database also consolidates the blast radius. If your cache is
deliberately separate so that losing it cannot touch your primary, that is a
coherent position and pg_resp does not serve it. The reasoning and the failure
modes are in [`docs/ops.md`](docs/ops.md).

## Performance, honestly

The pitch is consolidation and invalidation correctness. It is **not** raw speed,
and any report claiming pg_resp is faster than Redis at raw ops/sec should be
treated as a benchmark bug until proven otherwise — a rule this project applies to
its own results.

| comparison | result |
|---|---|
| vs Redis / Valkey, raw throughput | `PENDING` — they are expected to win; every number is published including the losses |
| vs Redka (architecture class) | `PENDING` — the structural claim, decided by the **weakest** measured cell rather than the best |
| RAM per 1M cached 1 KB entries | `PENDING` |

Method, environment, per-cell raw artifacts and every stated limitation:
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) and
[`bench/results/ENV.md`](bench/results/ENV.md). Every published figure has a
committed raw file and a command that regenerates it.

## Install

`PENDING` — a container image and a `CREATE EXTENSION` quickstart land with the
0.1.0 release. Building from source today:

```bash
cargo install cargo-pgrx --locked --version 0.19.2
cargo pgrx init --pg18 download
cd crates/pg_resp && cargo pgrx install --release
# then add pg_resp.so to shared_preload_libraries and restart PostgreSQL
```

PostgreSQL **16, 17 and 18**, all CI-tested. Configuration reference and memory
sizing: [`docs/ops.md`](docs/ops.md). Command semantics and every documented
divergence from Redis behaviour: [`docs/semantics.md`](docs/semantics.md).

## Status

**0.1.0-rc.** Not yet tagged, not yet announced. Phase 4 of the plan in
[`project-bible.md`](project-bible.md); benchmarking and packaging are what remain.

## Contributing

[`CONTRIBUTING.md`](CONTRIBUTING.md) — please read the first rule before writing
code. Security policy: [`SECURITY.md`](SECURITY.md).
[PostgreSQL License](LICENSE).

Behavioural reference is [Valkey](https://valkey.io) (BSD-3) and the public RESP
specification. Redis source is never consulted; the name contains no "Redis", and
"Redis-protocol-compatible" is nominative use.

---

<details>
<summary>Working on pg_resp with Claude Code</summary>

Method: [`docs/RUNBOOK.md`](docs/RUNBOOK.md). Agent entrypoint and iron rules:
[`CLAUDE.md`](CLAUDE.md).

```bash
cargo test -p resp-proto -p resp-store -p resp-client   # fast loop, no PostgreSQL
cargo pgrx test pg18                                    # slow loop, crosses FFI
make compat                                             # 5-client matrix
make harness-test                                       # benchmark table generator
```

Close every phase with `/phase-report N`, commit, `/clear`.

</details>
