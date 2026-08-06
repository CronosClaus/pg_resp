# pg_resp

> **DRAFT — PENDING HUMAN REVIEW. Not announced anywhere.**
> Draft for external review (Phase 4 W7/W14). Nothing here is published until
> reviewed.
>
> **Every throughput, latency and memory figure below is measured**, on a dedicated
> box, 3×60s per cell with a spread gate, from committed raw artifacts. Two things
> remain `PENDING` and are deliberately left empty rather than estimated: demo 2's
> stale-serve counts, and the container image that does not exist until release.

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
storm. The stale-serve counts are **not yet measured** — that demo's numbers are
outstanding, and no figure is quoted here until they exist.

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

The pitch is consolidation and invalidation correctness, **not** raw speed. Every
figure below comes from a committed raw artifact on a dedicated box, and the
tables are generated from those artifacts rather than typed here. Method,
environment and every limitation: [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

### Where Redis and Valkey win

At 1 KB values with pipelining and a single connection, both beat pg_resp
outright:

| arm | ops/s | vs pg_resp |
|---|---|---|
| Valkey | 342,198 | **+19%** |
| Redis | 335,933 | **+17%** |
| pg_resp | 287,152 | — |

*1 KB values, pipeline 16, 1 connection, 3×60s, median run.*

### Where pg_resp edges ahead — and immediately loses again

This is the honest exhibit, and it needs all three rows to mean anything:

| arm | ops/s | p99 | CPU used |
|---|---|---|---|
| pg_resp | 1,203,400 | 0.991 ms | 1.05 cores |
| Redis, `io-threads 1` (its default) | 1,095,843 | 1.703 ms | 1.02 cores |
| **Redis, `io-threads 4`** | **2,151,675** | 0.671 ms | 2.80 cores |

*64 B values, pipeline 16, 64 connections, over loopback, 3×60s. Both arms in the
first comparison verified core-saturated. pg_resp authenticates every connection;
the Redis arms do not.*

pg_resp is **~10% faster than a single-threaded Redis** at this payload — and
**1.79× slower than a Redis allowed its I/O threads**, on the same hardware. The
first number is real and narrow; the second is the one that matters if you are
choosing on throughput. Both are published because publishing only the first would
be a lie of omission.

### Memory: ~10% leaner per entry

**pg_resp used 1,210 bytes per entry against Redis's 1,345 and Valkey's 1,353 —
about 10% less — in this measurement.**

*1M entries of 1 KB values, caps raised to 1536 MB for this metric, RSS measured as
a cgroup `memory.current` delta (an upper bound including page cache, applied
identically to all three arms).* The result runs **against** an allocator handicap
rather than with it: pg_resp uses glibc malloc while both incumbents use jemalloc
5.3.0, which is generally stronger at this allocation pattern.

### vs Redka: the structural comparison

**At worst 8.1×, and that is the number to argue with.** G3 asks for ≥5× and is
decided by the *weakest* measured cell, not the average: at one connection with no
pipelining, where both arms are bound by network round trips, pg_resp is 8.1×
Redka-on-PostgreSQL. Every one of the 12 paired cells clears the bar, and the
strongest reaches 111.7× (64 B, pipeline 16, 64 connections — where Redka's p99 is
157 ms against pg_resp's 0.9 ms).

The gap is architectural, not a tuning result: Redka's throughput sits at
**~10,000 ops/s regardless of payload size**, which is what a per-operation cost
looks like rather than a per-byte one. Each cache write is a transaction against
indexed tables — ~2 row inserts and ~4 index scans per `SET`, counted from inside
the same PostgreSQL. Redka's PostgreSQL is deliberately configured in *its* favour
for these runs.

One caveat that runs in our favour and is therefore stated here rather than in a
footnote: Redka is unbounded where pg_resp evicts at its cap, so Redka usually had
the *higher* hit rate — and a hit returns the value where a miss returns 5 bytes.
That inflates the ratio. Per-cell hit rates are published.

### Rate limiting: below ~105k checks/s, the second service buys you nothing

| arm | checks/s | p50 | p99 |
|---|---|---|---|
| Redis | 106,036 | 72 µs | 117 µs |
| pg_resp | 104,936 | 74 µs | 101 µs |

*`INCR` + conditional `EXPIRE`, 8 concurrent clients, unpipelined, 30 s, limiter
enforcement verified.* A tie — and a tie because at this concurrency the workload
is bound by the network round trip, not by either cache. So the honest claim is a
**bound, not a crossover**: up to at least ~105,000 rate-limit checks per second,
the two answers are indistinguishable. Above that a crossover certainly exists —
see the `io-threads` row above for where Redis's headroom comes from.

### What is not measured

**16 KB values are absent from the tables, and that is a limitation of our
benchmark client, not of pg_resp.** The pinned `memtier_benchmark` collapses by
three orders of magnitude at exactly a 16,384-byte payload; `redis-benchmark`
against the same server shows no such effect:

| data-size | redis-benchmark | memtier_benchmark |
|---|---|---|
| 16,340 B | 20,000 req/s | 21,476 ops/s |
| **16,384 B** | **19,355 req/s** | **24.5 ops/s** |
| 32,768 B | 16,216 req/s | — |

**Read that table narrowly, because we did too.** The cliff **did not reproduce**
with a clean build of the same pinned memtier on a second machine against stock
Redis — so this is a client-side artifact **of that specific environment**, not a
general memtier defect. The evidence that it was client-side *there*: all four
servers (pg_resp, Redis, Valkey, Redka) collapsed identically on that box, while
`redis-benchmark` against the same box did not. If you run memtier at 16 KB on your
own machine you will most likely see no cliff — that does not contradict anything
here. Full account, including the failed reproduction:
[`ENV.md`](bench/results/ENV.md) §22 and §25, and
[`docs/upstream/04-memtier-16k-boundary-NOT-REPRODUCED.md`](docs/upstream/04-memtier-16k-boundary-NOT-REPRODUCED.md).

pg_resp serves 16 KB and 32 KB values fine. Every ranked cell in this document
comes from one pinned client, so rather than mix clients in one table we publish
the gap and its proof.

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

Developed agent-assisted (Claude Code) against a phased plan with hard gates. The
full plan, the phase reports, the benchmark methodology and the failures are public
in this repository — including the measurements that were withdrawn and the
conclusions that were wrong. Method: [`docs/RUNBOOK.md`](docs/RUNBOOK.md).

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
