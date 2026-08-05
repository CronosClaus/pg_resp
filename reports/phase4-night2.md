# Phase 4 — overnight run 2 state file

**Not a phase report.** This is the *live state file* for the authorized
overnight autonomous run of 2026-08-05/06. It exists so the run survives a
session interruption: if the session rate-limits or dies, the grid keeps running
detached on the box and this file says what was decided, what is done, and where
to resume. Updated at every stage boundary.

**Box:** `bench@188.34.158.72` (CCX33, 8 vCPU). Creation/destruction human-only.
**Repo head at start:** `6656815`. **Grid launched at:** `b6d38cc`.

---

## Resume instructions (read this first if you are a fresh session)

1. `ssh bench@188.34.158.72 'tmux ls; tail -40 ~/logs/grid.log'`
2. Cell-level progress is in `~/logs/grid-progress.tsv` on the box — one line per
   completed cell, appended by the runner. That file, not this session, is the
   source of truth for what has run.
3. Raw artifacts accumulate in `~/pg_resp/bench/results/grid/` on the box. They
   are written per cell as the grid proceeds, so an interrupted grid still has
   every completed cell on disk.
4. If the grid is still running: do not restart it. Poll.
5. If it finished: rsync `bench/results/grid/` back, **commit before analysing**,
   then `python3 bench/harness/curve.py bench/results/grid --arms <a> <b>`.
6. Then continue at the first unchecked box in "Stage checklist" below.

The grid runner is `bench/harness/grid.sh` (committed). It is idempotent per
cell: a cell whose artifact already exists is skipped, so a re-launch resumes
rather than redoing work.

---

## Authorized scope (from the overnight authorization)

Order: harness (committed first) → grid → local drafts while it runs → rsync +
commit → curve v2 + G3 verdict → W6 → demo 3 measurement → morning report +
SAFE TO DESTROY checklist.

**Hard stops** (write state, end run):
- more than 20% of cells VOID, or any systemic harness failure → stop the grid,
  diagnose here, **leave the box up**, do not improvise a protocol change
- no GHCR publish, no tag, no PGXN, no posting anywhere (pauses 2-4 stand)
- no protocol or warm-up changes beyond warm-up v2 as specified
- a gate failing twice for the same cause → park it, document, move on

---

## Stage A verdicts being carried forward (binding on all published output)

- **G3 on Stage A data: PASS.** Minimum same-workload ratio **6.7x**
  (`d1024-p1-c1`, both arms RTT-bound) against the >= 5x bar, rising with load.
- **Hand a skeptic the 6.7x first** — that framing is verbatim in BENCHMARKS.md.
- **No >= 100x cell ever appears without its conditions inline.**
- **K-pg's ~10k payload-invariant wall gets its own named subsection**, paired
  with the `pg_stat_user_tables` evidence (per-operation transaction cost
  dominating, not bytes).
- **"K-pg meets no cell under 10 ms at 64 B" is stated as the comparison ceasing
  to exist at that budget** — never rendered as a ratio.
- **Headline composition:** ">= 6.7x at worst, 40x at matched p99 (1 KB,
  <= 25 ms)".
- **Per-cell hit rates reported; any cell off parity by > 5 pts is flagged.**

## Warm-up v2 — supersedes v1 in every published table

Stage A used **v1**: a fixed *duration* (60 s) of SET-only load at the cell's own
client configuration. That is why P-opt's hit rate swung from 36% to 100% across
cells — 60 s at pipeline 16 with 64 connections writes far more data than 60 s at
pipeline 1 with 1 connection, so each cell started from a different degree of
fill. Measured spread of the arms' hit-rate difference on Stage A: **-27.6 to
+37.0 points**, i.e. a **~65-point band**, the "±30 pt limitation" recorded here.

