# pg_resp semantics — every known divergence

This page exists because pg_resp claims Redis-protocol compatibility, and a
compatibility claim is only honest if the exceptions are written down where
users will find them. Bible §6 requires this file; risk register §13 names
"semantics drift from Redis" as a high risk, and this is the mitigation that
does not depend on anyone's memory.

**How the claims here are backed.** Command-level behaviour is checked
continuously against a real Valkey instance by the differential oracle
(`tests/differential/`): 15,000 randomly generated commands across three seeds
plus a 121-command hand-written adversarial deck, byte-for-byte, zero
divergences. Anything below that says "matches Valkey" means that oracle
covers it. Behaviour the oracle *cannot* cover — because Redis has no
equivalent — is marked as such.

Behavioural reference is **Valkey (BSD-3) and the RESP2 spec only**; Redis
source is never consulted (bible `D8`, a licensing constraint).

---

## 1. Scope: what is not implemented

| area | status |
|---|---|
| Strings + TTL + counters (T0/T1/T2) | implemented — see the command list in `README.md` |
| Hashes, lists, sets, sorted sets | **not implemented** (`D9`). A cache does not need `ZADD` in v0.1; data structures are where scope goes to die |
| Pub/sub | not implemented (a `LISTEN`/`NOTIFY` bridge is a logged v0.2 idea) |
| `MULTI`/`EXEC` transactions | not implemented |
| RESP3 / `HELLO 3` | not implemented, deliberately. `HELLO` answers as RESP2; RESP3 sigils are never emitted |
| Persistence (`SAVE`, `BGSAVE`, AOF) | **never**, by design (`D5`). pg_resp is ephemeral: a worker restart is an empty cache. That is correct for a cache and is not a limitation being worked around |
| Replication, clustering | not implemented |
| TLS | not in v0.1. The default bind is loopback; see `docs/ops.md` |
| Multiple databases | `SELECT 0` succeeds, any other index errors. One keyspace per cluster |
| ACL users | no. A single optional password (`pg_resp.password`), matching Redis ≤ 5 semantics |

An unimplemented command returns `-ERR unknown command '<name>'`. It never
hangs and never drops the connection — client libraries probe for optional
commands on connect, and a stall there is worse than a clean refusal.

## 2. Divergences within implemented commands

| behaviour | Redis/Valkey | pg_resp | why |
|---|---|---|---|
| `KEYS` ordering | unspecified | unspecified, and genuinely differs | Hash iteration order is implementation-defined. The differential oracle compares `KEYS` order-independently for exactly this reason; relying on the order is a bug in the caller either way |
| `SCAN` cursor | reverse-binary iteration over the bucket array | snapshot cursors in a bounded server-side registry — 64 live cursors, 60s idle expiry (`D10`) | Rust's `std::HashMap` does not expose the bucket stability that reverse-binary iteration needs. **The documented guarantee is the same as Redis's**: keys present for the whole scan are never missed; duplicates are possible. On registry miss or overflow the scan restarts from cursor 0, which yields duplicates, never misses |
| `RANDOMKEY` distribution | sampled, approximately uniform | approximately uniform, different distribution | Not a contract either implementation makes |
| `INFO` fields | hundreds | a documented subset, plus one field Redis does not have (`invalidations_lost`, §4) | Enough for client health checks and for `resp.stats()`. Fields present are real, not stubbed |
| `TTL` rounding | `(ms + 500) / 1000` | identical | Verified against Valkey's `expire.c`. A key reporting `TTL` 0 in its final half-second is correct, not a bug — 0 does not collide with the -1/-2 sentinels |
| Memory accounting | precise allocator introspection | key bytes + value bytes + a measured 96-byte per-entry constant | Documented in `docs/ops.md`; the constant was measured, not guessed, and its variance is recorded in `reports/phase2.md` |
| `maxmemory` / eviction | many policies | `clock_lru` or `noeviction` (`pg_resp.eviction`) | Approximate LRU by the same philosophy as Redis's sampled LRU. **`noeviction` in v0.1 disables the byte budget entirely** rather than rejecting writes when full — stated plainly because the name suggests otherwise |

## 3. The SQL surface: semantics that have no Redis equivalent

The `resp.*` functions are pg_resp's reason to exist, and their semantics are
deliberately *not* the same as the wire protocol's. Nothing here can be checked
against Valkey, because Redis has no idea Postgres exists.

### 3.1 Reads are immediate; writes wait for commit

| function | when it takes effect |
|---|---|
| `resp.get`, `resp.exists`, `resp.ttl`, `resp.keys`, `resp.stats` | immediately, outside transaction control |
| `resp.set`, `resp.del`, `resp.evict` (trigger) | queued, applied **after the transaction commits** |

Consequences worth stating explicitly, because each one surprises somebody:

