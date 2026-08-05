# Benchmark environment and the Phase 2 soak record

**Written:** 2026-08-05, during the Phase 3 PRE-STEP.
**Why it exists:** the `bench-harness` skill §4 requires the exact command
line, verbatim, for every benchmark run. The Phase 2 soak was run without
this file, and its raw output (`2026-08-05-soak.txt`) does not echo its own
command line. So the soak's invocation had to be *reconstructed* from
evidence, and the "~50% of max throughput" figure its report claimed had to
be *measured for the first time* here, retroactively. Both are labelled
accordingly below: nothing in this file is presented as verbatim when it is
reconstructed.

---

## 1. Environment

| item | value |
|---|---|
| CPU | Intel Core i7-10750H @ 2.60GHz, 6 cores / 12 logical |
| SMT | 2 threads per core, **enabled** |
| CPU governor | **unavailable** — `/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor` does not exist under WSL2. The skill's §4 checklist asks for `performance`; it cannot be set or even read here. Recorded as a limitation rather than omitted. |
| Kernel | Linux 6.18.33.2-microsoft-standard-WSL2 |
| Virtualization | WSL2. Hypervisor overhead affects both latency and throughput; these numbers are not comparable to bare metal. |
| Client placement | **same machine as the server, unpinned.** Client and server contend for the same 12 logical CPUs. This skews in both directions (client steals server CPU; server steals client CPU) and is a stated limitation, not a controlled variable. |
| PostgreSQL | 18.4 (`~/.pgrx/data-18`, pgrx-managed) |
| pg_resp commit | `0eb1057` |
| memtier_benchmark | commit `272eeb6` (matches `docs/refs/PINS.md`), `v=255.255.255`, libevent 2.1.12-stable, OpenSSL 3.0.2 |

### pg_resp GUCs live on the measured instance

| GUC | value |
|---|---|
| `pg_resp.port` | 6379 |
| `pg_resp.max_memory` | 256MB |
| `pg_resp.eviction` | clock_lru |
| `pg_resp.password` | **set** (`secret123`) |

That last row is not a detail — see §4. Any memtier run against this
instance **must** pass `--authenticate`, or it measures nothing but the
speed of refusing commands.

---

## 2. memtier's progress line: "N conns" means `--clients`, not total

Needed because the soak's progress lines are the only surviving record of its
concurrency. Determined empirically with two short runs whose flags were
known:

```
--threads=4 --clients=2  →  "4 threads  2 conns"
--threads=2 --clients=4  →  "2 threads  4 conns"
```

So the printed "conns" echoes `--clients` (which memtier defines as clients
*per thread*), and total connections = `--threads × --clients`. This matches
the `bench-harness` skill §3 warning that the two flags multiply.

**Applied to the soak's `4 threads  8 conns`:** `--threads=4 --clients=8`,
i.e. **32 total connections**.

---

## 3. The Phase 2 soak invocation — RECONSTRUCTED

Best reconstruction, per-flag provenance stated:

```bash
memtier_benchmark \
  --host=127.0.0.1 --port=6379 \
  --ratio=1:10 \
  --data-size=1024 \
  --key-pattern=G:G --key-maximum=1000000 \
  --test-time=1800 \
  --threads=4 --clients=8 --pipeline=16 \
  --print-percentiles=50,99,99.9
```

| flag | value | provenance |
|---|---|---|
| `--threads` | 4 | **MEASURED** — progress line, per §2 |
| `--clients` | 8 | **MEASURED** — progress line, per §2 |
| `--test-time` | 1800 | **MEASURED** — run duration in the log |
| `--print-percentiles` | 50,99,99.9 | **MEASURED** — those three percentiles appear in the output |
| `--data-size` | 1024 | **DERIVED** — Sets 13891.74 KB/sec ÷ 13269.80 ops/sec = 1072 bytes/SET, which is 1024 bytes of value plus ~48 bytes of key and protocol framing |
| `--pipeline` | 16 | **DERIVED** — see the arithmetic below |
| `--ratio` | 1:10 | **ASSUMED** from the skill §2 template. Corroborated: measured Sets:Gets is 13269.80 : 132697.94 = 1 : 10.000 exactly |
| `--key-pattern` | G:G | **ASSUMED** from the skill §2 template — no independent evidence in the output |
| `--key-maximum` | 1000000 | **ASSUMED** from the skill §2 template — no independent evidence in the output |
| `--authenticate` | unknown | **UNRESOLVED** — the soak served real 1KB hits (55.8 MB/sec of GET traffic, 50564 hits/sec), so it was *not* being refused with NOAUTH; either the password GUC was unset at the time or the run authenticated. Not reconstructible from the output. |

