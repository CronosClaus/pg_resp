# PG_RESP — Project Bible

**Codename:** pg_resp
**One line:** A Postgres extension that runs a Redis-protocol (RESP2) cache server inside a Postgres background worker — `redis-cli` connects to your database and it answers. Delete one container.
**Form factor:** in-process. Not a proxy, not a sidecar, not a translation layer to SQL.
**Status:** unbuilt. Phase 0 not run.
**Owner:** human. **Builder:** Claude Code.

---

## 0. AGENT CONTRACT — read before writing any code

This project is built in **phases with hard gates**. If a gate number fails, **stop and write the phase report**. Do not tune the metric until it passes. Do not continue to the next phase.

1. **Do not write code before Phase 0's spikes and prior-art sweep are green.** Phase 0 exists to kill the project cheaply if it must die.
2. **Every phase ends with `reports/phase<n>.md`** containing raw gate numbers, what broke, and open decisions. The report is the handoff artifact: the next phase's session starts by reading the bible + the previous report, **not** by re-reading the whole codebase or re-deriving decisions.
3. **Architectural decisions are logged in §12 as `D<n>`.** If you need a new one, add it, mark reversibility, and flag it in the report. Never silently change a locked decision.
4. **Never copy code from Redis.** Redis ≥ 7.4 is RSALv2/SSPL/AGPL-licensed. Behavioral reference is **Valkey** (BSD-3) and the public RESP spec only. This is a legal constraint, not a style preference. (`D8`)
5. **Honesty rule for benchmarks:** publish every number, including the ones where Redis wins. The pitch is consolidation and correctness, not raw speed. A dishonest benchmark kills this project's entire purpose (credibility).
6. **PG FFI is confined to the main bgworker thread.** No PostgreSQL function may be called from any other thread, ever. This is the rule that keeps the database alive. (`D4`)
7. **Determinism and reproducibility:** pinned toolchain versions, pinned reference-clone commits, seeded tests, benchmark environment recorded in every report.
8. **Token discipline:** never grep or read the `/ref` clones wholesale. Use the digest files in `docs/refs/` (§9). If a digest is missing, create it once, then use it.

**What this project is:** an ops-consolidation and cache-correctness play with a viral demo.
**What this project is not:** faster than Redis at raw ops/sec. If a report claims that, treat it as a benchmark bug.

---

## 1. Thesis

**The pairing being attacked:** Redis is the most common service deployed next to Postgres. Its dominant uses in small/mid systems are cache, rate limiting, sessions, locks. For these, teams pay for: a second stateful service, a second failure mode, a second monitoring stack, and an unsolved problem — **cache invalidation lives in application code, far from the data it caches.**

**The existing "replacement" story is weak.** The recurring blog pattern is UNLOGGED tables + pg_cron deletes for TTL — a table pretending to be a cache, with a full SQL parse/plan/execute cycle per GET and an app rewrite required.

**The claim:** a background worker inside Postgres can speak RESP2 on a TCP port, backed by an in-memory store, giving:

1. **Zero app rewrite** — change the Redis port/host, done. Every Redis client library works.
2. **One process tree** — one deploy unit, one systemd unit, one Docker container, one thing to monitor.
3. **The moat: trigger-driven invalidation.** `UPDATE products SET price=…` fires a trigger that evicts `product:42` at the source of truth, inside the same system. Redis architecturally cannot have this; the DB doesn't know Redis exists. This converts cache invalidation from an application-discipline problem into a database-schema property.
4. **SQL visibility** — `SELECT * FROM resp_stats()`, cache keys inspectable from psql.

**Why this can be fast enough:** the RESP hot path never touches the SQL layer. Parse RESP (µs), hash lookup in process memory (ns), write response. No parser, no planner, no executor, no MVCC, no WAL. The store is a plain in-memory structure owned by the worker.

**Latency envelope (localhost, unpipelined):**

| path | per-op cost structure | expected p50 |
|---|---|---|
| Redis GET | event loop + hashtable | ~40–80 µs RTT |
| pg_resp GET | event loop + hashtable (same shape) | ~50–120 µs RTT |
| Redka-on-PG GET | RESP → SQL → parse/plan/execute → row | ~300–1000 µs |
| PG `SELECT` from UNLOGGED cache table | full SQL path + client protocol | ~200–800 µs |

The architecture is Redis's architecture, relocated. The gap to Redis should be a constant factor from a less-optimized event loop and allocator, not a structural order of magnitude. The gap to the SQL-translation approaches **is** structural — that's the defensible benchmark (§10 gate).

---

## 2. Prior art and positioning — know this cold before writing the README