**v2 is fixed key-count pre-population:** a SET-only pass of a fixed *number of
requests* (not seconds) over the same key space and access pattern the measured
run uses, so every cell of every arm begins from the same written volume
regardless of its client configuration. The remaining hit-rate difference between
a capped arm and an uncapped one is architectural (ENV.md §20) and is reported,
not engineered away.

**Stage B re-measures everything under v2. v2 supersedes v1 in all published
tables — no mixed-protocol tables.** Stage A survives only as the v1 record in
`ENV.md`, and its numbers do not appear in `docs/BENCHMARKS.md` or the README.

---

## Stage checklist

- [x] Orphan check: no tmux sessions, no containers, 204 GB free
- [x] State file created (this file)
- [x] Harness: saturation sampling + live-config capture + warm-up v2 + curve.py golden test — **committed before any grid cell** (`c1921ce`, `4122315`, `9ffedfb`, `b6d38cc`)
- [x] Grid launched detached — tmux session `grid`, started **2026-08-05T19:36:57Z** at head `b6d38cc`. ETA ~6.5 h (~02:00 UTC)
- [x] Local drafts done while grid runs: BENCHMARKS.md prose (`1a4f704`), README first screen (`53d3b9f`), demo 3 built (`a06e104`)
- [!] **Grid STOPPED at 12/108** — systemic harness failure, see "Grid stop" below. Box left UP. No protocol changed.
- [ ] Raw results rsynced and **committed before analysis**
- [ ] Curve tables v2 + publishability stamps + computed G3 verdict
- [ ] W6 (RAM per 1M x 1 KB)
- [ ] Demo 3 measurement + G4 crossover
- [ ] Morning report + SAFE TO DESTROY checklist

## Decision log for this run

Appended as decisions are made, so a fresh session inherits the reasoning rather
than re-deriving it.

1. **Pre-flight caught two launch-blockers that would each have cost the night.**
   (a) The box's `git pull` was aborting: result files rsynced off the box are now
   tracked in the repo but were still untracked there, so the box was silently
   running a `sweep.py` with no `--warmup-keys` and would have failed all 108
   cells on `unrecognized arguments`. All 48 blocking files were verified
   byte-identical against `origin/master` before any deletion. (b) `CpuSampler`
   named an attribute `self._stop`, shadowing `threading.Thread._stop()`, which
   `join()` calls — so a cell would run to completion and then crash with
   `TypeError: 'Event' object is not callable` at the moment it recorded results.
   A 10 s pre-flight cell found both.

2. **The 90%-of-one-core saturation threshold is now measured, not assumed.**
   Pre-flight: Redis at 64 B / p16 / 8 conns sampled **median 102.2%, peak
   103.1%** of one logical CPU. A single-threaded server saturates at ~100%, so
   the threshold is right and a 400% (four-CPU cpuset) expectation would have
   voided every honest cell.

3. **Live `CONFIG GET` confirms Redis 8 ships `io-threads 1`.** That is what makes
   the ranked comparison an honest single-thread-vs-single-thread one, and it is
   now captured per cell rather than assumed from documentation.

4. **First grid cell confirms the opt-in saturation decision was right.**
   `P-def d64-p1-c1` came in at 25,128 ops/s, spread 0.42%, publishable — with
   `cpu_peak 56%`, i.e. deliberately unsaturated at one connection with no
   pipelining. A global VOID-on-unsaturated rule would have discarded a valid
   low-load point on the curve.

5. **Demo 3's enforcement check caught a bug in itself on first run.** It derived
   "windows touched" from the run's duration, so a 3 s flood with a 60 s window
   computed one window — but the run straddled a real minute boundary, two buckets
   each correctly allowed one request, and a working limiter was reported as
   "DID NOT LIMIT". Fixed to derive the bound from wall-clock window indices, since
   the bucket key is a function of the clock. Side effect: the failure path is now
   proven to fire, empirically, rather than assumed to.