### Why `--pipeline=16`

In-flight requests = throughput × latency (Little's law), and per-connection
in-flight is bounded by the pipeline depth:

- soak: 145,967.74 ops/sec × 0.00379343 s = 553.6 in flight ÷ 32 conns = **17.3 per connection**

17.3 is just above 16, and the excess is expected: memtier's reported average
latency is per-operation including queueing, which slightly understates the
depth needed to sustain a given rate. The reading is confirmed by reproducing
it at a known pipeline on the same config (§4): `--pipeline=16` at
`--threads=4 --clients=8` gives 217,317.85 ops/sec at 2.35186 ms →
511.1 in flight ÷ 32 = **15.97 per connection**, i.e. the arithmetic recovers
the nominal 16 almost exactly when the pipeline value is known. Both runs are
consistent with pipeline 16; the soak's lower throughput is explained by its
higher latency (3.79 ms vs 2.35 ms), not by a shallower pipeline.

An earlier attempt to derive this flag by matching *throughput* against a
sweep of pipeline values concluded ~5–6. That conclusion is withdrawn: the
sweep it relied on was invalid (§4), and matching throughput across runs with
different hit rates is unsound in any case, since a hit returns 1KB and a
miss returns 5 bytes.

---

## 4. Max throughput — MEASURED (retroactively), and a voided first attempt

### The voided attempt, recorded because the failure mode will recur

The first saturation measurement produced 176,490 and 158,519 ops/sec at
"0% hit rate". Those numbers are **void**. The instance has
`pg_resp.password` set and the runs did not pass `--authenticate`, so every
single command was answered `-NOAUTH Authentication required.` — the runs
measured the throughput of *rejecting* commands, never executing one. The
0% hit rate was the symptom, not a workload property: no GET ever ran.

The tell was in the output itself, 5.3 million lines of
`server 127.0.0.1:6379 handle error response: -NOAUTH Authentication
required.`, which is also why those raw files came to 820 MB and were deleted
rather than committed. **A memtier run against an auth-enabled pg_resp
silently measures rejection throughput and inflates its output by ~1000×.**
Check for `NOAUTH` in the output before trusting any number, and check
`SHOW pg_resp.password` before starting. Added to
`docs/refs/memtier_benchmark-notes.md`, which had no entry for
`--authenticate` at all — that gap is what allowed this.

### The valid measurement

All runs below: `--authenticate=secret123`, 15 s, `--ratio=1:10
--data-size=1024 --key-pattern=G:G --key-maximum=1000000`, against a store
already warm and at its 256MB budget. Hit rate landed at ~37% throughout,
which matters: it is within a point of the soak's own 38%, so these are
comparable to the soak rather than to a different workload.

| threads × clients | total conns | pipeline | ops/sec | p50 | p99 | p99.9 |
|---|---|---|---|---|---|---|
| 4 × 8 | 32 | 16 | 198,356 | 2.479 | 4.063 | 7.359 |
| 4 × 8 | 32 | 16 | 217,318 | 2.287 | 3.823 | 7.039 |
| 8 × 8 | 64 | 16 | 226,908 | 4.415 | 7.423 | 13.055 |
| 6 × 16 | 96 | 16 | 230,757 | 6.527 | 10.239 | 18.687 |
| 2 × 4 | 8 | 64 | 264,297 | 1.871 | 3.471 | 6.623 |
| 4 × 8 | 32 | 32 | 289,573 | 3.423 | 5.887 | 8.895 |
| 4 × 8 | 32 | 64 | 366,500 | 5.439 | 9.727 | 13.631 |
| 4 × 8 | 32 | **128** | **397,398** | 10.047 | 15.103 | 22.399 |

Two observations worth keeping:

- **Adding connections barely helps; adding pipeline depth does.** 32→96
  connections at pipeline 16 buys 6% (217k→231k) while degrading p50 nearly
  3×. Pipeline 16→128 on the same 32 connections buys 83%. That is the
  expected shape for a single-threaded event loop (D4): the bottleneck is
  round-trips, not connection count.
- **Run-to-run variance is ~10%** on the identical 4×8/p16 config (198k vs
  217k, the first two rows). Same-machine unpinned client on WSL2; treat any
  single figure here as ±10%.

**Measured max: 397,398 ops/sec**, at pipeline 128 where p50 has degraded to
10 ms — a saturation point, not a usable operating point.

### What that makes the soak

| basis | max | soak as % |
|---|---|---|
| measured saturation ceiling (4×8, p128) | 397,398 | **36.7%** |
| ceiling at the soak's own pipeline depth (4×8, p16) | 217,318 | **67.2%** |

The soak ran at 145,968 ops/sec. So "~50% of max throughput" (bible §5's
Phase 2 gate text) brackets the truth rather than stating it: the soak sat
between 37% and 67% of max depending on whether "max" means the machine's
ceiling or the ceiling at the concurrency the soak itself chose. It was
never measured at the time — the figure in `reports/phase2.md` was an
assertion, not a measurement, and the honest version is this range. See the
amendment appended to that report.

---

## 5. Raw output committed here

| file | what |
|---|---|
| `2026-08-05-soak.txt` | the original 30-minute soak, untouched |
| `soak-rss.log` | RSS at t=0/5/15/30 min: 291908 / 291924 / 291312 / 291312 kB |
| `2026-08-05-sat-4x8-p16-first.txt` | first valid AUTH'd run (the 198,356 row) |
| `2026-08-05-sat-{4x8,8x8,6x16,2x4}-p{16,32,64,128}.txt` | the sweep in §4 |

The three voided NOAUTH runs are **not** committed — 820 MB of a single
repeated error line has no evidentiary value that this file's §4 does not
already carry.

---

# Phase 4 benchmark methodology

Added during Phase 4, per the kickoff amendments. Everything above this line is
the Phase 2/3 record and is unchanged.

## 6. K-pg verified PG-backed via table growth

**This is a G3-validity requirement, not a formality.** Redka picks its driver
from the DSN's scheme (`cmd/redka/main.go`: `strings.HasPrefix(path,
"postgres://")`) and falls back to **in-memory SQLite** — `config.Path =
cmp.Or(flag.Arg(0), os.Getenv("REDKA_DB_URL"), sqliteMemoryURI)` — whenever the
positional argument is missing or empty. A typo in the DSN would therefore
produce a *silently SQLite-backed* arm that still answers every RESP command
correctly, and the structural-win claim would be measured against the wrong
architecture entirely. Redka's own docs put SQLite in-memory at ~104k GET /
~36k SET against ~25k / ~11k for PostgreSQL, so the mistake would be worth
roughly 4x in Redka's favour and would look like a *harder* comparison rather
than an invalid one.

So the arm is verified by watching PostgreSQL, not by trusting redka's log:

```
tables redka created (public schema):  rhash rkey rlist rset rstring rzset
rkey / rstring row counts BEFORE:      0 | 0
SET burst:                             10,000 keys, errors: 0, replies: 10000
rkey / rstring row counts AFTER:       10000 | 10000
value read back out of PostgreSQL:     kpgprobe:4242 | val4242
pg_stat_user_tables (rstring):         n_tup_ins=10000  idx_scan=10001
```

**K-pg verified PG-backed via table growth.** The last line is also the clearest
single piece of evidence for the positioning claim in bible §2: 10,000 RESP SETs
produced 10,000 row inserts and 10,001 index scans inside PostgreSQL. That is
the SQL-per-operation architecture, measured rather than asserted.

Reproduce:

```bash
docker network create kpgnet
docker run -d --name kpg_pg --network kpgnet \
  -e POSTGRES_PASSWORD=redka -e POSTGRES_USER=redka -e POSTGRES_DB=redka \
  postgres:18 -c synchronous_commit=off -c shared_buffers=1GB \
  -c effective_cache_size=3GB -c max_wal_size=4GB \
  -c checkpoint_timeout=30min -c full_page_writes=off