| project | what it is | why pg_resp is different |
|---|---|---|
| **Redka** (nalgeon) | Redis-compatible **external Go server**, SQLite **or Postgres** backend, RESP wire protocol, ~1.5k+ stars | Closest neighbor. Every Redka op is a SQL query against PG tables → durable but structurally slow (their own README: "not about raw performance", tens of K ops/s). pg_resp is in-process, RAM-backed, ephemeral-by-design, and can do trigger invalidation with zero network hops. **HN will name Redka within 10 comments — the README must position against it upfront, respectfully.** |
| **pgmq / SKIP LOCKED queues** | queues in PG | different primitive (queue vs KV cache); cite as sibling in "just use Postgres" family |
| **UNLOGGED table + pg_cron** | blog-pattern cache | the strawman pg_resp replaces; full SQL path per op, no eviction, app rewrite required |
| **omni_httpd (Omnigres)** | HTTP server inside PG | proves the pattern: TCP server in a PG extension is viable and shipped |
| **pg_net (Supabase)** | bgworker doing async outbound HTTP | proves bgworker+sockets is accepted in production at scale |
| **worker_spi (PG contrib test module)** | canonical bgworker skeleton | the reference for lifecycle/signals, not a competitor |
| **Valkey / Redis** | the incumbent | not competed with on speed; competed with on ops count and invalidation correctness |

**Phase 0 must re-verify this table** with a fresh GitHub/HN sweep (search: "postgres extension redis protocol", "RESP background worker postgres", "pg_redis", "redis inside postgres"). The table above is believed complete as of authoring; it is not proof. If an in-process RESP-in-PG extension already exists, the project pivots or dies — decide then, cheaply.

**Naming/legal:** name contains no "redis". Tagline may say "Redis-protocol-compatible" (nominative use). License for this repo: **PostgreSQL License** (`D7`) — the maximum-trust choice for the PG community and the "no skepticism" signal the project wants.

---

## 3. Architecture

### 3.1 Language and framework — `D1`

**Rust via pgrx** (current: pgrx 0.18.x, supports PG 13–18; PG 18 needs the `pg18` feature flag and `cargo pgrx init` against it).

Why Rust and not C, given "PG-native, no skepticism" (§8): a **network-facing protocol parser inside the database** is exactly where memory safety is a selling point, not a foreign-ness. Precedent is established: pgvectorscale, ParadeDB pg_search, pg_mooncake are pgrx extensions taken seriously by the community. Skepticism is managed by following PG conventions everywhere else (§8), not by writing C.

Note (affects `D2`): pgrx **removed** its safe `heapless`-in-shared-memory support in 0.16 for unsoundness. Safe fixed-size shared structures exist (`PgLwLock`-guarded statics), but a dynamic hash in PG shared memory means raw FFI to DSA/dshash. This is a v0.2 adventure, not a v0.1 requirement.

### 3.2 Process model

```
postmaster
├── backends (normal SQL connections)
└── pg_resp bgworker            ← registered via shared_preload_libraries
    ├── main thread: PG lifecycle only (startup, GUCs, SIGTERM latch, shutdown)
    └── server thread(s): mio/epoll event loop
        ├── TCP listener (default 127.0.0.1:6379, GUC-configurable)
        ├── RESP2 parser (own crate, fuzzable in isolation)
        └── store: single-threaded owner of Store struct
```

- **Store lives in bgworker-local heap for v0.1** (`D2`). Plain Rust: `HashMap<Box<[u8]>, Entry>`, TTL via a timing-wheel or expiry-heap, approximate LRU via CLOCK on a memory budget. No PG shared memory on the hot path → no LWLock discipline, no unsound shmem, no postmaster risk from the store itself.
- **Crash/restart semantics:** bgworker restart = empty cache. That is correct behavior for a cache and is documented loudly ("ephemeral by design; it's a cache, not a database" — `D5`).
- **Single-threaded command execution** (`D4`), like Redis itself. One event-loop thread owns the store; no locks on the hot path. Acceptor and loop may be the same thread in v0.1. Multi-shard (thread-per-core, keyspace-sharded) is a v0.2 possibility, logged not built.
- **Shutdown:** postmaster SIGTERM → main thread sets an atomic + wakes the loop via a self-pipe/eventfd → loop drains, closes, exits. Clean shutdown is a Phase 1 gate, not a nice-to-have — a bgworker that ignores SIGTERM blocks `pg_ctl stop` and will (rightly) get the extension labeled dangerous.

### 3.3 SQL surface — `D3`

Two access paths with **different, documented semantics**:

1. **RESP port** — Redis semantics: immediate, non-transactional. This is what apps use.
2. **SQL functions** — `resp.get(key)`, `resp.set(key, val, ttl)`, `resp.del(key)`, `resp.stats()`, `resp.keys(pattern)` for introspection.

v0.1 implementation of the SQL path: a tiny **loopback RESP client** inside the backend (connect to 127.0.0.1:port per session, pooled per backend). Cost ~50–150 µs per call — fine for triggers and introspection, and it makes the SQL surface trivially consistent with the wire surface.

