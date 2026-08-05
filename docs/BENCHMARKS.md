# pg_resp benchmarks

> **STATUS: DRAFT — prose written, number tables still PENDING.**
> The methodology, reading rules and limitations below are final and are being
> reviewed. **No throughput or latency table in this document is filled in yet.**
> Sections marked **PENDING** have no numbers on purpose — an empty cell is
> honest, a placeholder number is not.
>
> Two constraints on what may ever appear here. Only figures from the dedicated
> benchmark box are eligible (Phase 4 environment amendment: development-machine
> runs are harness-validation data and may never appear). And every table is
> **generated from committed raw artifacts** by
> [`curve.py`](../bench/harness/curve.py) — no number is typed into this file by
> hand, so a table and its raw file cannot drift apart.
>
> Superseded data is excluded rather than mixed in: the first measured curve used
> a fixed-duration warm-up whose written volume varied with concurrency, and its
> numbers survive only as a methodology record in `ENV.md`.

Method, environment, arm configuration and per-cell raw artifacts:
[`bench/results/ENV.md`](../bench/results/ENV.md) and
[`bench/configs/README.md`](../bench/configs/README.md). Every figure in this
document has a committed raw file and a command that regenerates it.

## What is being compared, and what the comparison means

Six arms (bible §10). Two of them answer different questions, and conflating
them is the most common way to misread this page:

| arm | what it answers |
|---|---|
| R-def / R-opt / V-opt | **raw ceiling.** Redis and Valkey are expected to win throughput. This is stated up front, not buried |
| K-pg (Redka on PostgreSQL) | **architecture class.** In-process cache vs translating each operation into SQL — the structural claim |
| P-def / P-opt | pg_resp, untuned and with documented tuning only |

## The architectural difference, measured rather than argued

Before any throughput number: the reason the K-pg comparison exists is a
difference in *what each architecture does per operation*, and that is
countable independently of how fast any machine is.

**10,000 RESP `SET`s of distinct new keys against Redka on PostgreSQL, observed
from inside that PostgreSQL** (measured on the official box, `ENV.md` §20):

```
pg_stat_user_tables:
  relname  | n_tup_ins | n_tup_upd | idx_scan
  ---------+-----------+-----------+----------
  rkey     |     10001 |         0 |    30005
  rstring  |     10001 |         0 |    10003

row counts:  rkey 0 -> 10000,  rstring 0 -> 10000
```

Ten thousand cache writes became **twenty thousand row inserts across two
tables** and **roughly forty thousand index scans**, inside a database, through
a query planner, on tables with indexes to maintain. pg_resp's ten thousand
writes are ten thousand hash-map insertions in the background worker's own heap
(D2) — no SQL, no planner, no WAL, no shared buffer.

Two corrections to how this was stated before the box measurement, both of which
happen to run in pg_resp's favour and therefore get spelled out rather than
quietly swapped in:

- An earlier draft reported only `rstring` (`n_tup_ins = 10000`,
  `idx_scan = 10001`) and described it as "one row inserted per SET". Redka
  writes key metadata to `rkey` *and* the value to `rstring`, so a new key costs
  **two** inserts, and the index-scan count was understated roughly fourfold.
- **A `SET` that overwrites an existing key is cheaper than one that creates it**
  — an `UPDATE` to `rstring` rather than a pair of inserts. This measurement used
  10,000 distinct new keys (`n_tup_upd = 0` confirms none were overwrites), so it
  is the cost of *populating* a cache. A steady-state workload over a bounded key
  space mixes both, and the per-operation cost there sits below this figure. The
  throughput tables measure that mixed case directly; this section explains the
  mechanism, and should not be read as the constant applying to every `SET`
  forever.

**This is the honest explanation of whatever gap the throughput tables show**,
and it is worth more than the ratio itself: it is a structural property of the
two designs, reproducible on any machine, and unaffected by CPU governor, SMT,
or how the client was pinned. The ratio is what a reader sees; this is why it
exists.

Two things it is *not*:

- It is **not** a criticism of Redka. Redka is durable and pg_resp is not; those
  inserts are what durability costs, and Redka's own README is explicit that raw
  performance is not its goal. A cache that survives restart is a different
  product, and for some workloads it is the right one.
- It is **not** a claim that SQL is slow. It is a claim that *a cache* pays a
  large constant per operation when every operation is a transaction against
  indexed tables — which is precisely why the K-pg arm's PostgreSQL is
  configured in Redka's favour (`synchronous_commit=off` and the rest, ENV.md
  §7), so that this per-operation cost is what remains rather than fsync.

Reproduce: `bench/results/ENV.md` §20 (official box) and §6 (first verification).

## Raw throughput — PENDING (dedicated box)

Where Redis and Valkey win, with numbers. Per bible §0.5, every arm's figures
are published including the ones that lose.

### Reading rule: transport-bound cells are not ties