docker run -d --name kpg_redka --network kpgnet --entrypoint redka \
  pg_resp-bench-redka:pinned -h 0.0.0.0 -p 6379 \
  "postgres://redka:redka@kpg_pg:5432/redka?sslmode=disable"
docker exec kpg_pg psql -U redka -d redka -tAc \
  "SELECT (SELECT count(*) FROM rkey), (SELECT count(*) FROM rstring);"
```

The redka image is built from redka's **own** Dockerfile at the pinned clone
commit `d3c353f02470` (`docs/refs/PINS.md`), so the binary is upstream's build,
not ours.

## 7. K-pg's PostgreSQL is deliberately configured in the competitor's favour

The point of the K-pg arm is to compare *architectures*, so it must not be
possible to dismiss the result as "you ran Redka on an untuned database". Its
PostgreSQL therefore gets settings chosen to make Redka look as good as
possible:

| setting | value | why it favours Redka |
|---|---|---|
| `synchronous_commit` | `off` | removes WAL fsync from Redka's commit path — its single largest per-op cost, and the one pg_resp does not pay at all because it never writes WAL |
| `shared_buffers` | `1GB` | the whole Redka working set stays in PostgreSQL's cache; no arm should be disk-bound |
| `effective_cache_size` | `3GB` | encourages index scans over seq scans for Redka's key lookups |
| `max_wal_size` / `checkpoint_timeout` | `4GB` / `30min` | pushes checkpoints out of the measurement window |
| `full_page_writes` | `off` | further reduces WAL volume |

**`synchronous_commit = off` deserves being called out**, because it means the
K-pg arm is *not durable* during the benchmark. That is deliberate and it is the
generous choice: it removes Redka's durability cost from the comparison, leaving
only the architectural cost of translating each operation into SQL. If pg_resp
still wins by ≥5x against a Redka that is not even paying for durability, the
structural claim is stronger, not weaker. Stated in the README too, since a
reader who spots it independently should find we said it first.

## 8. One arm live at a time

Every measured run has exactly one arm running; all other arms are stopped
first. On a single box the arms otherwise contend for the same cores, page cache
and memory bandwidth, and a co-resident idle Redis still holds its `maxmemory`
allocation. `bench/harness/arms.sh exclusive <arm>` enforces this by stopping
every other arm container and then failing if any other arm port still answers,
and `sweep.py --require-exclusive` refuses to run otherwise.

## 9. Core pinning plan (`taskset`)

Topology of the development box: Intel i7-10750H, 6 physical cores / 12 logical,
SMT enabled, siblings **adjacent** (`/sys/.../thread_siblings_list` gives `0-1`,
`2-3`, … `10-11`), so logical CPU pairs share a physical core.

| side | logical CPUs | physical cores |
|---|---|---|
| server (PostgreSQL + pg_resp, or the incumbent container) | `0-5` | 0, 1, 2 |
| benchmark client (`memtier_benchmark`) | `6-11` | 3, 4, 5 |

The split is on *physical* core boundaries, so client and server never share a
core's execution resources — a naive "first half / second half" split of an
interleaved sibling map would put both on the same physical cores and quietly
serialise them. pg_resp executes commands on one thread (D4), so three physical
cores is ample for the server side.

Applied as `taskset -c 0-5` on the server process and `taskset -c 6-11` on
memtier (`sweep.py --server-cpus / --client-cpus`, recorded in every raw
header). The dedicated box's own topology must be re-derived the same way and
recorded here before its runs — do not copy this table to a different machine.

**Same-machine client placement remains a stated limitation** (§1), pinning
reduces but does not remove it. If the dedicated box permits a separate client
host, that is strictly better and pinning becomes unnecessary.

## 10. Allocator per arm — not matched, and it is not in pg_resp's favour

| arm | allocator |
|---|---|
| R-def / R-opt | jemalloc 5.3.0 (reported by `redis-server --version`) |
| V-opt | jemalloc 5.3.0 (reported by `valkey-server --version`) |
| K-pg | glibc malloc (Go runtime + PostgreSQL); its cache is in PG tables, not a heap |
| **P-def / P-opt** | **glibc malloc** — pg_resp has no `jemallocator` dependency, so Rust's default system allocator is used |

This matters most for bible §10's **bytes of RAM per 1M cached 1 KB entries**
(W6). jemalloc is generally stronger than glibc malloc for this exact pattern —
many small, long-lived, similarly-sized allocations — so any per-entry memory
gap between pg_resp and the incumbents includes an allocator component working
*against* pg_resp. The measurement is still reported as measured; the caveat is
reported beside it rather than used to explain the number away. Switching
pg_resp to jemalloc is a v0.2 question (and would also make bible §13's
"jemalloc stats in resp.stats()" mitigation literally true, which it currently
is not).

## 11. Correction: the "hit rate is worth ~2.4x" claim is withdrawn as a magnitude

During Phase 4 harness validation I reported that hit rate was a ~2.4x
confound, from two runs of the same cell:

| # | pre-state | test | hit | ops/s | p99 |
|---|---|---|---|---|---|
| 1 | cold store, no warm-up | 15s x1 | 1.05% | 198,509 | 4.86 ms |
| 2 | 45s SET-only warm-up immediately prior | 15s x1 | 100.00% | 82,637 | 27.26 ms |
| 3 | already warm, no warm-up run | 5s x1 | 100.00% | 213,549 | 3.89 ms |

Runs 2 and 3 have the **same 100% hit rate** and differ by **2.6x**, which
means the 198,509 → 82,637 gap cannot be attributed to hit rate: those two runs
differed in more than one variable (pre-state, and a 45-second pure-SET burst
finishing microseconds before run 2 started, which leaves CLOCK-LRU under heavy
eviction pressure). **The direction of the effect is still sound** — a GET miss
returns 5 bytes where a hit returns 1 KB, so an under-warmed arm does less work
per operation — but **the magnitude is not established and must not be quoted.**
It gets isolated properly on the dedicated box, one variable at a time, under
the full 3x60s protocol.

Two things this does not change, both of which stand on their own:

- **Warm-up must be equal across arms and recorded.** Whatever the magnitude,
  comparing a warm arm against a cold one compares different work.
- **Hit rate must be reported in every cell**, so a reader can check parity
  instead of trusting that it held.

And one thing it adds:

- **Sub-20-second runs on this box are not stable enough to compare at all.**
  Three runs of one cell spanned 82k–213k ops/s. ENV.md §4 already put
  run-to-run variance at ~10% for 15s runs at a fixed configuration; these
  short validation runs were worse than that, and the difference is that they
  varied pre-state as well. This is an argument for bible §10's 3x60s protocol
  and against reading anything into a quick run — including a quick run that
  agrees with expectations.

Recorded here rather than quietly dropped, because a plausible-sounding
magnitude that nobody re-derived is exactly the failure iron rule 7 exists to
prevent, and this one was mine.

## 12. Official-box acceptance criterion per cell

Every published cell is **3 × 60 s** (bible §10), and:

```
spread = (max_run_ops - min_run_ops) / median_run_ops   must be <= 8%
```

If a cell exceeds 8%, it is **re-run once** and the outcome flagged either way;
both attempts stay committed. `sweep.py` computes this from memtier's per-run
`RUN #N RESULTS` sections (`--print-all-runs`, added automatically whenever
`--run-count > 1`), records all three figures in the artifact header, and
**refuses to mark a cell publishable unless the spread is within threshold** —
so an unstable cell cannot silently become a README number.

