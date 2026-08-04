---
name: bench-harness
description: Benchmark execution cookbook for pg_resp — the six arms of bible §10, exact memtier_benchmark invocations, environment checklist, result parsing into markdown tables. Consult for any performance measurement; bench-runner subagent follows this verbatim.
model: haiku
---
# Bench harness

**STATUS: STUB — Phase 0 task 3 fills this** from /ref/memtier_benchmark docs.

Required contents when filled (bible §7 + §10):
1. The six arms (R-def, R-opt, V-opt, K-pg, P-def, P-opt) with their exact config files in bench/configs/.
2. memtier flag cookbook for the workload matrix: ratio 1:10, value sizes 64B/1KB/16KB, pipeline 1/16, connections 1/8/64, keyspace 1M gaussian, 3×60s.
3. Environment checklist: cpu governor performance, SMT state recorded, client placement (second machine or pinned cores), everything logged to bench/results/ENV.md.
4. Result-parsing recipe: memtier output → medians table (ops/s, p50/p99/p999) in markdown.
5. The RAM-per-1M-entries measurement procedure (bible §10 metrics).
6. Soak procedure: 30 min mixed at ~50% max throughput, RSS sampling at 0/5/15/30 min.
