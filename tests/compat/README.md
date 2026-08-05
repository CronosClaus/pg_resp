# Client compat matrix — status

Bible §5 Phase 1 gate: redis-cli, redis-py, node-redis, go-redis, jedis all
connect and run a T0 script green, dockerized. **PASS (full)** — all 5 run
for real via docker, confirmed 2026-08-05:

| client | result | notes |
|---|---|---|
| redis-cli | **11/11** | official `redis:7` image, zero dependency issues |
| redis-py | **12/12** | required `protocol=2` pinned explicitly — see below |
| node-redis | **13/13** | required `RESP: 2` pinned explicitly — see below |
| go-redis | **12/12** | test script bugs fixed along the way — see below |
| jedis | **13/13** | — |

## Running it

```
docker compose up -d pg_resp   # start the server, wait for it to be healthy
docker compose run --rm <client>
```

Run each client with a fresh `pg_resp` (`docker compose restart pg_resp`
or recreate it) between runs — the store is shared in-memory state, and
different clients' scripts reuse the same literal keys (`k`, `ctr`, `ek`).
Do NOT use `--abort-on-container-exit`: it aborts the whole run on the
*first* container exit, killing the other independent one-shot clients even
when that first one passed.

## What running this for real actually found (not just "written, untested")

- **`docker/pg_resp.Dockerfile`** needed real fixes: missing
  `ca-certificates`/`pkg-config`/`libssl-dev`, and its `COPY` source paths
  were wrong — `pg_resp` is a cargo workspace member, so `cargo pgrx
  package`'s output lands in the *workspace-root* `target/`, not
  `crates/pg_resp/target/`. Full detail in `reports/phase1.md`.
- **`pg_resp.bind_address` defaults to 127.0.0.1** (the real, correct
  production security default) — which means a container running the image
  as-is is unreachable from sibling containers on the compose network
  (loopback binds don't accept traffic via `eth0`). `docker-compose.yml`
  overrides this with `postgres -c pg_resp.bind_address=0.0.0.0`,
  commented as test-harness-only, not a deployment recommendation.
- **redis-py and node-redis both default to attempting RESP3** and do not
  gracefully fall back to RESP2 on any error reply to `HELLO` (traced
  through both libraries' actual source to confirm). Since bible D9 forbids
  implementing real RESP3 in v0.1, the fix is client-side: both scripts now
  pin RESP2 explicitly (`protocol=2` / `{ RESP: 2 }`), which is each
  library's documented, correct way to talk to a RESP2-only server.
- go-redis's script had two of its own bugs (not pg_resp bugs): its
  `SetNX()` method sends the legacy `SETNX` command (T2 tier, not yet
  built) instead of `SET ... NX` (T0) — fixed to use `SetArgs{Mode: "NX"}`;
  and `Expire(ctx, key, 10)` passed a bare `10` where the signature wants a
  `time.Duration`, silently truncated to 1 second instead of 10.
- node-redis's `client.quit()` sends the wire `QUIT` command (T1, not yet
  built) — fixed to `client.destroy()` (plain socket close, no wire command).

Full narrative in `reports/phase1.md`'s "PRE-STEP closure" section.