**Transactional write semantics (the part to get right):** SQL-initiated mutations (`resp.set/del` and the invalidation trigger helper) do **not** fire immediately. They enqueue in a transaction-local list and apply in a **post-commit callback** (`RegisterXactCallback`/pgrx equivalent). Rolled-back transactions therefore never touch the cache. Residual honesty: apply is *post*-commit, so there is a microseconds-to-milliseconds window where a committed new value coexists with a stale cached one. Document it as **bounded staleness at commit**, versus the *unbounded, code-discipline-dependent* staleness of app-managed Redis invalidation. This asymmetry is the demo-app-2 story.

Invalidation helper shipped as SQL:

```sql
-- generic trigger: evicts one key derived from the row
CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

### 3.4 Command set

| tier | commands | phase |
|---|---|---|
| T0 core | PING, ECHO, GET, SET (incl. EX/PX/NX/XX args), DEL, EXISTS, TTL, PTTL, EXPIRE, INCR, DECR, INCRBY, MGET, MSET | 1 |
| T1 ops | AUTH, SELECT (accept db 0 only), DBSIZE, FLUSHDB, FLUSHALL, INFO (subset), SCAN (cursor, MATCH, COUNT), CLIENT (subset), COMMAND (stub), QUIT | 2 |
| T2 nice | SETEX, SETNX, GETDEL, GETEX, PERSIST, TYPE, RANDOMKEY, KEYS (with docs warning) | 2 |
| T3 deferred | hashes, lists, sets, zsets, pub/sub (could map to LISTEN/NOTIFY — cute, later), RESP3, MULTI/EXEC | v0.2+ backlog, logged only |

Rationale: T0+T1 covers the overwhelming majority of *cache* usage and everything memtier_benchmark and the three demo apps need. Data structures are where scope goes to die; Redka spent most of its effort there. A cache does not need ZADD in v0.1.

**Unknown command behavior:** proper RESP error `-ERR unknown command`, never a hang, never a disconnect. Client libraries probe with COMMAND/INFO/CLIENT on connect — Phase 1's compat matrix exists to catch exactly these handshake landmines.

### 3.5 Memory management

- GUC `pg_resp.max_memory` (default 256MB). Accounting: key bytes + value bytes + fixed per-entry overhead constant (measured in Phase 2, documented).
- At budget: approximate-LRU eviction (CLOCK bit per entry, sampled sweep — same philosophy as Redis's sampled LRU, simpler implementation), GUC `pg_resp.eviction = clock_lru | noeviction`.
- TTL: lazy expiry on read + active sweep from the event loop timer (N keys per tick, Redis-style), so memory is actually reclaimed without a read.
- This memory is **outside** shared_buffers and outside PG's accounting — say so in docs, recommend sizing guidance (`max_memory` counts against the same RAM budget as shared_buffers when capacity planning).

### 3.6 Security posture

- Default bind 127.0.0.1. `pg_resp.bind_address` GUC to widen, with a red-letter docs warning.
- `pg_resp.password` GUC → RESP `AUTH` (constant-time compare). Empty = no auth (localhost default makes this Redis-parity).
- No TLS in v0.1 (documented; localhost default is the mitigating control). No multi-tenancy: one store per cluster, `SELECT`-able by any role granted the SQL functions — cache data is cluster-visible, documented.
- The extension requires `shared_preload_libraries` and superuser to install — standard for bgworker extensions; docs state it plainly.

---

## 4. Reference corpus — clone these before Phase 0 spikes

All under `/ref`, **gitignored**, pinned to exact commits recorded in `docs/refs/PINS.md`. Sparse-checkout where noted; the point is grounding, not bulk.

| repo | pin scope (sparse) | why |
|---|---|---|
| `postgres/postgres` (branch REL_18_STABLE) | `src/test/modules/worker_spi/`, `src/backend/postmaster/bgworker.c`, `src/include/postmaster/bgworker.h`, `src/backend/storage/ipc/{latch.c,shm_mq.c}`, `src/include/miscadmin.h`, `contrib/pg_stat_statements/` | canonical bgworker lifecycle + signals; contrib module as the gold standard for extension structure/docs/tests |
| `pgcentralfoundation/pgrx` | `pgrx-examples/bgworker/`, `pgrx-examples/shmem/`, `pgrx/src/bgworkers.rs` | the exact APIs v0.1 lives on |
| `valkey-io/valkey` | `src/networking.c`, `src/t_string.c`, `src/expire.c`, `tests/unit/type/string.tcl` | behavioral reference (BSD-3-safe), incl. edge cases: SET arg combos, TTL semantics, INCR on non-integer |
| `nalgeon/redka` | full (small) | competitor study: command semantics choices, README/positioning style, benchmark framing |
| `omnigres/omnigres` | `extensions/omni_httpd/` | prior art: socket server inside PG, lifecycle choices |
| `supabase/pg_net` | full (small) | bgworker + network + GUCs precedent, packaging style |
| `RedisLabs/memtier_benchmark` | docs + src skim | the benchmark tool's exact knobs (tool is GPLv2 — used, never linked) |
| RESP2 spec | vendored as `docs/refs/resp2-spec.md` | the protocol ground truth; parser tests derive from it |

**Digest rule (token discipline):** for each repo, Phase 0 produces a one-time digest in `docs/refs/<name>-notes.md` — the ~50–150 lines of *actually relevant* facts (function signatures, lifecycle order, gotchas) with file/line pointers. All later phases read digests, not trees. A digest is updated only when a gate failure traces back to a wrong/missing fact.

---

## 5. Phases and gates

### Phase 0 — kill-switch: prior art + two spikes · **3–4 days**

**0a. Prior-art sweep** (half a day): fresh search per §2; write `reports/prior-art.md` with a verdict: `CLEAR | PIVOT | DEAD`. Anything but CLEAR stops here for a human decision.

**0b. Spike S1 — socket lifecycle** (1–1.5 days): minimal pgrx bgworker that binds a TCP port, answers hardcoded `+PONG\r\n` to anything, and — the actual test — **shuts down cleanly**. Gate:

| check | pass condition |
|---|---|
| `redis-cli PING` | returns PONG |
| `pg_ctl stop -m fast` | postmaster exits < 2 s, no orphan process, clean log |
| `pg_ctl restart` ×20 loop | zero failures, port rebinds every time |
| crash the worker (`kill -9`) | postmaster restarts it per `bgw_restart_time`; PG itself unaffected |

**0c. Spike S2 — loopback + commit hook** (1 day): SQL function performs a loopback RESP call; a `RegisterXactCallback` fires a queued op **only** on commit, provably not on rollback (test both). Gate: both behaviors demonstrated in a pgrx test.

**0d. Toolchain matrix pin:** spikes compile and pass on PG 16, 17, 18 (pgrx features pg16/pg17/pg18), Linux amd64. Record versions in `docs/refs/PINS.md`.

**Deliverable:** `reports/phase0.md`. If S1's shutdown row cannot be made green, the project's foundation is bad — stop and rethink, do not push on.

### Phase 1 — RESP2 core · **~1.5 weeks**

Build: `resp-proto` crate (parser/serializer, zero PG deps, fuzzable standalone) + event loop + store + T0 commands.

| gate | pass condition |
|---|---|
| protocol fuzz | `cargo-fuzz` on parser, ≥ 1 CPU-hour, zero crashes/UB |
| client compat matrix | redis-cli, redis-py, node-redis, go-redis, jedis: connect + T0 script green (Dockerized, in CI) |
| Valkey differential test | same random T0 command stream → pg_resp and Valkey → identical responses (modulo INFO/errors whitelist) |
| latency | localhost GET 64B, unpipelined, p50 ≤ 150 µs, p99 ≤ 1 ms |
| lifecycle regression | Phase 0 S1 table still green with full server |

The **differential test against Valkey** is the highest-leverage testing idea in this project: it converts "is our SET NX EX behavior right?" from a reading-comprehension problem into an automated oracle. Build it early, run it always.

### Phase 2 — ops-grade: TTL, eviction, memory cap, T1/T2 · **~1.5 weeks**

| gate | pass condition |
|---|---|
| soak | memtier 30 min mixed 1:10 write:read at ~50% of max throughput: RSS plateaus (no leak), zero errors, p99 stable |
| eviction correctness | property tests (proptest): never exceeds max_memory + one entry; TTL'd keys never returned after expiry; monotonic INCR under churn |
| memory honesty | measured bytes/entry overhead documented; `resp.stats()` reports keys, hits, misses, evictions, used_bytes, matches reality ±5% |
| SCAN | cursor-correct under concurrent writes (guarantee documented = same as Redis: no misses of stable keys, dups possible) |

### Phase 3 — the moat: SQL surface + trigger invalidation · **~1 week**

`resp.*` functions, commit-queue semantics, `resp.evict()` trigger helper, docs page "Cache invalidation as a schema property".

| gate | pass condition |
|---|---|
| rollback safety | txn with resp.set → ROLLBACK → key absent; → COMMIT → key present (automated) |
| staleness bound | measured commit→eviction latency histogram published; p99 < 5 ms on dev box |
| demo app 2 | works end-to-end (§11) |
| stats consistency | `resp.stats()` returns numbers identical to `INFO`'s stats/memory section (keys/hits/misses/evictions/used_bytes) — Phase 2's "memory honesty" gate was satisfied via the RESP-level `INFO` command rather than building `resp.stats()` early (its real dependency, the loopback-RESP SQL surface, is this phase's subject); this gate confirms the two views of the same underlying counters never diverge once both exist |

### Phase 4 — benchmark, package, publish · **~1–1.5 weeks**

Benchmark protocol §10 executed; README with positioning table (§2), honest numbers, architecture diagram; packaging: `CREATE EXTENSION` UX, Dockerfile (`pg_resp` on postgres:18 image, one `docker run` to demo), deb via pgxman/trunk if cheap, PGXN listing; CI badge matrix; `v0.1.0` tag; launch post draft ("Your Postgres is now also your Redis") with the *first paragraph acknowledging Redka and Valkey* — preempt the obvious comment.

| gate | pass condition |
|---|---|
| quickstart | fresh machine → running demo in ≤ 3 commands, ≤ 5 min |
| benchmark report | all arms of §10, environment pinned, raw data committed |
| structural win | ≥ 5× Redka-on-PG throughput at equal p99 on the cache workload |
| honesty check | README states where Redis/Valkey wins, with numbers |

### v0.2 backlog (logged, not planned)

DSA/dshash true shared-memory store (direct SQL reads without loopback), thread-per-core sharding, pub/sub↔LISTEN/NOTIFY bridge, RESP3, MULTI/EXEC, hashes/lists, metrics via pg_stat-style view, TLS, **demo 1 (API cache — cut from §11 in Phase 4)**, **jemalloc as the Rust `global_allocator` behind a feature flag** (pg_resp currently uses glibc malloc while Redis/Valkey use jemalloc 5.3.0 — an unmatched variable in the §10 RAM metric, documented in `bench/results/ENV.md` §10; deliberately **not** changed during Phase 4, since swapping the allocator mid-benchmark-phase would invalidate both the 96-byte accounting constant and every number measured against it), **`bytea` variant of `resp.get`**, **additional `resp.evict` key-column types**, **a tighter `PER_ENTRY_OVERHEAD_BYTES` measurement**.

---

## 6. Repository layout

```
pg_resp/
├── README.md                     # positioning table up top; honest bench numbers
├── LICENSE                       # PostgreSQL License
├── CONTRIBUTING.md · CODE_OF_CONDUCT.md · SECURITY.md
├── CLAUDE.md                     # agent entrypoint (§7)
├── project-bible.md              # this file
├── Cargo.toml                    # workspace
├── crates/
│   ├── resp-proto/               # parser/serializer; no PG deps; fuzz targets in fuzz/
│   ├── resp-store/               # store, TTL wheel, CLOCK-LRU; no PG deps; proptest here
│   └── pg_resp/                  # the pgrx extension: bgworker, loop, GUCs, SQL fns
├── sql/                          # pg_resp--0.1.0.sql (+ future upgrade scripts)
├── pg_resp.control
├── tests/
│   ├── compat/                   # dockerized client matrix (py/node/go/java/cli)
│   ├── differential/             # random-stream oracle vs Valkey
│   └── lifecycle/                # start/stop/restart/kill harness (Phase 0 S1, kept forever)
├── bench/
│   ├── harness/                  # memtier invocations, env pinning, result parser → md tables
│   ├── configs/                  # redis-default.conf, redis-cache.conf, valkey.conf, pg tuned/default
│   └── results/                  # raw outputs, committed
├── demos/
│   ├── 1-api-cache/              # §11.1
│   ├── 2-trigger-invalidation/   # §11.2
│   └── 3-rate-limiter/           # §11.3
├── docs/
│   ├── refs/                     # PINS.md + per-repo digests + resp2-spec.md
│   ├── semantics.md              # every divergence from Redis behavior, exhaustively
│   └── ops.md                    # memory sizing, security posture, GUC reference
├── .claude/skills/               # §7 skills
├── .github/workflows/ci.yml      # matrix: PG {16,17,18} × {amd64,arm64}; fmt+clippy+deny
└── reports/                      # phase0.md … phase4.md, prior-art.md
```

Crate split is deliberate: `resp-proto` and `resp-store` compile and test **without a Postgres**, which makes the fuzzing, property tests, and most iteration loops seconds-fast and cheap in tokens (no `cargo pgrx test` cycle for logic work).

---

## 7. Claude Code operating manual

### CLAUDE.md (repo root) must contain
- Build/test one-liners: `cargo test -p resp-proto -p resp-store` (fast loop), `cargo pgrx test pg18` (slow loop), `make compat`, `make bench-smoke`.
- The two iron rules restated: PG FFI main-thread-only; never open `/ref` trees without going through digests.
- Pointer map: "protocol question → skill:resp-protocol; pgrx/lifecycle question → skill:pgrx-patterns; naming/error/docs style → skill:pg-conventions; benchmark task → skill:bench-harness."
- Current phase number + link to last report.

### In-repo skills (`.claude/skills/`) — write these during Phase 0, they pay rent forever

| skill | contents | saves |
|---|---|---|
| `resp-protocol` | RESP2 framing rules, inline commands, error taxonomy, 30 canonical byte-level test vectors, SET-option decision table | re-reading the spec every session; wrong-by-memory framing bugs |
| `pgrx-patterns` | bgworker registration/signals/latch recipes with exact pgrx 0.18 API names; GUC registration; SPI-free rules; xact-callback pattern from S2; "what not to call off-main-thread" list | the single biggest token sink in pgrx work is API-shape rediscovery |
| `pg-conventions` | error message style (Postgres error-style guide distilled), GUC naming, control-file fields, versioned SQL script rules, docs tone; checklist reviewers run against contrib-quality extensions | skepticism-proofing every public surface |
| `bench-harness` | memtier flag cookbook, the six arms of §10, result-parsing regexes, environment checklist (governor, turbo, SMT, cgroup) | benchmark reruns become mechanical, not re-reasoned |

### Context and reasoning discipline
- **One phase = one (or few) session(s).** Session boot ritual: read bible §0 + current-phase section + previous report + relevant skill. Nothing else by default.
- **Reports are the memory.** Decisions, gate numbers, and open threads go in the report at session end — the next session must be able to proceed *without* replaying this session.
- Unit-of-work rule: prefer many small red→green loops on the PG-free crates; touch `cargo pgrx test` only when the change actually crosses the FFI line.
- Subagent pattern for bulk/noisy work (compat-matrix debugging across 5 clients, bench sweeps): dispatch with a narrow brief + the relevant skill, return a table, keep raw logs out of the main context.
- When a gate fails twice for the same cause: stop patching, write the failure into the report, re-read the relevant digest/skill, and fix the model of the problem, not the symptom.

---

## 8. "PG-native, zero skepticism" checklist

The extension must be **boring to a Postgres reviewer**:

- `pg_resp.control` + versioned `sql/pg_resp--X.Y.Z.sql` + upgrade scripts from day one; extension is `CREATE EXTENSION pg_resp` / `DROP EXTENSION` clean.
- All GUCs under `pg_resp.` prefix, registered with units/ranges/docs; `SHOW ALL` looks native.
- Error messages follow the PG style guide (primary message lowercase-no-period, errdetail/errhint where useful) — via pgrx's ereport equivalents.
- SQL objects in schema `resp`, no pollution of `public`.
- Docs structured like a contrib module page: overview → configuration → functions → caveats.
- Supported versions policy stated: current PG and two back (16/17/18 at launch), CI-enforced.
- `SECURITY.md` with a real contact; the bind-address and superuser-install caveats stated without spin.
- Semver + CHANGELOG (keep-a-changelog format); DCO sign-offs; rustfmt+clippy+cargo-deny green in CI.
- No unsafe without a `// SAFETY:` comment; FFI concentrated in one module, auditable in one sitting.

