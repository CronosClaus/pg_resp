# Release notes for the GitHub Release object — DRAFT, do not publish

Paste as the body of the `v0.1.0` Release. The tag already exists; this is the
Release object's text.

---

**A Redis-protocol (RESP2) cache server that runs inside a PostgreSQL background
worker.** `redis-cli` connects to your database and it answers. One fewer service to
run, patch, monitor and page someone about.

```bash
docker run -d --name pg_resp \
  -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:6379:6379 -p 127.0.0.1:5432:5432 \
  ghcr.io/cronosclaus/pg_resp:0.1.0 \
  -c pg_resp.bind_address=0.0.0.0

redis-cli -h 127.0.0.1 -p 6379 SET greeting hello
```

Measured on a fresh host: **14 seconds** from nothing to a served `GET`, in three
commands.

## What it is

In-process — not a proxy, not a sidecar, not a layer that turns every `GET` into a
`SELECT`. The cache lives in the background worker's own heap. Strings, TTLs,
CLOCK-LRU eviction, a hard memory cap, `AUTH`, and a `resp.*` SQL surface.
PostgreSQL 16, 17 and 18, all CI-tested.

The part only an in-database cache can do:

```sql
CREATE TRIGGER invalidate_price AFTER UPDATE ON products
  FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
```

Application-side invalidation has *unbounded* staleness — when one write path
forgets, the cache serves stale until somebody notices. Measured under a write
storm: **493 stale serves were still stale when observation stopped at 10 s**
against the trigger arm's **0**.

## Performance, honestly

**It is not faster than Redis, and the numbers say so.** At 1 KB with pipelining,
Valkey is +19% and Redis +17%. Across 24 paired comparisons: pg_resp faster in 8,
an incumbent faster in 2, 14 within noise — and both incumbents run
single-threaded by default here, while the same Redis given `io-threads 4` reaches
**1.79× pg_resp**. The honest summary: *comparable to a single-threaded Redis at
these payloads, and beaten by a Redis you let use more cores.*

Where the structural claim lives: **≥ 8.1× Redka-on-PostgreSQL at worst**, decided
by the weakest measured cell rather than the best, rising to 111.7× under load.
Memory: **1,210 bytes per entry vs 1,345 (Redis) and 1,353 (Valkey)** — ~10% leaner,
against an allocator handicap.

Full record, including every caveat that runs in our own favour, the cells that
failed, and the conclusions we withdrew: [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)
and [`bench/results/ENV.md`](bench/results/ENV.md). Every table regenerates from
committed raw artifacts.

## Do not use it if

You cannot restart PostgreSQL or grant superuser · you need the cache to survive a
restart · you need more than strings and TTLs · you need isolation between tenants ·
you need more throughput than one core, or the cache reachable off-box (no TLS in
0.1). Details in the README.

## Method

Developed agent-assisted (Claude Code) against a phased plan with hard gates. The
plan, the phase reports, the benchmark methodology and the failures are all public
in this repository — including the measurements that were withdrawn and the
conclusions that turned out to be wrong.

## Verifying what you install

PGXN source distribution `pg_resp-0.1.0.zip`, sha256:

```
18941dfcd354b6778f53f26bbe315b1f03b6f38da1396a26c0c643aef9a783a1
```

Rebuild it yourself from the tag with `make pgxn-dist` — the recipe is reproducible
and the hash is committed at `docs/launch/DIST-SHA256.txt`, so the uploaded artifact
verifies against this repository rather than against a claim.

Container image `ghcr.io/cronosclaus/pg_resp:0.1.0`, digest
`sha256:15b477b8c21afd4d5c00abf95b83c54875d0477b144573ed61e92fb23ae815c6`.

`0.1.0-rc` is superseded; it remains published because its digest is cited in the G1
measurement record.
