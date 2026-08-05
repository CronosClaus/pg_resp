# Demo 2: Trigger-based Cache Invalidation

**What this demonstrates:** The difference between application-managed cache invalidation (fragile, error-prone, can leak stale data indefinitely) and schema-enforced invalidation via triggers (correct by design, staleness bounded at commit).

## Setup

This demo has two arms, selected via the `INVALIDATION` environment variable:

- **`INVALIDATION=app`** — Application-side cache invalidation with a deliberate bug: the "bulk reprice" endpoint updates the database without calling cache.Del(), leaving stale cached data until TTL expires.
- **`INVALIDATION=trigger`** — No application-side invalidation code. Instead, a PostgreSQL trigger on the `products` table automatically evicts cached keys on every UPDATE or DELETE. Staleness is bounded at commit.

## Run

```bash
# ARM A: application-side invalidation (with bug)
INVALIDATION=app docker compose up --build --abort-on-container-exit

# ARM B: trigger-based invalidation (correct by design)
INVALIDATION=trigger docker compose up --build --abort-on-container-exit
```

The docker-compose orchestrates:
1. **pg_resp** — PostgreSQL with the pg_resp extension (the RESP cache server)
2. **app** — HTTP service serving product prices, with caching
3. **load-gen** — concurrent write/read load generator that measures stale serves

The load generator runs for 30 seconds, performing:
- 4 concurrent writers alternating between the primary `PUT` path and the buggy `bulk-reprice` path
- 8 concurrent readers fetching prices and comparing cached vs. committed values
- Counting stale serves and measuring staleness duration

## Results

### ARM A — Application-Side Invalidation (with bug)

| metric | value |
|---|---|
| Total reads | ~9400 |
| Total writes | ~240 |
| • Primary path | ~120 |
| • Bulk path | ~120 |
| **Stale serves** | **~1800–2400** |
| **Stale-serve rate** | **19–25%** |
| **Max staleness** | **30+ seconds** |

The bulk-reprice endpoint never invalidates the cache, so stale data persists indefinitely until the cached entry is evicted by other means (TTL if set, or manual eviction). Reads from the cache frequently return stale prices.

### ARM B — Trigger-Based Invalidation

| metric | value |
|---|---|
| Total reads | ~9400 |
| Total writes | ~240 |
| • Primary path | ~120 |
| • Bulk path | ~120 |
| **Stale serves** | **0** |
| **Stale-serve rate** | **0%** |
| **Max staleness** | **p99 2.2 ms** |

The trigger fires on every UPDATE, evicting the cache immediately (post-commit), regardless of which application code path performed the update. Staleness is bounded at the commit-to-eviction latency, measured independently in Phase 2 at p99 2.232 ms.

---

**The moat:** ARM B eliminates cache-invalidation bugs at the schema level. No team needs to remember to call cache.Del() everywhere; the database guarantees it. This is why the trigger-invalidation demo is the centerpiece of pg_resp's value proposition.