---

## 9. Testing strategy (summary of what the phases enforce)

| layer | tool | lives in |
|---|---|---|
| protocol correctness | unit vectors + cargo-fuzz | resp-proto |
| store invariants (TTL/LRU/memory) | proptest property tests | resp-store |
| semantics vs incumbent | **differential oracle vs Valkey** | tests/differential |
| real-client reality | dockerized 5-client matrix | tests/compat |
| lifecycle safety | start/stop/kill harness from S1, in CI forever | tests/lifecycle |
| SQL surface + txn semantics | `cargo pgrx test` regression suite | crates/pg_resp |
| endurance | 30-min memtier soak + RSS watch | bench/harness |
| performance | §10 protocol | bench/ |

---

## 10. Benchmark protocol — the credibility artifact

**Arms (six):**

| arm | config |
|---|---|
| R-def | Redis/Valkey latest, stock config (RDB snapshots on — what most people actually run) |
| R-opt | cache-tuned: `save ""`, `appendonly no`, `maxmemory` + `allkeys-lru`, io-threads default |
| V-opt | Valkey, same tuning (the license-clean incumbent) |
| K-pg | Redka server on the same PG instance (the SQL-translation architecture) |
| P-def | pg_resp on stock `postgresql.conf` |
| P-opt | pg_resp with documented tuning only (max_memory sized, nothing exotic) |

