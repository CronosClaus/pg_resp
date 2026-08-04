# Prior-art sweep — Phase 0a

**Date:** 2026-08-04
**Verdict: CLEAR**

No existing project runs an in-process, RESP2-speaking TCP server inside a
PostgreSQL background worker. Every Redis+Postgres combination found on a
fresh sweep falls into one of three categories, none of which is pg_resp's
architecture:

1. **FDW (SQL→Redis direction)** — query Redis *from* Postgres SQL as
   foreign tables. `redis_fdw` (C, PG 9.1+), `redis_fdw_rs` (Rust/pgrx),
   `rw_redis_fdw`, Supabase Wrappers' Redis wrapper. These go the opposite
   direction from pg_resp: Postgres is the client, Redis is still a real
   external server. Not competitors, not architectural precedent for the
   bgworker approach.
2. **RESP→SQL translation servers, standalone process** — external server
   accepts RESP, translates each command to SQL against a relational
   backend. This is bible §2's known Redka entry, plus one previously
   unlisted match:
   - **Redka** (`nalgeon/redka`, Go, SQLite or Postgres backend) — unchanged
     from the bible's characterization. Still the closest neighbor, still
     explicitly "not about raw performance" in its own docs.
   - **NEW: `biozz/postgredis`** (Go, `redcon` + `pgx`) — same category as
     Redka but Postgres-only: creates a two-column `key`/`value` table and
     does upserts. Effectively a minimal Redka clone. **3 stars, 3 commits,
     1 watcher, 0 forks** — inactive toy project, not a real competitive
     signal, but confirms the "SQL-translation" pattern is an obvious idea
     others have poked at and abandoned. Add to §2 table as a footnote
     under Redka, not a new row worth full billing.
3. **Standalone Redis-API-compatible databases, unrelated to Postgres** —
   **NEW: `eloqdata/eloqkv`** — a distributed multi-model database exposing
   a Redis-protocol frontend, backed by its own TxService/LogService/Storage
   Service (RocksDB, Cassandra, or S3-tiered — not Postgres). Recent HN
   thread ("Postgres is reliable – I'll persist in EloqKV", Sept 2025) is
   about durability positioning, not a Postgres integration. Different
   category entirely: it is a database engine, not an extension living
   inside another database. No action needed beyond noting it exists in
   case it comes up in HN comments as "well there's already EloqKV" — the
   answer is "different thing, not Postgres-resident."

No hits for the literal name `pg_resp` — name appears free of direct
GitHub collision (this is not a trademark clearance, just a naming
collision check).

## Addendum — Redka throughput numbers (from Phase 0 ref-digestion, docs/refs/redka-notes.md)

Redka's own `docs/performance.md` (as digested from the pinned clone,
commit d3c353f02470) publishes concrete numbers that sharpen bible §10's
"≥5× K-pg" pre-registered claim:

| backend | GET ops/s | SET ops/s |
|---|---|---|
| SQLite in-memory | ~104k | ~36k |
| SQLite persisted | ~103k | ~26k |
| **PostgreSQL (K-pg arm)** | **~25k** | **~11k** |
| Redis (their control) | ~139k | ~133k |

Redka-on-PG is ~5-6x slower than Redis on GET, ~12x slower on SET, in
Redka's own numbers. This means bible §10's "≥5× K-pg" structural-win gate
is calibrated against a real, source-confirmed baseline, not a guess — and
it's a low bar precisely because SQL-per-op against PG is genuinely that
expensive. Also confirms architecturally why: each Redka SET is two
upsert statements (rkey + rstring tables); each GET is a SELECT with a
join and TTL filter (`internal/rstring/sqlite.go:5-47`). No background
GC for TTL either (manual/lazy only) — a real correctness/ops gap beyond
raw throughput.

## Updated positioning table (delta from bible §2)

| project | category | verdict |
|---|---|---|
| Redka | RESP→SQL, standalone, SQLite/PG | unchanged — closest neighbor, must be addressed first paragraph of README |
| `biozz/postgredis` | RESP→SQL, standalone, PG-only | new, minor — same pattern as Redka, dead/toy-scale, worth a one-line footnote, not a table row |
| EloqKV | standalone Redis-API DB, own storage engine | new, not a competitor to this architecture — different category (not Postgres-resident) |
| redis_fdw family (C, Rust/pgrx, others) | FDW, opposite direction (SQL queries Redis) | unchanged from bible framing — not architectural precedent, not competitors |
| omni_httpd, pg_net, worker_spi | bgworker/socket-in-PG precedent | not re-verified this pass (not competitors, cited for lifecycle precedent only); re-check during ref-digestion when `/ref` clones happen |

## Conclusion

Nothing found forces a pivot or kill. The specific claim — Postgres
background worker holding the cache in-process and speaking RESP2 directly
on the wire, no SQL translation on the hot path — remains unclaimed
territory. Proceed per bible §5 Phase 0 plan.

**Not yet done in this pass** (out of scope for the 0a websearch sweep,
noted so it isn't silently skipped): omni_httpd/pg_net/worker_spi are cited
in the bible for lifecycle precedent, not as competitors — no re-verification
search was run on them since they don't bear on the CLEAR/PIVOT/DEAD
verdict. If Phase 0's ref-digestion turns up anything that changes their
characterization, note it there, not here.
