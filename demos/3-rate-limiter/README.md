# Demo 3 — rate limiter (the honesty demo)

The other demos show where pg_resp wins. This one exists to find where it
**stops** winning, and to say so with a number.

A fixed-window rate limiter is the least favourable workload we can give
pg_resp: every request is a write (`INCR`), there is no read path to amortise, no
value to cache, and nothing to hit. The workload *is* the per-operation cost. If
the incumbent wins anywhere, it wins here.

The deliverable is therefore a **crossover**:

> Above roughly `X` rate-limit checks per second, keep Redis. Below it, the second
> service is not buying you anything.

Measured: **~105,000 checks/s**, where pg_resp and Redis are indistinguishable because the workload is round-trip-bound. See `ENV.md` §27 — the honest result is a bound, not a crossover.

## Run it

```bash
docker compose up -d --build pg_resp redis
docker compose run --rm limiter --target pg_resp --addr pg_resp:6379 --duration 10s
docker compose run --rm limiter --target redis   --addr redis:6379   --duration 10s
docker compose down -v
```

Identical binary, identical flags, one variable: which server answers.

## What it measures, and why one number is not enough

Each run emits a single JSON object. Two fields decide whether the rest of it
means anything.

**`checks_per_sec`** — sustained rate-limit checks per second, plus `p50`/`p99`
latency in microseconds.

**`enforcement_ok`** — whether the limiter actually limited. This is not a
formality. A rate limiter that returns "allowed" for every request is extremely
fast, and a benchmark that only measured throughput would rank a broken
implementation first. So every run derives a ceiling from its own configuration
(`budget x keys x windows touched`), asserts the observed `allowed` count does not
exceed it, and **exits non-zero if it does**. A run with `enforcement_ok: false`
is not a slow run; it is an invalid one, and its throughput must not be quoted.

Errors are treated the same way: any error during the flood invalidates the run
rather than reducing its score. A server that fails 10% of requests would
otherwise look faster than one that serves them all.

Two implementation details that are load-bearing rather than incidental:

- **`EXPIRE` is issued only when `INCR` returns 1.** Re-expiring on every request
  would silently convert the fixed window into a sliding one and make the limiter
  wrong — a bug that would not show up as an error, only as a limiter that permits
  more traffic than configured.
- **Keys rotate deterministically, not randomly.** The number of checks per key is
  then exactly derivable, which is the only reason the enforcement ceiling can be
  computed at all.

## Arm parity, stated plainly

This compose file is the *reproducible demo*, not the measurement of record. Both
containers share a host, run simultaneously, use Docker's bridge network, and are
not core-pinned, so both arms pay NAT and neither gets a clean core allocation.
That is acceptable here because the deliverable is a crossover magnitude from a
like-for-like pair paying the same tax — but it is not the §10 protocol, and a
number from this file is not interchangeable with a number from the grid.

Redis runs cache-tuned (`save ""`, `appendonly no`, `maxmemory 256mb`,
`allkeys-lru`) to match the `R-opt` arm. Benchmarking against a snapshotting Redis
would flatter pg_resp for a reason that has nothing to do with either design.

## How to read the result when it arrives

The expected and pre-registered shape is that **Redis wins this workload**. That is
not a disappointing outcome to be buried; it is the point of the demo, and bible
§0.5 requires publishing it. The useful question is not "who wins" but "at what
scale does the answer change", because most applications rate-limit at a rate far
below either server's ceiling — and for those, the interesting number is not
throughput at all but the number of services you operate.