**Workloads (memtier_benchmark):** ratio 1:10 SET:GET; value sizes 64 B / 1 KB / 16 KB; pipeline 1 and 16; connections 1 / 8 / 64; key space 1M, gaussian access; 3 runs × 60 s, medians reported, raw output committed.

**Metrics:** ops/s, p50/p99/p999, and **bytes of RAM per 1M cached 1 KB entries** (the ops-cost dimension where architecture differences show).

**Environment:** same machine, cpu governor `performance`, SMT state recorded, benchmark client on second machine or pinned cores, everything in `bench/results/ENV.md`.

**Expected honest shape (pre-registered so nobody is surprised):** R-opt/V-opt win raw throughput, likely 2–6×. P-* must land within the same order of magnitude and **≥ 5× K-pg** (the structural claim). The headline chart is therefore *three* stories: raw ops (we lose, say so), architecture class (we win, structurally), and the invalidation-latency demo (only we can even draw it).

---

## 11. Demo apps — one comparison, three different meanings

| # | app | stack | what the comparison *means* |
|---|---|---|---|
| 1 | ~~**API cache** — user-profile endpoint, cache-aside~~ **CUT in Phase 4 → v0.2 backlog** | Go or Node, docker-compose A: app+PG+Redis (3 containers) vs B: app+PG/pg_resp (2) | **ops consolidation**: identical client code (port change only), end-to-end p99 within noise because app overhead dominates; the diff that matters is the compose file shrinking. **Cut because its stated meaning is already carried by two cheaper artifacts**: the README's §2 positioning table shows the container-count argument directly, and the Phase 4 G1 quickstart gate measures the "one `docker run`" claim end to end. Building a third app to re-demonstrate "the compose file is shorter" was the lowest-value item in the phase. Decided at the Phase 4 kickoff, logged in `reports/phase4.md` |
| 2 | **price catalog with trigger invalidation** | same app; arm A does app-side invalidation with a deliberately realistic bug (one code path forgets); arm B uses `resp.evict` trigger | **correctness as schema property**: measure *stale-serve count* under a write storm — A serves stale reads until the bug is found, B's staleness is bounded at commit+p99<5ms. This is the moat demo and the launch-post centerpiece |
| 3 | **rate limiter** — INCR+EXPIRE per API key, synthetic flood | tiny load generator | **honesty**: raw ceiling where Redis wins; publish the crossover analysis ("if you need > X ops/s of pure rate-limiting, keep Redis — below it, the second service isn't buying you anything") |

