# Launch post — DRAFT, pending external review (pause 4). Not posted anywhere.

Suggested HN title:

> pg_resp: a Redis-protocol cache server inside a Postgres background worker

---

Most small and mid-sized systems run Redis next to Postgres, and for most of them
Redis is doing four things: cache, rate limiting, sessions, locks. You pay for that
with a second stateful service to run, patch, monitor and page someone about — and
with cache invalidation living in application code, far from the data it caches.

**pg_resp is a Postgres extension that speaks RESP2 on a TCP port.** `redis-cli`
connects to your database and it answers. Not a proxy, not a sidecar, not a layer
that turns every `GET` into a `SELECT` — the cache lives in a background worker's own
heap, in-process.

## Yes, Redka and Valkey exist

[Redka](https://github.com/nalgeon/redka) is the closest neighbour and it is a good
project. It is a Redis-compatible Go server with a Postgres backend, it is durable,
and its author is explicit that raw performance is not the goal. Every operation is a
transaction against indexed tables — measured from inside the same Postgres, a `SET`
of a new key costs **two row inserts and about four index scans**. pg_resp's
equivalent is a hash-map insert. That difference is the entire structural claim, and
it is why the comparison is architectural rather than a tuning result.

Valkey is the incumbent and it is faster than pg_resp. That is stated up front here
and in the README because a benchmark that hides it is worthless.

## The numbers, including the ones we lose

At 1 KB with pipelining and one connection, **Valkey is +19% and Redis +17%** over
pg_resp. Across 24 paired comparisons: pg_resp faster in 8, an incumbent faster in 2,
14 within run-to-run noise — and both incumbents run single-threaded by default here,
while the same Redis given `io-threads 4` reaches **1.79× pg_resp** using 2.8 cores to
our 1.05. The honest summary is: *comparable to a single-threaded Redis at these
payloads, and beaten by a Redis you let use more cores.*

Against Redka-on-Postgres: **≥ 8.1× at worst**, decided by the weakest measured cell
rather than the best, rising to 111.7× under load. Memory: **1,210 bytes per entry vs
1,345 (Redis) and 1,353 (Valkey)** — about 10% leaner, and against an allocator
handicap, since we use glibc malloc and they use jemalloc.

Rate limiting, the least favourable workload we could pick: a tie at **~105,000
checks/s**, because at that concurrency the bottleneck is the network round trip and
not either cache. So the claim is a bound, not a crossover.

## The part that is actually new

```sql
CREATE TRIGGER invalidate_price AFTER UPDATE ON products
  FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

Application-side invalidation has **unbounded** staleness: when one write path
forgets, the cache serves a stale value until somebody notices — an hour, a week,
until a customer complains. The bound is "when the bug is found". Attached to the
write and applied at commit, staleness gets a ceiling instead.

Measured under a write storm, same app, one arm with a realistically forgotten
invalidation path: **493 stale serves were still stale when observation stopped at
10 seconds. The trigger arm: 0.**

## How this was built, and why the failures are public

Developed agent-assisted (Claude Code) against a phased plan with hard gates. The
plan, the phase reports, the benchmark methodology and the failures are all in the
repository.

That last part is not modesty. **Five conclusions I reached during the benchmark
phase were wrong, and every one died to a control experiment rather than to review:**

1. "pg_resp lacks `TCP_NODELAY`, hence a 41 ms stall" — refuted when Redis, which
   sets it, collapsed identically.
2. "The 16 KB reply exceeds the send buffer" — refuted by 16 KB GET-only running at
   46,589 ops/s on a warm store.
3. "16 KB at pipeline 16 is clean" — my own earlier claim, refuted when I noticed the
   probe had run against an empty store, so every GET missed and returned 5 bytes and
   no large reply was ever sent.
4. "The default socket send buffer is the root cause" — refuted when raising it 16×
   did not move the cliff.
5. A rate-limiter correctness check that reported a working limiter as broken,
   because it derived its bound from run duration instead of wall-clock windows.

All five were plausible. All five would have shipped as fact without a control. The
benchmark numbers survived because they were measured; the *explanations* needed a
second opinion every single time — and that asymmetry is the most useful thing I can
tell you about trusting agent-built work.

Two upstream pgrx findings came out of it and are filed:
[#2365](https://github.com/pgcentralfoundation/pgrx/issues/2365) (`#[pg_trigger]`
renders a returned error with `Debug`) and
[#2366](https://github.com/pgcentralfoundation/pgrx/issues/2366)
(`GucSetting::get()`'s main-thread-only check is undocumented). Two more candidates
were investigated and **withdrawn** — one was our own misreading, the other would not
reproduce off a single machine — and both are written up in `docs/upstream/` rather
than quietly dropped.

## Do not use it if

You cannot restart Postgres or grant superuser · you need the cache to survive a
restart (it is ephemeral by design) · you need more than strings and TTLs · you need
isolation between tenants · you need more throughput than one core, or the cache
reachable off-box (there is no TLS in 0.1). It also will not run on RDS or Cloud SQL,
because it needs `shared_preload_libraries`.

## Try it

```bash
docker run -d --name pg_resp -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:6379:6379 -p 127.0.0.1:5432:5432 \
  ghcr.io/cronosclaus/pg_resp:0.1.0 -c pg_resp.bind_address=0.0.0.0

redis-cli -h 127.0.0.1 -p 6379 SET greeting hello
```

Measured on a fresh host: 14 seconds from nothing to a served `GET`.

PostgreSQL License. Postgres 16, 17, 18.