Two related points about how the headline figure is chosen:

- **The reported figure is the median *run*, not a column-wise median and not
  memtier's `AGGREGATED AVERAGE`** — that block is an arithmetic *mean*, while
  bible §10 asks for medians. Taking the median run keeps every number in a
  published row (ops/sec, p50, p99, p99.9, hit rate) belonging to one real run
  rather than to a synthetic composite.
- **A 1-run cell is never acceptable official data.** The harness stamps
  "single run — no spread available" into any such artifact.

### The cautionary artifact

The reason this criterion exists in this form is §11: three short validation
runs of one cell on WSL2 spanned **82k–213k ops/sec**, a spread of well over
100%, with two of them at an *identical* 100% hit rate. Nothing in memtier's
output flagged it; each run reported a clean summary table and a plausible
number. Had any one of those been taken as "the" measurement, it would have
been indistinguishable in a document from a real one. An 8% gate would have
rejected all three.

For calibration, a properly exclusive 3-run cell on the same box came in at
`#1 242,710 / #2 237,043 / #3 246,022 ops/sec` → spread **3.70%**, which passes.
So 8% is achievable here even under WSL2 once the arm is exclusive and the store
is warm; it is not a lax threshold chosen to let results through.

## 13. Topology must be re-derived on the box, never ported

§9's CPU map is **specific to the development machine** and must not be copied.
SMT sibling numbering is a kernel/firmware property that differs between
machines — adjacent pairs (`0-1`, `2-3`, …) here, but commonly
`0,N`/`1,N+1`/… elsewhere. Porting a pinning map to a box with the other
convention silently puts client and server on the *same* physical cores, which
looks like a tuned benchmark and behaves like a serialised one.

Re-derive on the bench box and paste the output here before any official run:

```bash
lscpu | grep -E '^CPU\(s\)|^Thread|^Core|^Socket|Model name'
for c in $(seq 0 $(($(nproc)-1))); do
  printf 'cpu%-3s siblings: %s\n' "$c" \
    "$(cat /sys/devices/system/cpu/cpu$c/topology/thread_siblings_list)"
done
cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor   # must read: performance
```

The governor line is the one WSL2 cannot answer at all (§1). On the dedicated
box it must read `performance`, and its value goes in this file verbatim.