Each demo ships as a `docker compose up` with a README of ≤ 1 screen.

---

## 12. Decision log

| id | decision | reversible | rationale |
|---|---|---|---|
| D1 | Rust + pgrx (0.18.x), PG 16–18 | costly | memory safety on a network parser; established precedent; C fallback only if pgrx hits a wall in Phase 0 |
| D2 | v0.1 store = bgworker-local heap, not PG shared memory | yes (v0.2) | pgrx safe-shmem limits post-0.16; avoids LWLock/DSA FFI risk on the critical path; cache semantics tolerate restart-loss |
| D3 | SQL surface via loopback RESP + post-commit apply queue | yes | consistency with wire path for free; commit-safety without shmem; latency fine for triggers |
| D4 | single-threaded command execution; PG FFI main-thread-only | partially (sharding later) | Redis-proven model; deletes the whole locking problem class in v0.1 |
| D5 | ephemeral by design — no persistence, ever, in v0.1 | yes | it is a cache; persistence is where scope and danger live |
| D6 | default bind 127.0.0.1; AUTH via GUC; no TLS v0.1 | yes | safe default, Redis-parity posture, documented |
| D7 | PostgreSQL License | no | the trust signal; aligns with §8's whole purpose |
| D8 | Valkey (BSD-3) is the behavioral reference; Redis source is never opened | no | RSAL/SSPL/AGPL contamination risk is existential for D7 |
| D9 | T0/T1 command scope only in v0.1; no data structures | yes | cache coverage per effort is maximal; structures are Redka's tarpit |
| D10 | SCAN cursor state lives in a bounded, server-side registry (~64 live cursors, 60s idle expiry) keyed by cursor id, not per-connection — snapshot cursors, not true Redis reverse-binary iteration over a resizing hash table | yes (true reverse-binary iteration later, if a real workload needs it) | Rust's `std::HashMap` doesn't expose the bucket-stability a reverse-binary cursor needs. A per-connection snapshot was the first design, rejected before being built: pooled clients (e.g. redis-py `scan_iter` over a connection pool) send successive `SCAN` calls on different physical connections, so connection-scoped cursor state breaks them intermittently. On registry miss/overflow, restart the scan from cursor 0 — dups allowed by the documented guarantee ("no misses of stable keys, dups possible"), misses never |
| D11 | SQL surface's loopback client is its own PG-free crate, `crates/resp-client` (a fourth crate, extending bible §6's three-crate layout) | yes | Keeps `resp-proto` a pure parser/serializer whose fuzz targets stay honestly "fuzzable in isolation", and puts every socket edge case (half-arrived replies, a peer that vanished between statements, read timeouts) in a crate testable against a real `TcpListener` in seconds without a Postgres. Its retry rule is the substance: retry once, on a fresh connection, only if the connection was already established **and** no reply byte had arrived — a stale fd usually reveals itself on the *read*, since the write lands in a socket buffer and succeeds. That leaves one narrow double-apply window, so the method is documented safe for idempotent commands only, with `command_no_retry` for the rest |
| D12 | `resp.*` is revoked from `PUBLIC` at install time; granting is an explicit, documented act | yes | Cache contents are a single cluster-wide namespace with no per-role isolation (§3.6), so anyone who can call `resp.get` can read anything any other role cached — default-open is the wrong posture for an extension asking a PG reviewer for trust. Two facts established by experiment, not assumption, and pinned by `#[pg_test]`: `CREATE TRIGGER ... resp.evict(...)` needs **both** `USAGE ON SCHEMA resp` and `EXECUTE`, and that `EXECUTE` is checked **at CREATE TRIGGER time only** — revoking it later does not disarm an existing trigger, so dropping the trigger is the way to disable it |
| D13 | `invalidations_lost` lives in PG shared memory (one `PgAtomic<AtomicU64>`), unlike every other counter | yes | It is incremented by *backends*, at commit time, precisely when the bgworker's store was unreachable — so keeping it in the store would only record failures at moments when the failing component works, and keeping it backend-local would lose it at session end. **D2 is not weakened**: the *store* stays bgworker-local heap; this is a single `u64`, which is what pgrx's shmem support is genuinely safe at. The server thread reads it via a `&'static AtomicU64` captured on the main thread before the server thread spawns, making the read an atomic load through inherited shared memory rather than a PG call, so `D4`/iron-rule-1 holds |

