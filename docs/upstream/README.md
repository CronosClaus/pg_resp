# Upstream issue drafts (pgrx)

Three findings from building [pg_resp](../../README.md) against pgrx 0.19.2 that
look like upstream bugs or documentation gaps rather than mistakes on our side.
Each was hit while building a real extension, cost real debugging time, and is
reproducible from a clean `cargo pgrx new` scaffold.

**Status: drafts.** These are written to be filed by a human from their own
GitHub account, not by an agent. Once filed, record the issue URL in the table
below so the launch post can link them.

| # | draft | pgrx version | filed as |
|---|---|---|---|
| 1 | [`pg_trigger`'s wrapper renders a returned error with `Debug`](01-pg-trigger-expect-debug.md) | 0.19.2 | _pending_ |
| 2 | **[WITHDRAWN — not a pgrx bug](02-ereport-no-hint-WITHDRAWN.md)** | 0.19.2 | _do not file as planned_ |
| 3 | [`GucSetting::get()`'s thread check is undocumented](03-gucsetting-get-thread-check.md) | 0.19.2 | _pending_ |

Verified on PostgreSQL 18.4, pgrx `=0.19.2` (commit `70383e884582`, per
[`docs/refs/PINS.md`](../refs/PINS.md)).

**Two findings survive, not three.** Finding 2 was checked against pgrx's source
before drafting and does not hold: the `ereport!` doc comment documents its
fourth argument as `detail`, the macro calls `set_detail`, and a second candidate
defect in the same area (doc examples that looked like they could not compile)
was disproved by compiling them. The misreading was ours, our shipped code was
already correct, and the withdrawal page records the whole check so the reasoning
is not lost. The launch post should therefore claim two upstream findings.

## Filing notes

- Each draft is written as a complete issue body: summary, reproduction,
  observed vs expected, and a suggested fix. Paste the file's content below the
  title line.
- Finding 1 has a concrete patch suggestion; 2 and 3 are arguably
  works-as-designed and are framed as documentation/ergonomics issues rather
  than as bugs, which is both more accurate and more likely to be well received.
- If a maintainer says any of these is intended behaviour, that answer is worth
  recording here too — a documented "intended" is still an improvement over the
  silence that cost us the debugging time.
