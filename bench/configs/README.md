# Benchmark arm configurations (bible §10)

Six arms. The point of committing these files rather than passing flags ad hoc
is that "stock config" and "cache-tuned" are *claims*, and a reader who doubts
them should be able to read exactly what was set.

| arm | server | config file | pinned image |
|---|---|---|---|
| R-def | Redis 8.2.8 | [`redis-default.conf`](redis-default.conf) | `redis:8.2-alpine` |
| R-opt | Redis 8.2.8 | [`redis-cache.conf`](redis-cache.conf) | `redis:8.2-alpine` |
| V-opt | Valkey 8.1.9 | [`valkey-cache.conf`](valkey-cache.conf) | `valkey/valkey:8.1-alpine` |
| K-pg | Redka (pinned clone `d3c353f02470`) | flags, see below | built locally |
| P-def | pg_resp | [`pg-default.conf`](pg-default.conf) | built locally |
| P-opt | pg_resp | [`pg-tuned.conf`](pg-tuned.conf) | built locally |

**Image digests** (recorded per the Phase 4 D15 amendment — a tag is mutable, a
digest is not):

```
redis:8.2-alpine          redis@sha256:a7859ed111db3c1f5404a973a4747505d559fb5ca32d37e447afc0ef845a2103
valkey/valkey:8.1-alpine  valkey/valkey@sha256:a038175878d66b9d274fbf8be73c0305e93798b83917647f167e18cef3c71eec
```

Both report `malloc=jemalloc-5.3.0`, which matters for the RAM-per-entry metric:
all three of Redis, Valkey and pg_resp are measured against jemalloc, so the
allocator is not a hidden variable between them.

## D8 note, stated plainly because it will be asked

Decision **D8** forbids ever opening Redis source — Redis ≥ 7.4 is
RSALv2/SSPL-licensed and this repo is PostgreSQL-licensed. Running the official
published Redis *binary image* to measure its throughput copies no code and
reads no source; it is ordinary benchmarking of a competitor, and bible §10
names Redis as the R-def/R-opt arm precisely so the comparison is against what
people actually run. Valkey remains the behavioural reference for *semantics*
(the differential oracle), and no pg_resp code has ever been derived from Redis
source. Approved as **D15** in the Phase 4 kickoff.

## Parity between arms — the part that decides whether the numbers mean anything

- **Memory cap is equal, not merely present.** `pg_resp.max_memory` is 256 MB on
  the measured instance, so R-opt and V-opt get `maxmemory 256mb` with
  `allkeys-lru`. Giving an incumbent an unbounded cache and pg_resp a 256 MB one
  would make the hit rates incomparable and quietly flatter whichever side had
  more room.
- **Eviction policy is matched in kind.** pg_resp implements CLOCK-LRU
  (`pg_resp.eviction = clock_lru`); `allkeys-lru` is the closest incumbent
  policy. Neither is `volatile-*`, since this workload sets no per-key TTLs.
- **The memtier client configuration is identical on both arms of every
  compared cell** (Phase 4 D14 amendment). The harness takes the workload
  parameters as arguments and does not vary them per arm, so a cell can only
  differ by the server under test.
- **R-def is deliberately *not* tuned.** RDB snapshotting stays on, at Redis's
  own shipped intervals, because that is what an unconfigured `docker run redis`
  does and a large share of real deployments never change it. R-def existing
  alongside R-opt is what keeps the comparison honest in *both* directions: it
  shows the incumbent's out-of-box cost, and R-opt shows its true ceiling.

## K-pg (Redka) is not configured by a file

Redka is a Go server taking flags. It is built from the pinned reference clone
(`ref/redka`, commit `d3c353f02470` per `docs/refs/PINS.md`) and run against the
**same PostgreSQL instance** pg_resp is running in, which is the entire point of
the arm: it isolates architecture (SQL-translation vs in-process) rather than
comparing two different databases.

Redka's own documented PostgreSQL numbers are ~25k GET / ~11k SET ops/sec
(`docs/refs/redka-notes.md`). If the measured arm lands wildly away from that,
suspect the arm before believing the result.

## The RAM-per-1M-entries measurement uses different files

Bible §10's third metric needs ~1.1 GB resident per arm, so it cannot run under
a 256 MB cap — a capped store would evict and the measurement would report the
cap instead of the per-entry cost. That measurement therefore uses raised caps,
documented separately in `bench/results/ENV.md`, and is never mixed into a
throughput table. See also the D17 note there: Redka's cache lives in
PostgreSQL tables on disk, so its RSS is not comparable to an in-memory store's
and is reported as disk bytes plus `shared_buffers` instead.