6. **Saturation VOID is opt-in per cell, not global.** `--require-saturation`
   voids a cell whose server never reached core saturation. Applying it to the
   whole grid would void every low-load cell — and those cells are legitimate
   latency-probing points on the throughput-vs-p99 curve, not failures. It is
   therefore applied to the cells where the anomaly protocol (ENV.md §21.3)
   demands it: the peak 64 B comparison cells. Sampling itself runs on **every**
   cell and is committed either way, so the evidence exists even where the
   verdict is not enforced.


---

# GRID STOP — 2026-08-05T20:45Z, at 12/108 cells

**Status: stopped under the authorized hard stop (systemic harness failure).
Box left UP. No protocol change improvised. Awaiting a human decision.**

## What happened

Cell 13 (`P-def d16384-p1-c1`) sat on its warm-up for 25 minutes at **~24
SETs/s** while the server burned **0.2% CPU**. At that rate a single 16 KB
unpipelined warm-up is ~2.3 hours and the 16 KB third of the grid would take
days, so the run was stopped rather than allowed to consume the night.

## The cause, and the diagnosis I got wrong first

**First diagnosis (WITHDRAWN):** a pg_resp bug. Operations arriving ~41 ms apart
with an idle server is the Linux delayed-ACK signature, and pg_resp really does
not call `set_nodelay(true)` on accepted sockets — only `resp-client` does, while
Redis sets it on every connection. It was a coherent story with real supporting
evidence.

**The control experiment refutes it.** 16 KB, pipeline 1, one connection,
SET-only, empty store:

| arm | ops/s | avg latency |
|---|---|---|
| P-opt (pg_resp) | 24.55 | 40.76 ms |
| R-opt (Redis) | 24.37 | 41.04 ms |
| V-opt (Valkey) | 24.38 | 41.03 ms |
| K-pg (Redka) | 23.87 | 41.88 ms |

All four collapse to the same ~41 ms per operation, and **pg_resp is marginally
the fastest of the four**. Redis sets `TCP_NODELAY` and collapses anyway, which
is what kills the hypothesis. So the cause is **client-side or environmental and
applies equally to every server** — it is not a property of any of them.

Two things were ruled out by measurement rather than by argument:

- **Not eviction.** Measured on an empty store, 300 requests, against a
  16,000-entry cap. The collapse is present with no eviction possible.
- **Not payload size alone.** The same 16 KB payload at pipeline 16 gives
  **62,412 ops/s**. Only the unpipelined case collapses.

The missing `TCP_NODELAY` on accepted sockets is still a real hygiene gap worth
fixing, and it is **not** the cause of this. It must not be reported as such.

## Why stopping was the right call regardless of the hours lost

Those 18 cells (`d16384` x `p1` x 3 connection counts x 6 arms) would have
published **"every cache in the comparison does ~24 ops/s at 16 KB"**. That
number is ~99.9% delayed-ACK timer and says nothing about any server, in either
direction. It would have been the most misleading table in the document, and it
would have arrived wearing a full protocol: 3 x 60 s, spread-gated, saturation
sampled, config verified.

## A gap in the hard stop itself

2 of the 12 completed cells (**17%**) are UNPUBLISHABLE on spread — 8.47% and
27.23%, both `P-def` at pipeline 16, the highest-throughput configurations. The
hard stop counts only **VOIDs** (`rc != 0`); an unpublishable-but-successful cell
exits 0. **A grid in which every cell exceeded the spread gate would never have
tripped the 20% stop.** That is a real hole in the guardrail, not a nuisance.

## What is preserved

12 cells (all `P-def`, 64 B and 1 KB), raw artifacts + run log + progress TSV,
committed at `e644778`. These payloads are unaffected by the 41 ms artefact
(1 KB unpipelined measures 25,753 ops/s at 38 us) and remain valid data.

## Decisions needed from the human — I am not authorized to improvise these

