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
