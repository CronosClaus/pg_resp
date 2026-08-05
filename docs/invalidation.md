# Cache invalidation as a schema property

There are two hard problems in caching. The first — where to put the bytes —
Redis solved decades ago. The second is knowing when a cached value stopped
being true, and that one is still open, because the answer normally lives in
application code, far from the data it describes.

pg_resp moves it into the schema.

```sql
CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

That is the whole feature. From then on, any statement that changes a row in
`products` — from your application, from a migration, from a colleague in psql
at 2am, from an `ON CONFLICT DO UPDATE` you forgot existed — evicts
`product:<id>` when, and only when, the transaction commits.

## Why the usual approach leaks

Application-managed invalidation is a *discipline*: every code path that writes
must remember to also evict. That works until the second write path exists.

```go
// The path everyone remembers.
func UpdatePrice(id int, price float64) error {
    db.Exec("UPDATE products SET price=$1 WHERE id=$2", price, id)
    cache.Del(fmt.Sprintf("product:%d", id))   // ✅
    return nil
}

// The path added six months later, by someone in a hurry, for a bulk
// repricing job. It is not obviously wrong. It is wrong.
func ApplyDiscount(pct float64) error {
    db.Exec("UPDATE products SET price = price * $1", 1-pct)
    // ...and nothing here. ❌
    return nil
}
```

Nothing fails. No error is logged. The cache serves confidently wrong prices
until a human notices, and the staleness is unbounded — the entry is not
refreshed by time, only by another write that happens to go through the
remembering path.

This is not a straw man; it is the single most common cache bug in production
systems, and the reason is structural: **the database knows the row changed,
and the cache is the one component that cannot find out.** Redis architecturally
cannot fix this. It has no idea your database exists.

## What the trigger changes

Invalidation stops being something code must remember and becomes something the
schema enforces. The properties that follow are worth being precise about:

**It cannot be bypassed.** The trigger is attached to the table, not to a code
path. A migration, an admin's manual `UPDATE`, a `DELETE` cascading from
another table, an ORM doing something unexpected — all of them fire it.

**It is transactional.** Cache writes are queued and applied in a post-commit
callback. A transaction that rolls back never touches the cache, so a failed
write cannot leave the cache reflecting a state the database never reached.
This includes savepoints and plpgsql `EXCEPTION` blocks: work queued in a
subtransaction that rolls back is discarded with it.

**Its staleness is bounded, and measured.** Postgres marks a transaction
committed and *then* runs the callback, so there is a window in which a reader
can see the new row while the cache still holds the old value. On the
development machine, over 1000 iterations, measured as a deliberate upper bound:

| p50 | p99 | p99.9 | max |
|---|---|---|---|
| 1.247 ms | 2.232 ms | 5.599 ms | 6.180 ms |

Full method and raw data: `bench/results/2026-08-05-staleness.md`. In 999 of
1000 iterations the eviction had already landed before the committing client
was even told its commit succeeded.

**The comparison that matters** is therefore not "2 ms of staleness versus 0".
It is:

| | application-managed | `resp.evict` trigger |
|---|---|---|
| staleness on the remembered path | ~0 | ~1–2 ms |
| staleness on the forgotten path | **unbounded** — until someone notices | ~1–2 ms |
| bypassed by a manual `UPDATE` | yes | no |
| survives a rolled-back transaction | depends on where the `Del` sits | never applied |
| requires code review discipline | permanently | no |

Trading a couple of milliseconds on the happy path for the elimination of an
entire failure class is the trade this feature offers. Demo 2
(`demos/2-trigger-invalidation/`) measures both columns of that table with a
real write storm against both write paths.

## Setting it up

### 1. Cache reads, as normal

Nothing about your read path changes. Point your existing Redis client at
Postgres's `pg_resp` port and use it exactly as before.

### 2. Attach the trigger

```sql
CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

The first argument is the key prefix, the second the column holding the rest of
the key. The key `resp.evict` builds is `prefix || column_value::text`, so it
must match whatever your application uses when it caches the row.

Cover `INSERT` too if a cached read can produce a negative result (a cached
"this product does not exist"), otherwise a newly inserted row stays invisible:

```sql
CREATE TRIGGER products_cache_evict
AFTER INSERT OR UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

### 3. Grant what a non-superuser needs

`resp.*` is revoked from `PUBLIC` by default (`D12`). The exact grants — which
were determined by testing, not assumption — are in
[`ops.md`](ops.md#granting-access-to-the-resp-functions). The half everyone
forgets is `GRANT USAGE ON SCHEMA resp`, whose absence reports as
`permission denied for schema resp` and does not obviously point at the fix.

One consequence worth knowing: `EXECUTE` on `resp.evict()` is checked **when
the trigger is created**, not each time it fires. Revoking it later does not
disarm triggers that already exist. Drop the trigger to disable it.

## When this is the wrong tool

- **Cache keys not derivable from a single column.** `resp.evict()` builds
  `prefix || column::text`. A composite or computed key needs a generated
  column holding the key text, or a hand-written trigger calling `resp.del()`.
- **One row invalidating many keys.** A price change that should evict a
  category listing, a search index, and a homepage fragment needs a custom
  trigger with several `resp.del()` calls. `resp.evict()` handles the
  one-row-one-key case, which is the common one, not every case.
- **Write-heavy tables where most rows are never cached.** The trigger fires
  per row regardless, costing about 0.12 ms at p50 on the committing
  transaction. On a table taking millions of writes whose rows are rarely read,
  that is wasted work — cache with a TTL instead.
- **Correctness that cannot tolerate a lost invalidation.** If the cache is
  unreachable at commit time the eviction is lost, counted in
  `invalidations_lost`, and not retried. Set a TTL so every entry is
  self-healing on a bounded horizon. See
  [`semantics.md` §3.3](semantics.md).

## The bit that is easy to miss

This works because the cache is *inside* the database. There is no network hop
between the transaction committing and the cache learning about it, no queue to
drain, no separate service to be down, and no second thing to deploy. The
trigger is not a clever integration between two systems; there is only one
system.
