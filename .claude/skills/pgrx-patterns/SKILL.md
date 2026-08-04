---
name: pgrx-patterns
description: pgrx 0.18.x recipes for pg_resp — background worker registration/lifecycle/signals, GUC registration, transaction callback pattern, the off-main-thread forbidden list. Consult before writing or modifying ANY code that touches pgrx or PG FFI.
---
# pgrx patterns

**STATUS: STUB — Phase 0 task 3 fills this** from digests of /ref/pgrx (examples/bgworker, src/bgworkers.rs), /ref/postgres (worker_spi, bgworker.c, latch.c), /ref/pg_net and /ref/omnigres/omni_httpd.

Required contents when filled (bible §7):
1. Bgworker registration from _PG_init with exact pgrx 0.18 API names; shared_preload_libraries requirement; bgw_restart_time choice and rationale.
2. Signal handling: SIGTERM/SIGHUP wiring, latch vs self-pipe/eventfd pattern for waking an epoll loop from the PG side (S1's core mechanism).
3. GUC registration recipe (int/string/bool, units, ranges) matching bible §3.5–3.6 GUC list.
4. The xact-callback pattern from spike S2: register, queue in xact-local storage, apply post-commit, drop on abort — with the exact pgrx API surface used.
5. **The forbidden list**: every category of PG call that must never happen off the main bgworker thread (memory contexts, ereport, SPI, anything touching shared PG state), and the safe alternatives.
6. Traps found during spikes: build flags, cargo pgrx init/test quirks per PG version, anything that cost >30 min.
