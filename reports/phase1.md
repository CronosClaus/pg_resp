# Phase 1 report — PASS (full)

**Status:** Phase 1 is closed. All 5 gates fully green with measured numbers.
The client compat matrix and Valkey differential test were originally
`PARTIAL(docker)` (this environment had no docker at the time); once docker
became available, both were closed for real as a Phase 2 kickoff pre-step —
see "PRE-STEP closure" below for what running them for real actually found.

## Gate table (bible §5 Phase 1) — final

| gate | pass condition | result |
|---|---|---|
| protocol fuzz | `cargo-fuzz` ≥1 CPU-hour, zero crashes/UB | **PASS** — 10 parallel jobs × 601s ≈ **1.67 CPU-hours**, **~96M total executions**, **0 crashes**, empty artifacts dir |
| client compat matrix | 5 clients connect + T0 script green, dockerized | **PASS (full)** — redis-cli 11/11, redis-py 12/12, node-redis 13/13, go-redis 12/12, jedis 13/13, all run for real via `docker compose` |
| Valkey differential test | random T0 stream, identical responses (modulo whitelist) | **PASS (full)** — **0/5000 mismatches** against real `valkey/valkey:8`, seed 42, via `docker compose` |
| latency | localhost GET 64B unpipelined, p50 ≤150µs, p99 ≤1ms | **PASS** — **p50 = 69µs, p99 = 139µs, p99.9 = 330µs** (measured with 5000 requests, TCP_NODELAY, after 500-request warmup) |
| lifecycle regression | Phase 0 S1 table still green with full server | **PASS** — inline `PING`→`+PONG` (verified via `/dev/tcp` and Python; `nc -q` itself was flaky in this environment, see below), clean shutdown **0.202s**, restart×20 **0/20 failures**, `kill -9` → postmaster self-heals identically to Phase 0's finding |

## PRE-STEP closure (docker became available; run as Phase 2 kickoff pre-step)

Running the two previously-PARTIAL gates for real, instead of just via
harness-mechanics self-tests, surfaced real bugs — exactly the value docker
access unlocked. All fixed before Phase 2 T1/T2 work began, per instruction
("fix any BUG-class divergences before building T1/T2 on top of them").

**Infrastructure bugs** (`docker/pg_resp.Dockerfile`, both compose files —
none of this had ever actually been built before):
1. `curl | sh` piped straight through masked a cert-store failure
   (`postgres:18` has no `ca-certificates` by default) — `sh` saw empty
   stdin and exited 0, so rustup silently never installed. Fixed: fetch to
   a file first, `ca-certificates` added.
2. `cargo-pgrx`'s build needs `pkg-config` + `libssl-dev` (openssl-sys) —
   not pulled in by `build-essential`/`libclang-dev` alone. Added.
3. **`pg_resp` is a cargo workspace member** (siblings: `resp-proto`,
   `resp-store`) — `cargo pgrx package`'s output therefore lands in the
   *workspace-root* `target/`, not `crates/pg_resp/target/`, even run from
   inside that directory. The Dockerfile's `COPY` paths were guessed at the
   per-crate path — grounded in an earlier *local* verification run whose
   "no such file" result was misattributed to a shell-cwd quirk instead of
   this actual cause. Fixed once the real build surfaced it unambiguously.
4. **`pg_resp.bind_address` defaults to 127.0.0.1** (bible D6's real
   security default) — a loopback-bound listener inside a container's
   network namespace does not accept traffic arriving via its `eth0`
   interface, confirmed by hand (`connection refused`) from a sibling
   container on the same compose network. Fixed with a test-only
   `postgres -c pg_resp.bind_address=0.0.0.0` override in both compose
   files (commented as test-harness-only, not a deployment recommendation).
   Valkey needed `--protected-mode no` for the same class of reason.
5. `go_redis`/`jedis` compose volumes were mounted `:ro`, but `go mod
   download`/`javac` need to write into that directory — fixed by copying
   to a writable `/work` first. `go mod download` alone also doesn't
   populate `go.sum` for a fresh module — needed `go mod tidy`.
