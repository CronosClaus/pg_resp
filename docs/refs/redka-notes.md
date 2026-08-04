# Redka: RESP-to-SQL Translation & Competitor Positioning

## Focus: RESP command dispatch, SQL-per-op architecture, throughput characteristics, data structure coverage

Redka (Go, maintained/in-maintenance-mode) maps RESP commands to SQL queries against SQLite or PostgreSQL. Architecture: in-process Go API OR standalone Redis-compatible server over RESP wire protocol.

### Command Dispatch & SQL Architecture

- **Dispatch entry**: `redsrv/internal/command/command.go:Parse()` — massive switch statement mapping ~100+ RESP command names to parse/run handlers (lines 20–239)
- **Handler structure**: Each command → `Parse*()` (validates args) → `redis.Cmd` interface → `.Run(conn, context)` executes against `*redka.DB` instance
- **Pipeline**: `redsrv/handlers.go:createHandlers()` chains middleware: logging → parse → multi-transaction support → handle (lines 14–16)
  - Multi-transaction: queues commands, executes batch in single SQL txn on EXEC
  - Single mode: executes immediately with `db.Update()` wrapper

### SQL Per Operation Pattern (Core Claim)

- Schema in `docs/persistence.md`: 6 tables (rkey, rstring, rlist, rset, rhash, rzset) + views
- Per-command overhead verified in `internal/rstring/sqlite.go` (lines 5–47): each operation = upsert chain on rkey (line 17–24) + rstring table (line 26–30)
- Example SET: two INSERT OR CONFLICT statements with version increment on rkey
- Example GET: single SELECT with join on rkey filtered by key + TTL expiry (lines 5–8)
- GET on non-existent key: no result; returns `ErrNotFound` (core.go pattern)

### Throughput & Performance Claims

From `docs/performance.md`:
- **SQLite in-memory**: GET ~104k ops/sec, SET ~36k ops/sec (p50: 63ms, 167ms)
- **SQLite persisted**: GET ~103k ops/sec, SET ~26k ops/sec (slightly lower due to disk sync)
- **PostgreSQL backend**: GET ~25k ops/sec, SET ~11k ops/sec (p50: 359ms, 775ms)
- **Redis baseline**: GET ~139k ops/sec, SET ~133k ops/sec (control)
- **Conclusion from docs**: "tens of thousands of operations per second" but "noticeably slower than Redis"
  - PG overhead ~4–11× on write-heavy (SET), ~5× on read-heavy (GET)
  - Stated reason: relational DB overhead vs specialized KV store

### Data Structure Support & "Tarpit" Indicator

Redka supports 5 Redis data types (from README):
1. **Strings** (basic, binary-safe) — most optimized
2. **Lists** — ordered by insertion (position column in rlist)
3. **Sets** — unordered unique members
4. **Hashes** — field-value maps (field + value blob in rhash)
5. **Sorted sets** (zsets) — scored ordering (elem + score columns in rzset)

**Coverage indicator**: `redsrv/internal/command/command.go` lists ~60 string ops, ~30 list ops, ~25 hash ops, ~20 set ops, ~25 zset ops. Full CRUD for each type.

**Missing**: Advanced ops like APPEND, GETRANGE (strings), stream data structures (not in Redis 5 core). Notably absent: bit operations, HyperLogLog, geospatial.

### Traps & Design Notes

1. **Version field on rkey**: Incremented on every write (even expiry changes). Used to track staleness but adds overhead.
2. **TTL via timestamp**: Expiry stored as unix milliseconds (etime) in rkey. Cleanup is _manual trigger_ (no background gc in stored data); user must call TTL-aware queries or use helper functions. PG/SQLite don't auto-expire.
3. **Dialect abstraction** (sqlx): Queries split into sqlite.go and postgres.go variants. Enum/placeholders differ ($1 vs ?). Adds maintenance burden for multi-backend support.
4. **No atomicity across operations**: Each RESP command is wrapped in `db.Update()` txn (line 99 in handlers.go). Multi-command batches use explicit `db.Update()` with inner loop (lines 100–106). Snapshot isolation per statement, but user sees "fast enough" semantics, not true snapshot-per-RESP-stream.
5. **Maintenance mode**: Author in-process Go API preferred; RESP server may not track Redis 7+ commands.

### File Pointers (Go, function signatures only)

- `redka.go:74–89` — DB struct, Type constants
- `redka.go:98` — `type DB struct`
- `redsrv/handlers.go:14` — `createHandlers(db *redka.DB) redcon.HandlerFunc`
- `redsrv/handlers.go:85` — `handle(db *redka.DB) redcon.HandlerFunc`
- `redsrv/internal/command/command.go:20` — `Parse(args [][]byte) (redis.Cmd, error)` entry point
- `internal/rstring/db.go:78` — `Set(key string, value any) error`
- `internal/rstring/tx.go:37` — `Get(key string) (core.Value, error)`
- `docs/performance.md:10–50` — Benchmark table (GET/SET ops/sec across 3 backends)
- `docs/persistence.md:5–43` — Schema definition
