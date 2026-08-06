# Upstream issue drafts (pgrx)

Four candidate findings from building [pg_resp](../../README.md) against pgrx
0.19.2. **Two survived scrutiny and are filed; two did not and are recorded here as
non-findings.** Keeping the dead ones is the point: a list of only the survivors
would misrepresent how many leads turned out to be our own misreading.

**Status: filed.** Two of the four were filed upstream from the maintainer's own
account on 2026-08-06; the other two are recorded here as non-findings and are
deliberately not filed.

- **[pgrx#2365](https://github.com/pgcentralfoundation/pgrx/issues/2365)** —
  `#[pg_trigger]` renders a returned error with `Debug`
- **[pgrx#2366](https://github.com/pgcentralfoundation/pgrx/issues/2366)** —
  `GucSetting::get()`'s main-thread-only check is undocumented

Both cost real debugging time on this project and are reproducible from a clean
`cargo pgrx new` scaffold.

| # | draft | pgrx version | filed as |
|---|---|---|---|
| 1 | [`pg_trigger`'s wrapper renders a returned error with `Debug`](01-pg-trigger-expect-debug.md) | 0.19.2 | **[pgrx#2365](https://github.com/pgcentralfoundation/pgrx/issues/2365)** |
| 2 | **[WITHDRAWN — not a pgrx bug](02-ereport-no-hint-WITHDRAWN.md)** | 0.19.2 | _do not file as planned_ |
| 3 | [`GucSetting::get()`'s thread check is undocumented](03-gucsetting-get-thread-check.md) | 0.19.2 | **[pgrx#2366](https://github.com/pgcentralfoundation/pgrx/issues/2366)** |
| 4 | **[NOT FILED — did not reproduce off one machine](04-memtier-16k-boundary-NOT-REPRODUCED.md)** | memtier `272eeb647df5` | _not filed, deliberately_ |

Verified on PostgreSQL 18.4, pgrx `=0.19.2` (commit `70383e884582`, per
[`docs/refs/PINS.md`](../refs/PINS.md)).

**Two findings survive, not four.** Finding 2 was checked against pgrx's source
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
