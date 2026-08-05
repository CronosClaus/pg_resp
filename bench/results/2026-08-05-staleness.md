## G2 — commit-to-eviction staleness

All figures in milliseconds.

### Part A — what the applier costs the committing transaction

| measurement | n | min | p50 | p99 | p99.9 | max | mean |
|---|---|---|---|---|---|---|---|
| COMMIT, no invalidation queued | 500 | 0.946 | 1.518 | 3.408 | 5.830 | 5.830 | 1.629 |
| COMMIT, one invalidation queued | 500 | 1.139 | 1.642 | 4.689 | 51.237 | 51.237 | 1.833 |

Added cost at p99: **1.282 ms** (p50: 0.124 ms).

### Part B — the window a concurrent reader can observe

| measurement | n | min | p50 | p99 | p99.9 | max | mean |
|---|---|---|---|---|---|---|---|
| staleness window (upper bound) | 1000 | 0.937 | 1.247 | 2.232 | 5.599 | 6.180 | 1.318 |

- Measured as `t_evicted - t_commit_start`, where `t_commit_start` is taken immediately before `COMMIT` is sent. Row visibility cannot precede that instant, so this **overstates** the true window.
- Polling resolution: 70738 cache samples taken during the run (one RESP round trip each; SQL is not polled in the hot loop).
- Iterations skipped because a transition was not observed within 5s: 0.
- `t_evicted - t_commit_done` was <= 0 in **999/1000** iterations (99.9%), i.e. the eviction had already landed before the committing client was told its commit succeeded. This is the structural reason the naive measurement would have reported ~0.

**Gate (p99 < 5 ms): PASS** — measured p99 2.232 ms.