At 1 KB and pipeline 16 the benchmark saturates the **loopback + syscall + copy
path** at roughly 215-230 MB/s — about 410,000 ops/s at this workload's average
payload — regardless of which server is behind the socket. The arithmetic is in
[`ENV.md`](../bench/results/ENV.md) §21.4, and the evidence is in §21.1: five
different implementations (pg_resp, Redis, Valkey) landed within 4% of each
other, and doubling the client's cores moves the figure by 1.7%.

**Therefore, binding on this document and on the README:** where arms land within
noise of that ceiling, the cell is reported as **transport-bound**, never as
parity, equivalence, or "matching Redis". The honest statement is that *the
measurement cannot distinguish the servers at that payload size* — a fact about
the bench, not a property of pg_resp. Cells below the ceiling (64 B) and the K-pg
comparison (an order of magnitude below it) are where the arms are genuinely
distinguishable, and those are the cells that carry an interpretation.

## Structural comparison vs K-pg — tables PENDING (Stage B), method final

Per D14 as amended: the matched-p99 headline, the **full throughput-vs-p99 curve
for both arms** (not only the matched point), identical memtier client
configuration on both arms of every compared cell, and the secondary
each-at-own-saturation ratio reported alongside.

### How to read the ratio, before you see one

**If you are skeptical of this comparison, start with the weakest cell, not the
strongest.** At one connection with no pipelining — where both arms are bound by
network round trips and neither server is working hard — pg_resp is about
**6.7x** Redka-on-PostgreSQL. That is the honest floor, it is the number a
skeptic should be handed first, and it is the one that decides the gate: G3 asks
for >= 5x and the *minimum* cell is what has to clear it, not the average and
certainly not the maximum.

The ratio then grows with load, because the two architectures diverge under
pressure rather than at rest. Cells at or above 100x exist and are published, and
**they never appear without their conditions inline** — payload size, pipeline
depth, connection count, and both arms' p99 at that point. A three-digit ratio
quoted bare is a marketing number; the same ratio with "at 64 B, pipeline 16, 64
connections, where Redka's p99 is 83 ms and pg_resp's is 0.6 ms" is a
measurement.

Headline composition, fixed in advance so it cannot drift toward the flattering
end: **">= 6.7x at worst, 40x at matched p99 (1 KB, <= 25 ms)".**

### Where the comparison stops existing

At 64 B there is no p99 budget below 10 ms that Redka can meet at any load on
this hardware. The correct statement is that **the comparison ceases to exist at
that budget** — one arm has no qualifying cell — and not that the ratio is
large. A ratio computed against a missing cell is not a large ratio; it is a
division by nothing. The tables render it as "meets no cell at this budget".

### The ~10,000 ops/s wall: why K-pg's throughput barely moves

The single most informative thing in the K-pg data is a *flat* line. Across
payload sizes from 64 B to 1 KB, pipeline depths of 1 and 16, and 8 to 64
connections, Redka-on-PostgreSQL lands within a narrow band around
**~10,000 ops/s**. Offered load above that point does not raise throughput at
all; it converts almost entirely into latency, with p99 climbing from single-digit
milliseconds to well past 100 ms while ops/s stays put.

Two features of that wall matter more than its height:

**It is payload-invariant.** 64 B and 1 KB values give effectively the same
ceiling. A byte-rate limit would not behave that way — a 16x larger value would
cost roughly 16x more bytes moved. A *per-operation* limit behaves exactly that
way. So whatever bounds this arm is priced per command, not per byte.

**The per-operation cost is countable, and it was counted.** From inside the same
PostgreSQL, 10,000 RESP `SET`s of new keys produced **two row inserts** (`rkey`
and `rstring`) and **~40,000 index scans** — the measurement in the architectural
section above, and in [`ENV.md`](../bench/results/ENV.md) §20. That is the wall:
each cache operation is a transaction against indexed tables, through a planner,
with the per-statement overhead that implies. pg_resp's equivalent is a hash-map
operation in the background worker's own heap (D2).

The two observations meet: a cost that is per-operation rather than per-byte
predicts a ceiling that does not move when the payload does, and that is what the
data shows. This is why the K-pg comparison is described as *architectural* rather
than as a tuning result — and why the arm's PostgreSQL is deliberately configured
in Redka's favour (`synchronous_commit=off`, matched `shared_buffers`; ENV.md §7),
so that what remains is the transaction cost and not fsync.

It is also the reason this section leads with 6.7x. If the gap were a constant
factor, one number would describe it. It is not a constant factor: it is one
architecture hitting a per-operation ceiling while the other keeps scaling, so the
ratio is a function of how hard you push. The floor is the honest summary, and the
curve is the actual finding.

## RAM per 1M cached 1 KB entries — PENDING (dedicated box)

With the allocator caveat that belongs beside it: the incumbents use jemalloc
5.3.0 and pg_resp uses glibc malloc (ENV.md §10). jemalloc is generally stronger
at this allocation pattern, so part of any per-entry gap is the allocator rather
than the design, and moving pg_resp to jemalloc is a v0.2 item deliberately not
done mid-benchmark-phase. K-pg is measured differently again (D17): its cache
lives in PostgreSQL tables on disk, so it is reported as disk bytes plus
`shared_buffers`, not RSS.

