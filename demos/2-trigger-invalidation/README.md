# Demo 2 — cache invalidation as a schema property

Two arms of the same app under an identical write storm. Arm A invalidates the
cache in application code and has one realistic bug: a second write path, added
later, that forgets. Arm B has **no invalidation code at all** — a trigger on
the table does it:

```sql
CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

## Run it

```bash
cd demos/2-trigger-invalidation

for arm in app trigger; do
  docker compose down -v
  INVALIDATION=$arm docker compose up -d --build pg_resp setup app
  INVALIDATION=$arm docker compose run --rm load-gen
done
```

(Don't use `up --abort-on-container-exit`: the one-shot `setup` container exits
by design and would tear the stack down before the load generator runs.)

## Measured results

30-second run, 4 writers split across both write paths, 8 readers. Real numbers
from a real run, not estimates.

| | Arm A: app-side | Arm B: `resp.evict` trigger |
|---|---|---|
| reads | 705 | 20,300 |
| writes (primary / bulk) | 1,623 (787 / 836) | 7,868 (3,936 / 3,932) |
| stale serves | 621 (**88.09%**) | 1,183 (**5.83%**) |
| **still stale after 10s** | **504 of 621** | **0 of 1,183** |
| staleness p50 | ≥ 10s (floor) | **2.54 ms** |
| staleness p99 | ≥ 10s (floor) | **398.7 ms** |
| max staleness | ≥ 10.3s | **642 ms** |

**The row that matters is "still stale after 10s".** Arm A's staleness is
*unbounded*: 504 of its 621 stale reads were still serving the wrong price when
the harness stopped watching, so its p50 and p99 are right-censored floors, not
measurements — the true durations are unknown and longer. Nothing in Arm A ever
fixes those keys; only another write through the *remembering* path would.

Arm B's staleness always resolved — zero censored samples — with a p50 of
2.54 ms that matches the independently measured commit→eviction latency
(p99 2.232 ms, `bench/results/2026-08-05-staleness.md`).

## Why Arm B is not 0%

Because cache-aside has a race that has nothing to do with pg_resp, and
pretending otherwise would make this demo dishonest. A reader that misses can
read the database, and then write that already-superseded value back into the
cache *after* the trigger's eviction has landed. The trigger did its job; the
application then re-populated the key with a stale read. This happens
identically with Redis, and it is why Arm B's tail (p99 398 ms) is longer than
the eviction latency itself — the re-populated value survives until the next
write evicts it again.

The claim being demonstrated is therefore not "zero staleness". It is
**bounded versus unbounded**: every stale read in Arm B corrected itself, and
most in Arm A never did.

## Arm A's read count is lower, and why

705 vs 20,300. Both arms run the same code; the difference is that measuring
staleness requires polling a key until it converges, and in Arm A most keys
never converge, so 621 probe goroutines run their full 10-second observation
window and compete with the readers. Treat the two **percentages** as
directional rather than precisely comparable; the censoring row is the finding
that does not depend on the denominator.

## Notes

- The app connects as a **non-superuser** (`demo_user`), and Arm B's trigger is
  created *as that role*, so the demo exercises the documented grant recipe
  (`GRANT USAGE ON SCHEMA resp` + `GRANT EXECUTE ON FUNCTION resp.evict()`) end
  to end rather than leaning on superuser privileges. See `docs/ops.md`.
- `pg_resp.bind_address=0.0.0.0` is a compose-only override so sibling
  containers can reach the port. It is not a deployment recommendation — the
  real default is loopback.
- Arm B's init SQL verifies its own trigger exists and raises if not, because an
  arm that silently measures nothing is worse than one that fails.
