---
name: bench-harness
description: Benchmark execution cookbook for pg_resp — the six arms of bible §10, exact memtier_benchmark invocations, environment checklist, result parsing into markdown tables. Consult for any performance measurement; bench-runner subagent follows this verbatim.
model: haiku
---
# Bench harness

Filled Phase 2 (per this run's amendments: bench-harness fills at the start
of Phase 2's official bench work, not before). Sourced from
`docs/refs/memtier_benchmark-notes.md` (full digest of the CLI, already
written in Phase 0/1). This session's `memtier_benchmark` is built from
`/ref/memtier_benchmark` source (a full, non-sparse clone) rather than a
system package — build it once with `autoreconf -ivf && ./configure &&
make` from that directory (needs `libevent-dev`, `libpcre3-dev`,
`libssl-dev`, `zlib1g-dev`, `autoconf`, `automake`, `pkg-config`; check
`/ref/memtier_benchmark/README.md` for the exact list if the build fails on
a missing header).

## 1. The six arms (bible §10)

| arm | config |
|---|---|
| R-def | Redis/Valkey latest, stock config (RDB snapshots on) |
| R-opt | cache-tuned: `save ""`, `appendonly no`, `maxmemory` + `allkeys-lru` |
| V-opt | Valkey, same tuning as R-opt (the license-clean incumbent) |
| K-pg | Redka server on the same PG instance (SQL-translation architecture) |
| P-def | pg_resp on stock `postgresql.conf` |
| P-opt | pg_resp with documented tuning only (`pg_resp.max_memory` sized, nothing exotic) |

Config files for each arm (Redis/Valkey `.conf`, PG tuned/default) live in
`bench/configs/`; raw output and `ENV.md` live in `bench/results/` — both
directories are created by whichever benchmark run is first to need them,
committed after every run, per bible §0.6 ("publish every number, including
losses").

**Phase 2 scope note**: the soak gate (§2 below) only needs the P-def or
P-opt arm (pg_resp against itself, no comparison) — the full six-arm sweep
comparing against R-def/R-opt/V-opt/K-pg is Phase 4's benchmark-report
work, not Phase 2's soak gate. Don't run the other five arms for the soak
gate; that's a different, later deliverable.

## 2. Phase 2 soak procedure (the immediate need)

Bible §5 Phase 2 gate: "memtier 30 min mixed 1:10 write:read at ~50% of max
throughput: RSS plateaus (no leak), zero errors, p99 stable."

1. Start pg_resp against a fresh instance (empty store, known
   `pg_resp.max_memory` if the GUC exists yet — if not, unbounded is fine
   for this gate, since the point is leak detection, not eviction
   correctness, which the property tests already cover separately).
2. Find max throughput first with a short (~30s) saturation run at high
   `--pipeline`/`--clients` to establish a ceiling, then compute ~50% of
   that ops/sec as the target rate for the real soak (memtier doesn't have
   a direct "target ops/sec" throttle flag in all versions — if unavailable,
   approximate by halving `--clients`/`--threads` from the saturation
   config, or use `--rate-limiting` if this memtier build supports it;
   record whichever approach was used in `bench/results/ENV.md`).
3. Real soak invocation (30 min, 1:10 write:read — memtier's `--ratio` is
   SET:GET, and "write:read" in the gate text maps directly to that):
   ```bash
   memtier_benchmark \
     --host=127.0.0.1 --port=6379 \
     --ratio=1:10 \
     --data-size=1024 \
     --key-pattern=G:G --key-maximum=1000000 \
     --test-time=1800 \
     --clients=<half-of-saturation-clients> --threads=<half-of-saturation-threads> \
     --print-percentiles=50,99,99.9
   ```
4. **RSS sampling at 0/5/15/30 min** (bible §5 exact cadence): sample the
   pg_resp bgworker process's RSS in parallel with the memtier run —
   ```bash
   PID=$(pgrep -f 'postgres: pg_resp')
   for t in 0 300 900 1800; do
     sleep $((t - PREV)); PREV=$t
     awk '/VmRSS/{print}' /proc/$PID/status >> bench/results/soak-rss.log
   done
   ```
   (adjust sleep math to actual elapsed time; the point is four samples at
   those four marks, not a busy-loop).
5. **Pass condition**: RSS at 30 min is not meaningfully higher than RSS at
   5-15 min (a plateau, not a climbing line — a single-digit-percent
   difference between the 15 and 30 minute samples is a plateau; a
   still-climbing line is a leak). Zero errors in memtier's summary. p99 at
   the end of the run is not materially worse than p99 in the first few
   minutes (checked via memtier's own histogram output, or by running two
   shorter windows and comparing if a single 30-minute run doesn't expose
   intra-run percentiles).

## 3. memtier flag cookbook (full six-arm matrix, Phase 4 scope)

- `--host=ADDR --port=PORT`: plain TCP, no cluster mode.
- `--ratio=1:10`: SET:GET.
- Value sizes: `--data-size=64` / `1024` / `16384` — **one flag per run**,
  never combined with `--data-size-range`/`--data-size-list` (mutually
  exclusive, may error or behave unpredictably if mixed).
- Pipeline: `--pipeline=1` and a separate run with `--pipeline=16`.
- Connections 1/8/64 — **`--clients` × `--threads` multiply, they are not
  additive.** `--clients=1 --threads=1` (1 total), `--clients=8
  --threads=1` (8 total), `--clients=8 --threads=8` (64 total). This is
  the single most common memtier misconfiguration — always compute the
  product before trusting a number.
- Key space: `--key-maximum=1000000 --key-pattern=G:G` (gaussian access,
  1M keys, default `--key-minimum=0` so the range is `[0, 1000000]`).
- `--test-time=60 --run-count=3` for the 3×60s protocol runs (sequential,
  not parallel — total wall time ≈ 3×60s; use `--print-all-runs` to see all
  three instead of just the median).
- `--print-percentiles=50,99,99.9` — raw percentile values, not fractions
  (`99.9`, never `0.999`).

## 4. Environment checklist

Record every item in `bench/results/ENV.md` for every real (non-soak-only)
benchmark run:
- CPU governor (`performance`, not `powersave`/`ondemand`) —
  `cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor`
- SMT/hyperthreading state (`lscpu | grep Thread`)
- Client placement: second machine, or pinned cores (`taskset`) if
  same-machine — same-machine runs must state this as a limitation, since
  client and server contending for the same cores skews both directions
- Kernel version, memtier_benchmark version/commit (`docs/refs/PINS.md`
  already pins the source commit), pg_resp version/commit
- Exact command line used, verbatim, not paraphrased

## 5. Result parsing → markdown table

memtier's own summary table already reports ops/sec and p50/p99/p99.9 per
command type (Sets, Gets, Totals) — copy those rows directly into a
markdown table per run, one table per arm/workload combination, committed
under `bench/results/`. Don't hand-recompute anything memtier already
computed (`run_stats.cpp`'s own ops/sec math, `(total_ops /
test_duration_usec) × 1,000,000`, is the source of truth).

**RAM-per-1M-entries** (bible §10 metric, memtier does NOT report this —
confirmed in `docs/refs/memtier_benchmark-notes.md`, client-side tool
only): measure server-side, not via memtier.
- pg_resp: `/proc/<bgworker-pid>/status`'s `VmRSS`, before and after
  loading exactly 1M entries of the target value size, delta = the number.
- Redis/Valkey: `INFO memory`'s `used_memory` field, same before/after
  methodology.
- Always report bytes/entry as measured-delta ÷ 1,000,000, not a
  theoretical estimate — this is the same discipline as
  `PER_ENTRY_OVERHEAD_BYTES` in `resp-store` being labeled an estimate
  until a real measurement replaces it (see `reports/phase2-prework.md`
  and `reports/phase2.md`).