## Acceptance criteria applied to every cell here

- 3 × 60 s per cell; `(max−min)/median ≤ 8%` or re-run once and flag (ENV.md §12)
- median **run** reported, never memtier's `AGGREGATED AVERAGE` (a mean)
- one arm live at a time, verified (ENV.md §8)
- equal, recorded warm-up per arm; hit rate published per cell
- `--authenticate` on every run, `NOAUTH` count asserted zero (ENV.md §4)

## Method, in the order it constrains a number

Stated here because every table on this page is only worth the protocol behind
it, and because several of these rules exist as a direct result of a
measurement that went wrong first. Full detail:
[`ENV.md`](../bench/results/ENV.md).

**One arm live at a time.** Verified, not intended: the harness refuses to run if
another arm's port answers, and additionally checks `docker ps`, because a
co-resident container that is unreachable still burns the same cores and holds
its whole `maxmemory` allocation.

**Three runs of 60 s per cell, and the reported figure is the median *run*.** Not
memtier's `AGGREGATED AVERAGE`, which is an arithmetic mean — taking the median
run keeps every column of a published row (ops/s, p50, p99, p99.9, hit rate)
belonging to one real run rather than to a synthetic composite.

**A cell whose three runs spread by more than 8% is not publishable.** The
harness computes `(max−min)/median` from the per-run sections and refuses to
stamp the cell publishable outside that band. Provenance: three short validation
runs of one cell on the development machine once spanned **82k–213k ops/s**, two
of them at an identical hit rate, each with a clean-looking summary. An 8% gate
rejects all three.

**A single-run cell is never acceptable data** and is stamped as such
automatically, so it cannot be mistaken for a measured cell later.

**Equal warm-up, by written volume rather than by clock.** Every cell
pre-populates with the same *number* of SET requests over the same key space and
access pattern the measured run uses. An earlier protocol used a fixed 60-second
warm-up, which writes wildly different amounts of data at different pipeline
depths and connection counts — it made one arm's hit rate swing from 36% to 100%
across cells. Numbers measured under that protocol are superseded and do not
appear on this page.

**Hit rate is published for every cell.** A GET hit returns the value and a miss
returns 5 bytes, so two arms at different hit rates are not doing the same work
per operation. Any compared pair off parity by more than 5 points is flagged
inline. The capped arms evict and Redka does not (ENV.md §20); that asymmetry is
architectural, is reported, and cuts in both directions rather than favouring
either side.

**Server CPU is sampled through every run and committed.** A throughput
comparison in which one server was never core-saturated is measuring the harness.
For the cells where that matters most, an unsaturated server voids the cell
outright.

**Live configuration is read from the running server**, not from the config file,
and lands in each raw artifact's header — `CONFIG GET` for Redis and Valkey,
`SHOW` for pg_resp. A mounted-but-unparsed config file is invisible to a check
that reads the file, and that exact failure has already occurred here once.

**`--authenticate` whenever a password is set, and the output is scanned for
`NOAUTH` afterwards.** A run against an auth-enabled server once reported a
plausible 176,000 ops/s having never executed a single GET, in 820 MB of
rejection messages.

**Core pinning, re-derived per machine.** Server and client get disjoint sets of
whole physical cores. The map is re-derived from `lscpu` and
`thread_siblings_list` on each machine and never ported, because SMT sibling
numbering differs between machines and porting a map silently places client and
server on the same physical cores.

**Every figure has a committed raw file and a verbatim rerun command.** Tables on
this page are generated from those artifacts by
[`curve.py`](../bench/harness/curve.py), which is itself covered by golden tests
(`make harness-test`) — including one that makes a specific past error
structurally impossible to repeat.

## Limitations, stated by us rather than found by you

- **The benchmark client shares the machine with the server.** Pinning gives them
  disjoint physical cores, but they still share one last-level cache and one
  memory controller. A separate client host would be strictly better.
- **The CPU governor could not be set or read.** The bench box is a KVM guest
  whose `cpufreq` subsystem is absent entirely; the hypervisor owns P-states. The
  instance type has dedicated cores, which removes most of the concern, and the
  8% spread gate would reject a cell whose runs disagreed because the clock moved.
- **Allocators are not matched, and not in pg_resp's favour.** Redis and Valkey
  use jemalloc 5.3.0; pg_resp uses glibc malloc. jemalloc is generally stronger
  at this allocation pattern, so part of any per-entry memory gap is the
  allocator rather than the design.
- **pg_resp pays one `AUTH` per connection that the incumbents do not**, because
  its arms run with a password set and theirs run without one. The effect is tiny
  and it runs against pg_resp.
- **At 1 KB the measurement is transport-bound** and cannot distinguish the
  in-memory servers at all — see the reading rule above.
- **Single-threaded command execution** (D4). pg_resp is not sharded, and these
  numbers are a single core's worth of work on both sides of the ranked
  in-memory comparison.
- **This is not a durability comparison.** Redka is durable and pg_resp is not,
  by design (D5). The row inserts that make K-pg slow are what durability costs.
