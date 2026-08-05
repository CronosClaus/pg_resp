# Phase 4 — overnight run 2 state file

**Not a phase report.** This is the *live state file* for the authorized
overnight autonomous run of 2026-08-05/06. It exists so the run survives a
session interruption: if the session rate-limits or dies, the grid keeps running
detached on the box and this file says what was decided, what is done, and where
to resume. Updated at every stage boundary.

**Box:** `bench@188.34.158.72` (CCX33, 8 vCPU). Creation/destruction human-only.
**Repo head at start:** `6656815`.

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
- [ ] Harness: saturation sampling + live-config capture + warm-up v2 + curve.py golden test — **committed before any grid cell**
- [ ] Grid launched detached
- [ ] Grid complete
- [ ] Raw results rsynced and **committed before analysis**
- [ ] Curve tables v2 + publishability stamps + computed G3 verdict
- [ ] W6 (RAM per 1M x 1 KB)
- [ ] Demo 3 measurement + G4 crossover
- [ ] Morning report + SAFE TO DESTROY checklist

## Decision log for this run

Appended as decisions are made, so a fresh session inherits the reasoning rather
than re-deriving it.

1. **Saturation VOID is opt-in per cell, not global.** `--require-saturation`
   voids a cell whose server never reached core saturation. Applying it to the
   whole grid would void every low-load cell — and those cells are legitimate
   latency-probing points on the throughput-vs-p99 curve, not failures. It is
   therefore applied to the cells where the anomaly protocol (ENV.md §21.3)
   demands it: the peak 64 B comparison cells. Sampling itself runs on **every**
   cell and is committed either way, so the evidence exists even where the
   verdict is not enforced.