1. **The 16 KB workloads.** Bible §10 mandates 64 B / 1 KB / 16 KB. Options: (a)
   root-cause the client-side stall first, (b) run 16 KB at pipeline 16 only and
   document the exclusion, (c) publish the pipeline-1 16 KB cells labelled as
   measuring the transport artefact rather than the servers. Any of these is a
   protocol change.
2. **Whether the spread gate should feed the hard stop**, so an all-unpublishable
   grid stops itself.
3. **Whether to fix the missing `TCP_NODELAY`** before the remaining cells. It is
   a one-line product fix, but it changes the artifact under test and every
   completed cell was measured without it.

## Resume, once decided

`bench/harness/grid.sh` is idempotent per cell: the 12 completed cells are
skipped on relaunch. Restricting `SIZES`/`PIPES` in that script re-cuts the grid
without touching anything else.


---

# RESUMED after decisions — 2026-08-05T21:16:59Z

All three decisions executed. Grid relaunched **detached** at head `76ee662`,
90 cells (6 arms x 15 workloads x 3 connection counts), post-fix image built
21:02:58Z. ETA ~02:25Z.

## What was done

**D3 — `TCP_NODELAY` fixed** (`5e725fb`), image rebuilt, and verified inert at
1 KB exactly as expected: `P-def d1024-p16-c8` 391,697 -> 398,154 ops/s, **+1.65%
against that cell's own 2.38% spread**. Recorded as a verification, not a
performance claim (ENV.md §24). The 12 pre-fix cells are archived in
`bench/results/grid-prefix-superseded/` with a README stating they can never be
published — pre-fix image *and* a one-arm stopped grid.

**D2 — hard stop widened** to `VOIDED + UNPUB` (`5e725fb`), so a grid where every
cell blows the spread gate now stops itself instead of running to completion at a
0% void rate. And the two blown cells were repeated first, as asked:
`d1024-p16-c64` reproduced at **27.66%** (was 27.23%) — reproducibly unstable;
`d1024-p16-c1` came in within gate (was 8.47%) — borderline. ENV.md §23, including
the consequence accepted in advance: **the peak 1 KB pg_resp cell may have no
publishable figure at all.**

**D1(a) — root cause found inside the timebox.** The default socket send buffer is
16,384 B (`net.ipv4.tcp_wmem`); a 16 KB SET is ~16,430 B with framing, so the write
completes partially and the remainder waits on a delayed ACK. Confirmed by a
44-byte boundary (16,340 -> 25,268 ops/s; 16,384 -> 24.6 ops/s) and by
per-operation latency staying at 40.68-40.74 ms across a 64x concurrency change
with throughput exactly linear in connection count. **No legitimate in-box fix
exists** — root unavailable, the pinned memtier has no send-buffer option, and
`--sysctl` is refused under `--network host`.

**D1(b) applied.** 16 KB runs at pipeline 16 only (116k-163k ops/s, healthy). 16 KB
at pipeline 1 is excluded as a transport artefact, with the four-way 41 ms table
and the boundary sweep as its published proof (ENV.md §22). The ~24 ops/s figures
are never presented as any server's numbers.

## Also fixed, because it bit twice

`bench/harness/box/sync.sh` (`50b2e46`). Results produced on the box as untracked
files then committed from the workstation made every later `git pull` on the box
abort — which once left the box running a `sweep.py` without `--warmup-keys`, and
once let a background image rebuild start against the **old tree** because the
pull had failed and nothing checked. The script refuses unless every file it would
delete is byte-identical to `origin/master`.

## Remaining

- [ ] Grid complete (90 cells) — monitor armed for failures and stage boundaries
- [ ] rsync + **commit raw before analysis** -> curve tables v2 + G3 verdict
- [ ] W6 (RAM per 1M x 1 KB, caps raised ~1.5 GB, K-pg as disk + shared_buffers per D17)
- [ ] Demo 3 measurement + G4 crossover
- [ ] Morning report + SAFE TO DESTROY checklist
