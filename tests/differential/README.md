# Valkey differential oracle — status

Bible §5 Phase 1 gate + §9: "the highest-leverage testing idea in this
project." Status: **PASS (full)** — run for real against `valkey/valkey:8`
via docker, 2026-08-05:

```
5000 commands replayed, 0 mismatches
```

## Running it

```
docker compose run --build runner
```
from this directory — brings up pg_resp + `valkey/valkey:8`, replays 5000
random T0 commands (seed 42) against both, diffs every reply. Both services
needed test-only overrides to be reachable from the `runner` container on
the compose network (loopback-bound / protected-mode services don't accept
traffic from sibling containers) — see `docker-compose.yml`'s comments and
`reports/phase1.md`'s "PRE-STEP closure" for what running this for real
actually found and fixed (Dockerfile bugs, bind-address, etc.).

Without docker, point `generate_and_compare.py` at any two already-running
RESP2 servers:

```
python3 generate_and_compare.py \
  --candidate-host 127.0.0.1 --candidate-port 6379 \
  --oracle-host    127.0.0.1 --oracle-port    6380 \
  --seed 42 --commands 5000
```

## Harness mechanics self-test (kept for regression coverage)

`mechanics_selftest.py` points both "sides" of the replay/diff engine at the
**same** pg_resp instance, clearing used keys between runs so both start
from an identical empty state — proves the RESP2 reply parser, command
builder, and diff logic are correct independent of whether a second
implementation is available. Useful as a fast sanity check before a real
docker-based run; not a substitute for one.
