# Phase 1 plan — RESP2 core

Written per this run's Stage B item 8 (substitutes for the human
`/kickoff 1` ritual in an unattended overnight run). Ordered items, fast-loop
before slow-loop, per CLAUDE.md's iron rules and bible §7.

## Gate table (bible §5 Phase 1, copied verbatim)

| gate | pass condition |
|---|---|
| protocol fuzz | `cargo-fuzz` on parser, ≥ 1 CPU-hour, zero crashes/UB |
| client compat matrix | redis-cli, redis-py, node-redis, go-redis, jedis: connect + T0 script green (Dockerized, in CI) |
| Valkey differential test | same random T0 command stream → pg_resp and Valkey → identical responses (modulo INFO/errors whitelist) |
| latency | localhost GET 64B, unpipelined, p50 ≤ 150 µs, p99 ≤ 1 ms |
| lifecycle regression | Phase 0 S1 table still green with full server |

**Pre-scoped PARTIAL(docker) condition**: this WSL2 environment has no
`docker` CLI installed at all (confirmed in Phase 0 environment pre-flight).
The client compat matrix and Valkey differential test both require
Dockerized harnesses per bible §6's repo layout (`tests/compat/`,
`tests/differential/`). Per this run's pre-approved amendments, these two
gates will be written (test harness code committed, ready to run) but marked
`PARTIAL(docker)` with the exact blocking command, not silently skipped or
faked green. If docker becomes available, both are a `make compat` /
`cargo test` invocation away from real results.

## Ordered items

1. **`resp-proto` crate** (`crates/resp-proto/`, no PG deps): RESP2
   parser + serializer implementing every framing rule and all 38 byte-level
   vectors from `.claude/skills/resp-protocol/SKILL.md` §7, red→green.
   Zero-copy where reasonable (parse into borrowed slices/`Bytes`-like type),
   but correctness first — this crate is also the fuzz target, so panics
   under malformed input are exactly what fuzzing needs to catch, not
   something to avoid via unsafe corner-cutting.
2. **`cargo-fuzz` target** on the parser, **launched in the background
   immediately** once the parser exists and passes its unit vectors — the
   ≥1 CPU-hour gate needs wall-clock accumulation, so it should run
   unattended while items 3-5 proceed, not be left until last.
3. **`resp-store` crate** (`crates/resp-store/`, no PG deps): the T0-scoped
   subset of bible §3.5's store — plain `HashMap<Box<[u8]>, Entry>` with
   TTL (lazy expiry check on read; the active sweep timer and CLOCK-LRU
   eviction are Phase 2 scope per bible §5, not built here, but the `Entry`
   shape should already carry what Phase 2 needs: value bytes, optional
   expiry instant, and a slot for a future CLOCK bit — designed so Phase 2
   extends rather than rewrites it).
4. **`pg_resp` extension crate**: replace the S1/S2 spike code with the real
   event loop — bgworker main-thread lifecycle (unchanged from the proven S1
   pattern) driving a server thread that runs the actual RESP protocol over
   `resp-proto`, dispatching T0 commands against a `resp-store` instance.
   GUCs from bible §3.5-3.6 (`pg_resp.bind_address`,
   `pg_resp.max_memory`, `pg_resp.password`; `pg_resp.eviction` is Phase 2
   scope since eviction itself is Phase 2). Per `pgrx-patterns` SKILL.md §8.7:
   command dispatch must never panic past the connection-handling boundary —
   a panic there is a measured full-instance-crash risk, not a theoretical
   one, per Phase 0's kill-9 finding.
5. **Lifecycle regression**: re-run Phase 0 S1's exact gate table (PING,
   `pg_ctl stop -m fast` timing, 20× restart loop, `kill -9`) against the
   *full* T0-command server, not the hardcoded-PONG spike, to confirm
   nothing about running the real event loop regressed shutdown cleanliness
   or restart behavior.
6. **Latency gate**: localhost GET 64B unpipelined, p50/p99, measured
   directly (no memtier needed for a single-command latency check — a tight
   Rust or redis-cli-in-a-loop client is enough, and avoids pulling in
   `bench-harness` before Phase 2 per the run's amendments).
7. **Compat matrix + Valkey differential harnesses**: write
   `tests/compat/` and `tests/differential/` per bible §6/§9, using the
   `resp-protocol` skill's client-handshake-probe table (§6) for what the 5
   clients need to see on connect. Given no docker, these are written and
   committed but executed/marked `PARTIAL(docker)` rather than run for real
   — the `compat-runner` and `differential-triager` subagents exist
   specifically for when docker is available; this run will invoke them and
   honestly report what happens.

## Fast-loop-before-slow-loop discipline

Items 1-3 never touch `cargo pgrx test` — pure `cargo test -p resp-proto -p
resp-store`. Item 4 is the first point this phase crosses into pgrx/FFI
territory, and only after 1-3 are green. This matches CLAUDE.md's stated
iron rule and bible §7's "prefer many small red→green loops on the PG-free
crates."

## Verdict placeholder

To be filled in `reports/phase1.md` at phase close: PASS / FAIL / PARTIAL,
per-gate raw numbers, and the single next action into Phase 2.
