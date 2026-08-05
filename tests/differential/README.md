# Valkey differential oracle — status

Bible §5 Phase 1 gate + §9: "the highest-leverage testing idea in this
project." Status: **PASS (full)** — run for real against `valkey/valkey:8`
via docker, 2026-08-05, covering T0:

```
5000 commands replayed, 0 mismatches
```

**Phase 2 END-STEP** extended `random_t0_stream()` to also generate the new
T1/T2 commands (SETEX, SETNX, GETDEL, GETEX, PERSIST, TYPE, DBSIZE, KEYS) —
new commands don't ship oracle-unchecked, same standard as T0. Re-run for
real against `valkey/valkey:8`, 2026-08-05, three seeds, 15000 commands
total, 0 mismatches:

```
seed 42:  5000 commands replayed, 0 mismatches
seed 123: 5000 commands replayed, 0 mismatches
seed 999: 5000 commands replayed, 0 mismatches
```

KEYS replies are compared order-independently (`normalize_for_compare`) since
hash iteration order differs between pg_resp and Valkey even for an
identical key set — not a divergence. Deliberately excluded from generation:
SCAN (bible D10 — pg_resp's cursor design is its own, not meant to match
Valkey's cursor encoding), RANDOMKEY (non-deterministic across
implementations), and connection-lifecycle commands (AUTH/HELLO/CLIENT/
COMMAND/QUIT/SELECT/FLUSHDB) that would invalidate the replay's key-state
assumptions mid-stream.

## Adversarial coverage: `nasty_deck.py`

`random_t0_stream()` above is *randomized*, not *adversarial* — it draws
well-formed commands from a small ASCII key/value pool and never probes
integer boundaries, binary payloads, wrong-arity errors, or option-syntax
edge cases. `nasty_deck.py` is a fixed, hand-written complement: 121 specific
adversarial commands (SET option combos incl. EX+NX+GET/KEEPTTL, INCR/DECR
at i64::MAX/MIN, EXPIRE 0/negative, binary-safe CRLF/NUL/full-byte-range
keys and values, empty keys/values, wrong-arity errors, GETEX-bare-call vs
PERSIST TTL semantics, KEYS/SCAN MATCH glob edge patterns including
malformed ones). Run for real against `valkey/valkey:8`, 2026-08-05, after a
harness self-consistency pre-check (both sides pointed at the same pg_resp
instance, 0 mismatches expected trivially and confirmed):

```
self-consistency check: 121 nasty-deck commands replayed, 0 mismatches
real run (pg_resp vs valkey): 121 nasty-deck commands replayed, 0 mismatches
```

Notably, even the deliberately malformed glob patterns (unterminated class
`glob:[ab`, dangling backslash `glob:a\`) matched Valkey byte-for-byte —
pg_resp's from-scratch matcher (bible D8: written from the spec digest,
never copied) happens to be lenient in the same way Valkey's is. Full detail
in `reports/phase2.md`.

## Running it

```
docker compose run --build runner
```
from this directory — brings up pg_resp + `valkey/valkey:8`, replays 5000
random T0+T1+T2 commands (seed 42) against both, diffs every reply. Both services
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
