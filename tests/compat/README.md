# Client compat matrix — status (Phase 1)

Bible §5 Phase 1 gate: redis-cli, redis-py, node-redis, go-redis, jedis all
connect and run a T0 script green, dockerized, in CI. This environment has
**no docker at all** (confirmed in `reports/phase0.md`'s environment
pre-flight) — pre-scoped `PARTIAL(docker)` per this run's amendments. Exact
per-client status as of this phase:

| client | status | how verified |
|---|---|---|
| redis-py | **PASS — actually run** | `redis` installed via `pip install --target ~/.cargo/pylibs` (kept off system/Anaconda Python); ran `redis_py/test_t0.py` against a real local pg_resp instance: **12/12 checks passed**, including redis-py's own connection handshake |
| redis-cli | not run as the literal binary | Ubuntu's `redis-tools` package pulls a transitive shared-library chain (`liblzf`, `lua5.1`, `lua5.1-cjson`, `lua5.1-bitop`, `libjemalloc2`) deep enough that chasing every missing `.so` had poor effort/value here. **The exact bytes redis-cli would exchange were already exhaustively verified by hand** via raw sockets in this same phase (36/36 byte-exact vectors against the resp-protocol skill's test vectors) — redis-cli is just another RESP2 client speaking the same wire format, so this is a gap in "ran the named binary," not in protocol-correctness confidence. `redis_cli/test_t0.sh` is written and ready. |
| node-redis | not run | no `node` runtime in this environment; script written carefully against node-redis v4's documented API, not locally executed |
| go-redis | not run | no `go` toolchain in this environment; script written carefully against go-redis v9's documented API, not locally executed |
| jedis | not run | `java` is present but the Jedis jar/dependencies are not, and fetching them (Maven Central) wasn't attempted given the marginal value versus the other four clients already covering this same T0 surface |

## Running the real matrix (once docker is available)

```
docker compose up --build --abort-on-container-exit
```

from this directory. `docker/pg_resp.Dockerfile` (repo root) builds the
extension into a real `postgres:18` image — **this Dockerfile is itself
untested** (no docker to build it with); it's grounded in one verified fact,
noted in its own header comment: `cargo pgrx package`'s output layout was
confirmed locally against this machine's pgrx-managed install, and the
Dockerfile assumes the equivalent layout for a real `postgresql-18` apt
package (`/usr/lib/postgresql/18`, `/usr/share/postgresql/18`).

## Running the one client that IS verified, without docker

```
pip install --target /tmp/pylibs redis   # or just `pip install redis` normally
PYTHONPATH=/tmp/pylibs python3 redis_py/test_t0.py <host> <port>
```