- **A transaction does not read its own uncommitted cache writes.**
  `BEGIN; SELECT resp.set('k','new'); SELECT resp.get('k');` returns the *old*
  value. Nothing has been sent yet. This is intended: the alternative is a
  cache that reflects writes which may never commit.
- **Rolling back is complete.** A rolled-back transaction never touches the
  cache. This is the guarantee the whole design exists for, and it is tested,
  including the case that actually proves it — a committed transaction followed
  by a rolled-back one in the same session, which a queue that failed to reset
  per transaction would get wrong.
- **Savepoints and `EXCEPTION` blocks are honoured.** Cache writes queued
  inside a subtransaction that later rolls back are discarded, including the
  implicit subtransaction a plpgsql `EXCEPTION` block creates. If a savepoint
  is released and the *outer* transaction then rolls back, the writes are still
  discarded.
- **Reads are not transactional at all.** Two `resp.get` calls in one
  transaction can return different values if another session wrote in between.
  A cache read is a question about the present, not about a snapshot.

### 3.2 Bounded staleness at commit — stated honestly

Cache writes apply *post*-commit. Postgres marks the transaction committed
(making the new row visible to other transactions) and then runs the callback
that reaches the cache. So there is a real window in which a concurrent reader
can see the new row while the cache still holds the old value.

Measured on the development machine (`bench/results/2026-08-05-staleness.md`,
1000 iterations, deliberately measured as an upper bound):

| p50 | p99 | p99.9 | max |
|---|---|---|---|
| 1.247 ms | 2.232 ms | 5.599 ms | 6.180 ms |

The honest comparison is not "pg_resp is stale for 2 ms". It is: this window is
**bounded and measurable**, whereas application-managed invalidation is stale
for however long it takes someone to notice the code path that forgot to
invalidate. That asymmetry — bounded-at-commit versus
unbounded-and-discipline-dependent — is the entire argument, and demo 2 exists
to measure both halves of it.

### 3.3 A lost invalidation is possible, and counted

If the RESP server is unreachable at the moment the post-commit callback runs,
**that invalidation is lost**. The stale entry survives until its TTL, or
forever if it has none.

This is not a bug that will be fixed by trying harder: the callback runs after
the commit is irrevocable. It cannot fail the transaction, and it must not
raise an error — an error thrown while post-commit callbacks fire aborts the
backend, and can escalate to a cluster-wide restart. Taking the database down
because the cache blinked is not a trade worth making.

What happens instead:
- a `WARNING` is emitted, naming how many invalidations were lost and why;
- `invalidations_lost` is incremented, visible in both `resp.stats()` and
  `INFO`. It lives in Postgres shared memory precisely so it survives the
  condition it records — the failing component is the one that would otherwise
  have to store the count.

**There is no retry queue in v0.1.** If a non-zero `invalidations_lost`
matters to your correctness, the mitigation is a TTL on cached entries, so that
every entry is self-healing on a bounded horizon.

### 3.4 Text, not bytes

The RESP wire protocol is binary-safe; SQL `text` is not. A value containing
invalid UTF-8 or an embedded NUL can be written over the wire and then **cannot
be read through `resp.get()`** — it raises a clear error rather than returning
mangled text. Read such keys over the RESP port. `resp.set()` can only write
valid text by construction.

### 3.5 `resp.evict()` key-column types

The trigger helper derives a cache key from a column, and supports `text`,
`varchar`, `char`, `name`, `smallint`, `integer`, `bigint` and `uuid`. Any
other type raises an error naming the type and the supported set. Rendering
arbitrary types would mean reconstructing Postgres's output-function machinery
inside a per-row trigger path, and the failure modes of getting that wrong are
worse than the inconvenience of adding a generated column.

On `UPDATE`, if the key column itself changed, **both** the old and new keys
are evicted — otherwise a row that changes identity strands the entry cached
under its previous key.

### 3.6 `resp.stats()` is a projection of `INFO`

`resp.stats()` does not maintain its own counters. It issues `INFO` over the
loopback connection and parses it, so the two views cannot drift: there is one
source of truth, not two implementations that happen to agree. The test that
compares them is therefore guarding the *parse*, not guarding against counter
divergence — a distinction worth stating, since the gate's name ("stats
consistency") could be read as a stronger claim than it is.

### 3.7 `resp.keys()` walks the whole keyspace

Backed by `KEYS`, with the same O(N) cost. It is for introspection from psql.
Do not call it from application code or a trigger.

## 4. `INFO` fields pg_resp adds

| field | meaning |
|---|---|
| `pg_resp_version` | the extension version |
| `invalidations_lost` | post-commit cache writes that could not be delivered (§3.3). Has no Redis equivalent because the situation cannot arise there |

`redis_version` is reported as `7.0.0`. This is a compatibility declaration
aimed at client libraries that gate features on a version string, not a claim
to implement Redis 7.0. Everything absent from §1 is absent.
