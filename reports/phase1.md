# Phase 1 report — PASS (2 gates PARTIAL(docker), pre-approved)

**Status:** Phase 1 is closed. 3 of 5 gates fully green with measured
numbers; the remaining 2 (client compat matrix, Valkey differential) are
`PARTIAL(docker)` per this run's pre-approved amendments — this environment
has no docker at all (confirmed in `reports/phase0.md`), and Phase 1 is
cleared to proceed to Stage C when red rows are solely `PARTIAL(docker)`.

## Gate table (bible §5 Phase 1) — final

| gate | pass condition | result |
|---|---|---|
| protocol fuzz | `cargo-fuzz` ≥1 CPU-hour, zero crashes/UB | **PASS** — 10 parallel jobs × 601s ≈ **1.67 CPU-hours**, **~96M total executions**, **0 crashes**, empty artifacts dir |
| client compat matrix | 5 clients connect + T0 script green, dockerized | **PARTIAL(docker)** — redis-py **verified for real: 12/12 checks passed**; other 4 written, not locally run (no docker/node/go, jedis deps not fetched); see `tests/compat/README.md` for exact per-client status |
| Valkey differential test | random T0 stream, identical responses (modulo whitelist) | **PARTIAL(docker) / PARTIAL(no oracle binary)** — harness written and its own mechanics self-tested (**0/500 mismatches** on an isolated same-store replay); never run against a real Valkey (no docker, and `/ref/valkey`'s sparse checkout lacks the `deps/` tree needed to build from source); see `tests/differential/README.md` |
| latency | localhost GET 64B unpipelined, p50 ≤150µs, p99 ≤1ms | **PASS** — **p50 = 69µs, p99 = 139µs, p99.9 = 330µs** (measured with 5000 requests, TCP_NODELAY, after 500-request warmup) |
| lifecycle regression | Phase 0 S1 table still green with full server | **PASS** — inline `PING`→`+PONG` (verified via `/dev/tcp` and Python; `nc -q` itself was flaky in this environment, see below), clean shutdown **0.202s**, restart×20 **0/20 failures**, `kill -9` → postmaster self-heals identically to Phase 0's finding |

## What broke and why

1. **Latency gate failed badly on the first implementation — caught before
   being reported as green.** The initial event loop reused Phase 0 S1's
   fixed-interval sleep-then-poll pattern (`sleep(20ms)` between
   accept/read attempts on all connections). Measured **p50 = 20,236µs**
   (~20ms) — 135x over the 150µs target. Root cause: the 20ms sleep was
   adding its *full duration* to every single request's latency, not just
   shutdown latency, because the server thread was never actually blocked
   *on the sockets*. Fixed by rewriting the event loop with `mio`
   (bible §3.2 names it explicitly) — real epoll-based readiness
   notification, so real traffic wakes the thread immediately; the poll
   timeout (100ms) now only bounds how long the loop can go with zero
   activity before re-checking the shutdown flag. Re-measured: p50 69µs,
   p99 139µs. This is recorded in `pg_resp/src/lib.rs`'s own comments so a
   future contributor doesn't reintroduce the fixed-sleep pattern by
   copying Phase 0's spike code without re-deriving why it's wrong for a
   real event loop.
2. **A digest's own worked example was arithmetically wrong, caught by
   `resp-store`'s unit tests.** `docs/refs/valkey-notes.md` claimed "a key
   with 1ms remaining shows TTL=1, not 0" while also citing the formula
   `(ms + 500) / 1000` — which gives `(1+500)/1000 = 0` in integer division,
   contradicting its own claim. The formula (a real Redis/Valkey source
   idiom) is correct and is what pg_resp implements; the illustrative
   example was simply wrong arithmetic by the digesting subagent. Corrected
   in both `docs/refs/valkey-notes.md` and the `resp-protocol` skill, with
   unambiguous replacement examples (400ms→TTL 0, 600ms→TTL 1). This is
   exactly the "gate failure traces to a wrong digest fact → fix the
   digest" case CLAUDE.md's iron rule #3 describes, except caught by a unit
   test rather than a second gate failure.
3. **`nc`'s `-q` flag behaved unreliably against the real server** (empty
   read where a reply was clearly sent — confirmed via `/dev/tcp` and a
   Python socket getting the correct `+PONG\r\n` for the identical bytes).
   Root cause not fully diagnosed (BSD-nc `-q` semantics vs. this specific
   build); worked around by using `/dev/tcp` and Python for wire-level
   verification instead of chasing the `nc` quirk further — recorded so a
   future session doesn't mistake this for a server regression if it
   resurfaces.
4. **A dependency chain, not a code bug, blocked running the literal
   `redis-cli` binary.** Ubuntu's `redis-tools` `.deb` (fetched the same
   no-sudo way as Phase 0's libclang fix — `apt-get download` + `dpkg-deb
   -x`) pulls a transitive shared-library chain (`liblzf`,
   `liblua5.1-cjson`, `liblua5.1-bitop`, `liblua5.1`, `libjemalloc2`) deep
   enough that continuing to chase missing `.so` files had poor
   effort/value versus what was already proven: the exact bytes redis-cli
   exchanges were verified byte-for-byte by hand (36/36 vectors) earlier in
   this phase. Recorded as a deliberate scope stop, not a silent gap — see
   `tests/compat/README.md`.

## Decisions

No new `D<n>` decisions this phase. One design choice worth flagging for
future reference (not a locked architectural decision, just a documented
simplification): the event loop registers connections for `Interest::READABLE`
only, not `WRITABLE` — replies are written with a blocking-style `write_all`
on a nonblocking socket, which works fine for T0's small reply sizes but
could theoretically return `WouldBlock` under a full OS send buffer (heavy
pipelining or very large values). Not exercised by any Phase 1 gate; worth
revisiting if Phase 2's soak test or larger value sizes expose it.

## Open threads for Phase 2

1. **Real docker or a built Valkey binary** would immediately upgrade both
   `PARTIAL(docker)` rows to real PASS/FAIL — the harnesses are ready and
   waiting (`tests/compat/`, `tests/differential/`), no further authoring
   needed, just execution.
2. **`pg_resp.max_memory`, `pg_resp.eviction`, `pg_resp.password` GUCs are
   deliberately not yet registered** — scoped out of Phase 1 because they're
   tied to features that don't exist yet (eviction, AUTH — both Phase 2/3
   per bible's command-tier table), not an oversight. Register them
   alongside the features they configure.
3. **`WRITABLE` interest / partial-write handling** (see Decisions above) —
   revisit if Phase 2's soak test or larger value sizes expose backpressure.
4. This machine's libclang/cargo-fuzz-nightly/redis-py workarounds
   (`~/.cargo/libclang-lib`, `~/.cargo/clang-resource`, `~/.cargo/config.toml`,
   `~/.cargo/pylibs`, rustup's `nightly` toolchain) are all machine-local,
   outside the repo, and undocumented anywhere except `reports/BLOCKED.md`
   and this file — a fresh machine will need to redo them (or just run
   `sudo apt-get install libclang-dev` for real, which makes most of them
   unnecessary).

## Verdict

**PASS.** Proceeding to Stage C (Phase 2 pre-work, fast-loop only) per this
run's instructions.