6. `docker compose up --build --abort-on-container-exit` aborts the whole
   run on the *first* container exit — wrong flag for independent one-shot
   test containers (killed the other 4 clients the moment redis-cli
   finished, even though it passed). Fixed by starting `pg_resp` once and
   running each client via `docker compose run --rm <svc>` individually,
   restarting `pg_resp` between clients so state from one client's run
   (e.g. `INCR ctr`) can't leak into the next and produce false failures
   (this exact leakage produced two false "FAIL incr" results for jedis on
   the first attempt — not a pg_resp bug, a shared-fixture-state bug in the
   test loop).

**A real protocol-compatibility finding, not a test bug — required a
dispatch.rs fix:** redis-py's and node-redis's default-constructed clients
both attempt RESP3 first (`redis-py`: `protocol=None` resolves internally to
`DEFAULT_RESP_VERSION == 3`; node-redis: `RedisClientOptions`'s `RESP` type
parameter defaults to `3`) and **do not gracefully fall back to RESP2** on
any error reply to `HELLO` — traced through both libraries' actual installed
source (not guessed) to confirm this precisely. Since bible D9 forbids
implementing real RESP3 in v0.1, `HELLO` now succeeds (RESP2 array reply,
never a RESP3 Map) for version 2 or absent, and returns the exact same
`-ERR unknown command 'HELLO'` shape for anything else — matching what was
empirically proven tolerable, not the spec-documented `-NOPROTO` (which,
counterintuitively, both libraries treat as a *harder* failure than a plain
unknown-command reply and refuse to downgrade from). The actual, complete
fix for these two clients is client-side: pin `protocol=2` /
`{ RESP: 2 }` explicitly, which is each library's documented, correct way to
talk to a RESP2-only server — done in both test scripts, with the reasoning
recorded inline so it isn't rediscovered the hard way twice.

**Test-script-only bugs, not pg_resp bugs** (each would have produced a
false "FAIL" against a fully correct server): go-redis's `SetNX()` method
sends the literal legacy `SETNX` command (bible §3.4 T2 tier, correctly not
implemented in this T0-only pre-step) rather than `SET ... NX` (T0) — fixed
to use `SetArgs{Mode: "NX"}`, matching what the other 4 clients actually
test. go-redis's `Expire(ctx, key, 10)` passed a bare `10` where the
signature wants a `time.Duration` (nanoseconds) — silently truncated to 1s
by the driver, not 10s. node-redis's `client.quit()` sends the wire `QUIT`
command (T1 tier, not yet built) instead of just closing the socket — fixed
to `client.destroy()`; its EXPIRE check compared RESP2's raw integer reply
against a boolean.

## What broke and why (original Phase 1 build, before the PRE-STEP)

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
   `redis-cli` binary** in the original no-docker environment — later
   superseded entirely: docker became available and the official `redis:7`
   image runs `redis-cli` with zero dependency issues (see PRE-STEP above).

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

1. **`pg_resp.max_memory`, `pg_resp.eviction`, `pg_resp.password` GUCs are
   deliberately not yet registered** — scoped out of Phase 1 because they're
   tied to features that don't exist yet (eviction, AUTH — both Phase 2/3
   per bible's command-tier table), not an oversight. Register them
   alongside the features they configure.
2. **`WRITABLE` interest / partial-write handling** (see Decisions above) —
   revisit if Phase 2's soak test or larger value sizes expose backpressure.
3. This machine's libclang/cargo-fuzz-nightly/redis-py workarounds
   (`~/.cargo/libclang-lib`, `~/.cargo/clang-resource`, `~/.cargo/config.toml`,
   `~/.cargo/pylibs`, rustup's `nightly` toolchain) are all machine-local,
   outside the repo, and undocumented anywhere except `reports/BLOCKED.md`
   and this file — a fresh machine will need to redo them (or just run
   `sudo apt-get install libclang-dev` for real, which makes most of them
   unnecessary).
4. **`docker/pg_resp.Dockerfile` and both compose files are now real,
   verified, working infrastructure** (not "written but untested") —
   reusable for Phase 2's END-STEP (extending compat+differential to the new
   T1/T2 surface) and beyond, into Phase 4's packaging work.

## Verdict

**PASS (full).** All 5 gates green with real measured numbers, no PARTIAL
rows remaining. Proceeding with Phase 2 (T1/T2 command set, GUCs, soak test)
per the approved kickoff plan and its amendments.
