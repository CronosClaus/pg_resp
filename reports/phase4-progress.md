# Phase 4 progress report — written before a `/clear`

**Not a phase report.** Phase 4 is mid-flight; `reports/phase4.md` gets written
at the phase boundary via `/phase-report 4`. This file exists so the next
session can resume without the conversation that produced these decisions
(bible §0.2). Section A is **binding**.

**Session date:** 2026-08-05. **Branch:** `master`, pushed to
`https://github.com/CronosClaus/pg_resp` (public).
**Head at time of writing:** `4f68e86`.

---

# A. Consolidated approved amendments — BINDING

Reproduced as given. Where a later instruction superseded an earlier one, both
are kept and the supersession is noted, because the reasoning matters.

## A.1 Kickoff decisions (approval of the Phase 4 plan)

> **DECISIONS:** (1) Repo: public from day one, my account, name pg_resp —
> remote URL follows once created; I've lifted the git-push deny in settings for
> origin pushes. (2) D15: approved as recommended — official Redis images for
> R-def/R-opt, Valkey for V-opt; pin exact image tags and record digests in
> ENV.md; D8 untouched. (3) Staging approved: Stage A first, Stage B overnight.
> (4) Demo 3 only; demo 1 moved to backlog — amend bible §11 and log the cut
> explicitly in the phase report. (5) pg16/17 CI-only, local stays pg18.
>
> **D14 approved with three additions:** publish the full throughput-vs-p99
> curve for BOTH arms, not only the matched point; identical memtier client
> configuration on both arms of every compared cell; and report the secondary
> each-at-own-saturation ratio alongside the matched-p99 headline. **D16
> approved + one requirement:** the GHCR package must be explicitly set PUBLIC
> (packages default private independently of repo visibility) — G1 is unmeetable
> for outsiders otherwise. **D17 approved** as a documented note. **R6
> endorsed.**
>
> **NEW AMENDMENT — official benchmark environment:** local WSL2 runs are
> DEVELOPMENT data only (harness validation, Stage A dry run). The published §10
> numbers and the G1 quickstart timing both execute on a clean dedicated-vCPU
> Linux box (Ubuntu 24.04 + docker; I provision it when W5 is ready and provide
> SSH access). ENV.md must name the exact instance type, kernel, and governor;
> any number sourced from WSL2 that survives into a document must say so inline.
>
> **W12 amendment:** draft the three pgrx issue bodies to docs/upstream/*.md
> with repro snippets; I file them from my account — link the filed URLs from
> the launch post afterward.
>
> **W9 additions:** SECURITY.md contact = ccclaudiucarare@gmail.com; deb timebox
> of 2h confirmed, drop-and-document if exceeded.
>
> **Subagent acceptance rule as you stated it** (≥1 re-verified cell per arm on
> the main thread, demo measurement never delegated) — **confirmed as binding
> for every number that enters README or BENCHMARKS.md.**

## A.2 Execution instruction

> Main thread stays on Opus for the whole phase.
>
> **AUDIT CLOSED (do not redo, do not rewrite history):** git ls-files
> ref/target = 0; repo 2.84 MiB / 584 objects; value-grep for real credentials
> outside tests = empty. The push stands. Public bible/reports confirmed
> intentional — they are launch material.
>
> **Bench box:** provisioned by me at the W5-official pause, Ubuntu 24.04
> dedicated-vCPU; **STANDING RULE:** the box gets a freshly generated throwaway
> `pg_resp.password` at provision time — never reuse the local dev literal. It
> may appear in ENV.md (box is destroyed after the sweep).
>
> **HARD PAUSES (stop, report, wait for me):**
> 1. Before W5-official / W10: request the bench box — I provide SSH + instance
>    details.
> 2. W9 GHCR: after publishing, stop for me to flip the package PUBLIC; then
>    verify yourself with an unauthenticated docker pull before marking G1
>    measurable.
> 3. W12: draft docs/upstream/*.md only — I file the issues and return URLs.
> 4. W14: launch post + README first screen are DRAFTS pending my external
>    review before anything is announced anywhere. The v0.1.0 tag itself waits
>    for W13 all-green regardless.
>
> Standard rules hold: gate red twice same cause → stop that thread and report;
> §0 contract binding; WSL2 numbers never survive into a document without the
> inline caveat.

## A.3 Four mid-flight amendments

> 1. **K-pg: verify Postgres-backed, not SQLite** (redka's default). During a
>    SET burst, psql into its PG and confirm redka's tables exist and row counts
>    climb. Record "K-pg verified PG-backed via table growth" in ENV.md. This is
>    a G3-validity requirement.
> 2. **K-pg's PG gets deliberately competitor-favorable config**
>    (synchronous_commit=off, sane shared_buffers), documented as such.
> 3. **W1 CI: explicit pg_config paths per matrix leg**
>    (/usr/lib/postgresql/16/bin/pg_config), never $(which pg_config). Cache
>    cargo registry/git deps; NEVER target/ or generated bindings.
> 4. **Bench methodology, dry AND official:** one arm live at a time, others
>    stopped; plan taskset core-pinning (client vs server cores) and record it in
>    ENV.md; note allocator per arm (incumbents jemalloc-5.3.0, pg_resp glibc)
>    beside the W6 RAM metric.

## A.4 Six further amendments

> 1. **bench-runner: sonnet for W5/W6 official.** Mechanism: remove
>    CLAUDE_CODE_SUBAGENT_MODEL from .claude/settings.json (it forces all
>    subagents) and pin per-agent frontmatter: bench-runner sonnet,
>    differential-triager sonnet, compat-runner haiku, ref-digester haiku.
> 2. **Official-box acceptance criterion:** each cell 3×60s; spread
>    (max−min)/median ≤ 8% or re-run once and flag. Cite the WSL2 82k–213k
>    spread as the cautionary artifact.
> 3. **Carry the pg_stat_user_tables row** (10,000 SETs → 10,000 n_tup_ins,
>    10,001 idx_scan) into README/BENCHMARKS positioning as the measured
>    architectural explanation of the K-pg gap.
> 4. **jemalloc: v0.2 backlog only** (global_allocator behind a feature flag).
>    Do not touch the allocator mid-benchmark-phase — 96 B/entry and W6 stay
>    glibc-measured with the caveat as written.
> 5. **Box topology: never port the WSL2 taskset map** — re-derive from lscpu on
>    the bench box, record the map in ENV.md.
> 6. At pause 1 I'll add Bash(ssh:*) / Bash(scp:*) / Bash(rsync:*) to the allow
>    list for the box handoff.

---

# B. W-item status

| item | status | evidence |
|---|---|---|
| **W0** — R6 cold-build gate | **DONE** | `adb2284`. `cargo clean` removed 11,150 files / 4.4 GiB; `pgrx-pg-sys` regenerated `pg18.rs` (2,201,278 B) — first proven from-scratch bindgen since Phase 0. Build 1m10s. R1 **119/119** (22 client + 45 proto + 52 store), R1b **76/76**, clippy `--all-targets` clean, `cargo fmt --check` clean |
| **W1** — CI matrix PG 16/17/18 | **DONE** | `ec2632f` (workflow) + `90595bb` (fix). Run `31023715193` on `90595bb`: **all four jobs success** — fast loop 28s, PG16 24m31s, PG17 5m29s, PG18 12m44s, `cargo pgrx test` passing on all three legs. First passing test runs ever for PG 16 and 17. `target/` uncached by design; distro `libclang-dev` with an assertion that no vendored libclang is in play |
| **W2** — SCAN interleaving proptest (filler) | **OPEN** | Designated filler (kickoff amendment 6). Not started. Must not displace critical path |
| **W3** — bench harness + configs | **DONE** | `110dbcf`, then extended in `cb1a7fd` and `4f68e86`. `bench/harness/sweep.py`, `bench/configs/*` (6 arms). Parser validated against a committed artifact: reproduces ENV.md §4's 4×8/p16 row exactly (217,317.85 ops/s, p99 3.823) |
| **W4** — six-arm stand-up | **DONE (dev box, with a stated limit)** | `0c1aed6` + `cb1a7fd`. `bench/harness/arms.sh`. R-def/R-opt/V-opt verified live by SET/GET round-trip; K-pg built from redka's own Dockerfile at pinned `d3c353f02470` and **verified PG-backed** (ENV.md §6). Limit: on WSL2 only P-* is reachable from where memtier runs — see §D.2 |
| **W5-dry** — Stage A dry run | **DONE as harness validation** | `cb1a7fd`, `4f68e86`. Full mechanism exercised: password read from live instance, exclusivity, warm-up, NOAUTH=0, hit-rate guard, per-run parse, median run, spread, verbatim rerun line. **No cross-arm cell has been run anywhere** |
| **W5-official** | **BLOCKED — pause 1** | Needs the bench box |
| **W6** — RAM per 1M entries | **OPEN** | Needs the box. Allocator caveat pre-written (ENV.md §10), D17 note in `docs/BENCHMARKS.md` |
| **W7** — README rewrite | **OPEN** | `docs/BENCHMARKS.md` scaffold exists (`4f68e86`) with the architectural section written and all throughput sections marked PENDING |
| **W8** — ops.md 8 GB worked example | **DONE** | `0c1aed6`. Three-tenant table, measured RSS multiplier (285 MiB / 256 MiB = **1.11**), the 96-byte constant's real range (~71–134 B/entry, so ~40 MB unaccounted at 1M entries near a resize), OOM rule, cold-start stampede as caveat #1. Blast-radius already current from Phase 3 — unchanged |
| **W9** — packaging | **OPEN** | SECURITY.md contact supplied. GHCR is pause 2 |
| **W10** — G1 quickstart measurement | **BLOCKED** | Runs on the box; depends on W9's public GHCR image |
| **W11** — demo 3 (rate limiter) | **OPEN** | Demo 1 cut → v0.2 backlog (bible §11 amended in `4f68e86`) |
| **W12** — pgrx issue drafts | **DONE — draft only** | `adb2284`. `docs/upstream/`: 01 (`#[pg_trigger]`) and 03 (`GucSetting::get()`) stand; **02 WITHDRAWN** — see §D.1 |
| **W13** — full regression sweep | **OPEN** | Before the tag. R2/R3 delegated to compat-runner/differential-triager (models now pinned) |
| **W14** — tag + launch post | **BLOCKED — pause 4** | Drafts only; needs W13 all-green and external review |
| **W15** — `/phase-report 4` | **OPEN** | Must log the demo-1 cut explicitly (kickoff decision 4) |

**Housekeeping done:** `Bash(git push:*)` removed from the settings deny list
(`adb2284`); `CLAUDE_CODE_SUBAGENT_MODEL` removed and per-agent models pinned
(`4f68e86`).

---

# C. Pending pauses and owed items

| # | what | owed by | state |
|---|---|---|---|
| **Pause 1** | Bench box: SSH + instance type/kernel/governor. Plus `Bash(ssh:*)`/`scp`/`rsync` allowlist (A.4 item 6). Standing rule: freshly generated throwaway `pg_resp.password`, never the dev literal | human | **IMMINENT — the next thing that happens after W2** |
| **Pause 2** | GHCR package flipped **PUBLIC** after publish; then I verify with an unauthenticated `docker pull` before G1 is called measurable | human, then me | not reached (W9) |
| **Pause 3** | W12: human files issues 01 and 03, returns URLs for the launch post | human | drafts ready |
| **Pause 4** | README first screen + launch post reviewed externally before any announcement; `v0.1.0` waits for W13 all-green regardless | human | not reached |
| — | SECURITY.md contact | **supplied**: `ccclaudiucarare@gmail.com` | to be used in W9 |

**Not owed but worth re-stating:** the 397,398 ops/s ceiling in ENV.md §4 was
measured at ~37% hit rate and is not a valid basis under the equal-warm-up
protocol. Kickoff amendment 2 points at that number for throttle runs. §10's
matrix has no throttled arms so nothing is blocked, but the figure must be
re-measured on the box before it is reused, and it is labelled as the ~37%-hit
basis where it stands.

---

# D. Decided this session, and living nowhere else

## D.1 One of the three upstream findings does not exist

Kickoff amendment 5's finding 2 — "`ereport!`'s optional fourth argument being
**errdetail**, not errhint, which silently misfiles every hint" — **is not a
pgrx defect.** pgrx's own doc comment documents that argument as `detail` and
the macro calls `set_detail`. Our shipped comment in `crates/pg_resp/src/sql.rs`
was already correct; only the characterisation as an upstream bug was wrong.

A follow-up candidate in the same area also died: the doc examples call
`ereport!(PgLogLevel::ERROR, …)` while the visible arms match a bare `ERROR`
ident, which looked like a non-compiling example. It compiles — there is a
generic `($loglevel:expr, …)` catch-all arm. **Confirmed by compiling the
documented form in-tree**, not by reading, because reading had already misled us
once.

Finding 3 is *stronger* than recorded: `GucSetting::get()` has **no doc comment
at all**, while the type is `Sync`, which invites the opposite inference.

**Consequence: the launch post claims two upstream findings, not three.** Full
reasoning in `docs/upstream/02-ereport-no-hint-WITHDRAWN.md`.

## D.2 The dev box cannot run a cross-arm comparison, and why that is fine

Under WSL2 with Docker Desktop, `--network host` places a container in the
**Docker VM's** network namespace, not the WSL2 distro's. Measured from where
`memtier_benchmark` actually runs: pg_resp reachable, all three containerised
arms **refused**. K-pg additionally needs a `pg_hba` path from the Docker VM to
the dev PostgreSQL.

This surfaced a bug in the harness itself: the pre-run probe shelled out to a
*containerised* `redis-cli`, so it was able to "verify" an arm that memtier
could not reach at all. The probe is now a raw socket from the harness process,
traversing exactly the measurement path. A probe that does not use the
measurement path is not a probe.

It also broke the first exclusivity check, which only probed ports — a WSL2
container is unreachable while still burning the same physical CPUs and holding
its whole `maxmemory`. It now also checks `docker ps`, and was verified firing
against five live containers.

The topology is uniform on a native-docker box, so none of this constrains the
official run. It is independent support for the environment amendment.

## D.3 The "hit rate is worth ~2.4×" magnitude was withdrawn

Reported mid-session from two runs (198,509 ops/s at 1.05% hit vs 82,637 at
100%), then **withdrawn**: a third run at the *same* 100% hit rate came in at
213,549 ops/s, i.e. 2.6× apart at identical hit rate, so the original pair
differed in more than one variable. Direction still holds (a miss returns 5
bytes, a hit 1 KB); magnitude gets isolated on the box, one variable at a time.

Both operational consequences stand: **equal recorded warm-up per arm**, and
**hit rate published per cell**. Recorded in ENV.md §11 with the numbers, since a
plausible magnitude nobody re-derived is exactly what iron rule 7 exists to
prevent — and this one was mine.

## D.4 The jemalloc claim in `bench/configs/README.md` was wrong

An earlier revision said all three arms use jemalloc. **pg_resp has no
`jemallocator` dependency and uses glibc malloc**; the incumbents use jemalloc
5.3.0. The error was in the flattering direction, and the asymmetry works
*against* pg_resp on W6's bytes-per-entry, since jemalloc is stronger at that
allocation pattern. Corrected in place, with the correction itself recorded
rather than silently fixed.

## D.5 memtier's `AGGREGATED AVERAGE` is a mean, so it is not what we report

Discovered while implementing the 8% criterion. bible §10 asks for medians. The
harness parses each `RUN #N RESULTS` section and reports the median **run**, so
every column in a published row belongs to one real run rather than a synthetic
composite. `--print-all-runs` is added automatically whenever `--run-count > 1`,
and a single-run cell is stamped as unacceptable official data.

## D.6 CI logs are unreadable without a token; annotations are not

`GET /actions/jobs/{id}/logs` returns **403** unauthenticated, but
`GET /repos/{owner}/{repo}/check-runs/{id}/annotations` works. The workflow's
`cargo pgrx test` step therefore re-emits its grepped error lines and last 15
lines as `::error::` annotations, and uploads the full log as an artifact for a
human. Diagnosing a red leg should not require someone pasting a log out of a
browser.

The first red matrix was diagnosed without any of that, by a cheaper route worth
remembering: **`cargo pgrx test pg16` passes 76/76 locally**, so the failure was
environmental — pgrx writes the extension into the pg_config's own
sharedir/pkglibdir, user-owned under `~/.pgrx` locally but root-owned
`/usr/{share,lib}/postgresql/<N>` in CI. Exit code **1**, not 101, was the tell:
tooling failure, not a test panic.

## D.7 `arms.sh` port map and the Valkey version trap

Port map: 6379 P-def/P-opt (native), 6380 R-def, 6381 R-opt, 6382 V-opt, 6383
K-pg. Also: Valkey reports **both** a `valkey_version` (8.1.9) and a
compatibility `redis_version` (7.2.4); reading the first match records the wrong
version, so `arms.sh` prefers `valkey_version`.

## D.8 Redka's silent SQLite fallback is the reason A.3.1 exists

`config.Path = cmp.Or(flag.Arg(0), os.Getenv("REDKA_DB_URL"), sqliteMemoryURI)`
and the driver is chosen by `strings.HasPrefix(path, "postgres://")`. A typo in
the DSN yields a **silently SQLite-backed** arm that answers every RESP command
correctly. Redka's own docs put SQLite in-memory ~4× above its PostgreSQL
backend, so the mistake would have looked like a *harder* comparison rather than
an invalid one. Verified PG-backed via table growth (ENV.md §6):
`rkey`/`rstring` 0 → 10,000 under a 10,000-key burst, `kpgprobe:4242` read back
out of PostgreSQL, and `n_tup_ins=10000 idx_scan=10001`.

## D.9 Environment note for whoever resumes

The dev PostgreSQL instance is pgrx-managed on port **28818** with
`unix_socket_directories=~/.pgrx`, so psql needs `--pg-host ~/.pgrx` (carried
from `reports/phase3.md`). `pg_resp.password` is `secret123` there. All three
majors are already built locally under `~/.pgrx/{16.14,17.10,18.4}`, so
`cargo pgrx test pg16` and `pg17` can be run locally despite kickoff decision 5
making them CI-only — useful for exactly the isolation described in D.6.

---

## Single next action

**W2** (the randomized SCAN-interleaving proptest, fast loop, `resp-store`),
then **stop at pause 1** and request the bench box.
