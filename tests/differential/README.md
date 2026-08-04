# Valkey differential oracle — status (Phase 1)

Bible §5 Phase 1 gate + §9: "the highest-leverage testing idea in this
project." Status: **PARTIAL(docker) / PARTIAL(no oracle binary)** — the
harness is written and its own mechanics are verified; it has not yet been
run against a real Valkey instance.

## What's verified

`mechanics_selftest.py` points both "sides" of `generate_and_compare.py`'s
replay/diff engine at the **same** local pg_resp instance, clearing the used
keys between runs so both start from an identical empty state. Result:

```
500 commands replayed, 0 mismatches
Harness mechanics OK: deterministic replay against a cleared store produces
identical responses both times.
```

This proves the RESP2 reply parser, command builder, and diff logic are
correct — **it does not prove pg_resp matches Valkey**, since no second
implementation is involved. Don't mistake a clean self-test run for a passed
differential gate.

## Why the real run didn't happen this phase

Two independent blockers, either one sufficient on its own:

1. **No docker** in this WSL2 environment (confirmed in
   `reports/phase0.md`'s environment pre-flight) — the natural way to stand
   up a Valkey instance for comparison.
2. **No local Valkey binary either.** `/ref/valkey` is a sparse checkout of
   `src/` + `tests/unit/type` only (per bible §4's "grounding, not bulk"
   clone policy) — it's missing the `deps/` tree (bundled jemalloc, lua,
   hiredis, fpconv) a from-source build needs. Building those from scratch
   was judged poor effort/value against this run's remaining time budget.
   No Ubuntu `valkey` package exists in this machine's configured apt
   sources (checked; only `redis-server`/`redis-tools` are available, and
   bible D8 says the oracle must be Valkey specifically — using
   Redis-the-binary here, even as a black-box network peer with zero source
   code read, would misrepresent this specific gate's result).

## Running the real thing (once docker, or a Valkey binary, is available)

```
docker compose run --build runner
```
from this directory — brings up pg_resp + `valkey/valkey:8`, replays 5000
random T0 commands against both, diffs every reply.

Without docker, point `generate_and_compare.py` at any two already-running
RESP2 servers:

```
python3 generate_and_compare.py \
  --candidate-host 127.0.0.1 --candidate-port 6379 \
  --oracle-host    127.0.0.1 --oracle-port    6380 \
  --seed 42 --commands 5000
```
