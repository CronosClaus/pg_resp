# Changelog

All notable changes to this project are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Note on version numbers: the **extension** version (`default_version` in
`pg_resp.control`, and the `sql/pg_resp--X.Y.Z.sql` filename) is not the same
surface as the **release** version below. `0.1.0-rc` is a release candidate of
extension version `0.1.0` — see the comment in `pg_resp.control` for why those
are deliberately decoupled. **They converge at the `v0.1.0` tag**, where the
crate version drops its `-rc` and both surfaces read `0.1.0`; the extension
version does not move, so no upgrade script is needed for that transition.

## [Unreleased]

## [0.1.0-rc] — 2026-08-05

First release candidate. A RESP2 cache server running inside a PostgreSQL
background worker: `redis-cli` connects to your database and it answers.

### Added

- **RESP2 wire server** in a background worker, listening on
  `pg_resp.bind_address:pg_resp.port` (default `127.0.0.1:6379`). Inline and
  multibulk framing, per the public RESP specification.
- **Commands.** T0 core: `PING`, `ECHO`, `GET`, `SET` (with `EX`/`PX`/`NX`/`XX`),
  `DEL`, `EXISTS`, `TTL`, `PTTL`, `EXPIRE`, `INCR`, `DECR`, `INCRBY`, `MGET`,
  `MSET`. T1 ops: `AUTH`, `SELECT` (db 0 only), `DBSIZE`, `FLUSHDB`,
  `FLUSHALL`, `INFO` (subset), `SCAN` (cursor, `MATCH`, `COUNT`), `CLIENT`
  (subset), `COMMAND` (stub), `QUIT`. T2: `SETEX`, `SETNX`, `GETDEL`, `GETEX`,
  `PERSIST`, `TYPE`, `RANDOMKEY`, `KEYS`.
- **TTLs, CLOCK-LRU eviction and a hard memory cap** (`pg_resp.max_memory`,
  default 256MB; `pg_resp.eviction`).
- **SQL surface** in schema `resp`: `resp.get`, `resp.set`, `resp.del`,
  `resp.exists`, `resp.ttl`, `resp.keys`, `resp.stats`. Revoked from `PUBLIC` at
  install time (D12); granting is an explicit act.
- **Trigger-based cache invalidation**: `resp.evict` as a row trigger, applied
  after commit, so cache staleness is bounded by the schema rather than by every
  application code path remembering to invalidate.
- **AUTH** via `pg_resp.password`.
- PostgreSQL **16, 17 and 18** supported, all three CI-tested.
- Packaging: `pg_resp.control` + `sql/pg_resp--0.1.0.sql`, and a
  `postgres:18`-based image (`docker/pg_resp.Dockerfile`).
- Docs: `docs/ops.md` (memory sizing, blast radius), `docs/semantics.md`,
  `docs/invalidation.md`, `docs/BENCHMARKS.md`.

### Known limitations

Design decisions, not open defects. Stated here so they are visible before
installation rather than after.

- **Ephemeral by design** (D5). No persistence, ever, in 0.1: a restart or crash
  empties the cache.
- **No per-role isolation.** The cache is one cluster-wide namespace; any role
  that can call `resp.get` can read anything any role cached. See
  [`SECURITY.md`](SECURITY.md).
- **No TLS** (D6). A `bind_address` widened beyond loopback sends the password
  and every value in cleartext.
- **Superuser install, `shared_preload_libraries`, restart required.**
- **Single-threaded command execution** (D4). Not sharded; not faster than
  Redis at raw ops/sec, and not trying to be — see `docs/BENCHMARKS.md`.
- **No hashes, lists, sets, sorted sets, pub/sub, RESP3 or MULTI/EXEC**
  (T3, v0.2 backlog).
- `SCAN` gives the documented guarantee "no misses of stable keys, duplicates
  possible", via a bounded server-side cursor registry rather than true
  reverse-binary iteration (D10).

[Unreleased]: https://github.com/CronosClaus/pg_resp/compare/v0.1.0-rc...HEAD
[0.1.0-rc]: https://github.com/CronosClaus/pg_resp/releases/tag/v0.1.0-rc