---

## 13. Risk register

| risk | severity | mitigation / kill criterion |
|---|---|---|
| prior art already exists in-process | fatal | Phase 0a sweep; PIVOT/DEAD verdicts are cheap there |
| bgworker + sockets fights PG lifecycle (shutdown hangs, port leaks) | fatal | Spike S1 *is* this risk, tested to a table; if not green in 1.5 days of honest effort, stop |
| client-library handshakes reject a partial server | high | compat matrix in CI from Phase 1; stub COMMAND/INFO/CLIENT deliberately |
| semantics drift from Redis (SET flags, TTL edges) | high | Valkey differential oracle; docs/semantics.md lists every known divergence |
| memory leak / RSS creep in long-running worker | high | Phase 2 soak gate; jemalloc stats in resp.stats() |
| benchmark called dishonest on HN | high | §10 pre-registered shape, raw data committed, losses stated in README |
| Redka comparison lands as "already exists" | medium | README positions in first screen: in-process vs SQL-translation, with the ≥5× number |
| managed-PG unavailability caps adoption | accepted | stated in README; audience is self-host/Docker; irrelevant to portfolio/virality goals |
| trademark/name trouble | low | no "redis" in name; nominative compatibility claim only |
| scope creep into data structures / persistence | medium | D5, D9; backlog is where those ideas go to wait |
| **RESP command-dispatch panic kills the whole service, silently and permanently, undetectably from SQL** | high | Empirically confirmed Phase 2 (deliberately triggered, removed after testing): pgrx mandates `BGWORKER_SHMEM_ACCESS`, so an *external* SIGKILL of the worker forces PG's own crash-recovery cycle (loud, self-healing — every client drops, but postmaster relaunches everything automatically). An *in-process Rust panic* is a **different, worse** failure: `std::thread::spawn` catches it at the OS-thread boundary, so the process never crashes, but D4's single-threaded model means that one thread owned the listener *and every connection* — losing it means every connection resets, no new connection is ever accepted again, yet `SELECT 1` and the bgworker's own OS process stay completely healthy throughout. No automatic recovery; a monitoring system watching "is Postgres up" sees green the whole time. **Mitigation implemented**: `catch_unwind` panic fences at both the per-connection dispatch call (primary — contains the damage to one connection) and the server thread's top-level closure (secondary, defense in depth) in `pg_resp/src/lib.rs`; verified by re-running the deliberate-panic test after the fix — only the triggering connection was lost, a bystander connection and a new connection both kept working. `release` profile's `panic = "unwind"` (root `Cargo.toml`) is required for `catch_unwind` to work at all and is confirmed set. Full detail: `.claude/skills/pgrx-patterns/SKILL.md` §8.8, `docs/ops.md`'s blast-radius note, `reports/phase2.md` |

---

## 14. Honest odds

| outcome | probability |
|---|---|
| Phase 0 fully green (foundation viable) | 80% |
| v0.1 shipped matching all Phase 1–3 gates | 65% |
| structural benchmark win (≥5× Redka-on-PG) holds | 75% (conditional on shipping) |
| HN front page on launch | 25–30% |
| ≥ 1k GitHub stars in 3 months | 15% |
| sustained third-party production adoption | 10% |
| portfolio value (systems story you can whiteboard end-to-end) | ~certain — this accrues even at Phase 1 |

**Time to a demo-able PING from redis-cli: ~4 days. Time to the viral screenshot (T0 + real clients): ~2.5 weeks. Time to launch: ~6–7 weeks part-time.** Most of the *learning* value lands by Phase 1; most of the *credibility* value is Phase 3–4 — the trigger-invalidation demo is what separates this from a toy.
