# pg_resp benchmarks

> **STATUS: SCAFFOLD. No throughput or latency numbers in this document yet.**
> Every measured cell is pending the dedicated benchmark box (Phase 4
> environment amendment: WSL2 runs are development data only and may never
> appear here). Sections marked **PENDING** have no numbers on purpose —
> an empty cell is honest, a placeholder number is not.

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

## Structural comparison vs K-pg — PENDING (dedicated box)

Per D14 as amended: the matched-p99 headline, the **full throughput-vs-p99 curve
for both arms** (not only the matched point), identical memtier client
configuration on both arms of every compared cell, and the secondary
each-at-own-saturation ratio reported alongside.

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
