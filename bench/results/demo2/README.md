# Demo 2 raw results — and why the quoted claim is a COUNT, not a percentage

Raw output: `2026-08-06-demo2-rerun.txt`. Phase 3's earlier run is summarised in
`reports/phase3.md`.

## The two runs

| | fresh (2026-08-06) | Phase 3 (2026-08-05) |
|---|---|---|
| arm A stale serves | 894 of 1,031 reads (86.7%) | 621 of 705 (88.1%) |
| **arm A still stale at the 10 s limit** | **493** | **504** |
| arm A censored *fraction* | 55% | 81% |
| arm B stale serves | 1,138 of 22,964 (5.0%) | 1,183 of 20,300 (5.8%) |
| arm B censored | 0 | 0 |
| arm B staleness p50 | 2.3 ms | 2.54 ms |

## Why the count is stable and the fraction is not

**The censored count is stable — 493 against 504 — because it is a property of the
workload, not of the machine.** Arm A's forgotten invalidation path leaves a fixed
population of keys that never get invalidated at all. Every read of one of those
keys is stale forever, so the number of never-corrected staleness samples is set by
how many such keys the write storm touches, which the compose file fixes.

**The censored *fraction* moved 81% → 55% because it has read throughput in its
denominator.** Arm A served 705 reads in Phase 3 and 1,031 here; a faster or less
loaded machine serves more reads, and each additional read of an
*eventually*-corrected key dilutes the ratio without changing the count of
never-corrected ones. The fraction therefore measures the host as much as the bug.

**That is the reason the README quotes the count row and not the percentage.** 493
never corrected against 0 is the finding; "55% of stale samples were censored" is
that finding divided by an irrelevant number.

The same logic applies to the stale *rates* (86.7% / 5.0%): both are ratios over
reads, both will move on different hardware, and both are quoted only as context
for the count row.

## One dependency worth stating

**Arm A's `p50 ≥ 10 s` only holds while more than half its staleness samples are
censored.** With 493 of 894 censored (55%) the median sample is a censored one, so
the median is pinned at the observation limit and is a floor rather than a
measurement. Phase 3's 504 of 621 (81%) also cleared that threshold. On a machine
fast enough to push the censored fraction below 50%, arm A's p50 would become a
real (large) number instead of a floor, and the honest phrasing would have to
change from "≥ 10 s" to the measured value. **It held in both runs; it is not
guaranteed to hold in all.**

## Why this is not WSL2-disqualified

The Phase 4 environment amendment confines development-machine numbers to
harness validation, and that rule exists for **throughput variance** — ops/sec on a
hypervisor with a co-resident client is not comparable to a dedicated box.

These are **behavioural counts** produced by a compose-portable demo: whether a
forgotten code path ever invalidates its key is a property of the code, not of the
clock. Rates and latencies here do move with the environment and are labelled
inline as WSL2. The distinction the demo exists to show — bounded versus unbounded
— and the censored count are invariant.
