#!/usr/bin/env bash
# Stand up, verify, and tear down the six benchmark arms of bible §10.
#
# WHY EVERY CONTAINER USES --network host
# ---------------------------------------
# pg_resp is a PostgreSQL background worker: it runs natively and is reached
# over the host's loopback interface. If the incumbent arms ran under Docker's
# default bridge network, every one of their operations would additionally pay
# NAT and (depending on the daemon) the userland proxy — a per-round-trip cost
# that has nothing to do with the server being measured. At pipeline 1, where
# throughput is round-trip-bound, that alone can move a result by a wide margin,
# and it would move it in pg_resp's favour, which is the direction this project
# must be most suspicious of (bible §0.5).
#
# So every containerised arm runs with --network host and its own port. All six
# arms are then reached identically: host loopback, no NAT, no proxy. This is
# recorded in ENV.md because it is a real methodological choice, not a detail.
#
# HOW THAT INTERACTS WITH THE BOX'S LOOPBACK-ONLY REQUIREMENT
# -----------------------------------------------------------
# Phase 4 box requirement 2 asks that every published port bind loopback only.
# With --network host there is no docker -p publishing to constrain: the
# container is IN the host's network namespace, so the bind address is decided
# by each server's own configuration. That is where it is set —
# `bind 127.0.0.1` in the redis/valkey configs, `-h 127.0.0.1` for redka,
# `listen_addresses` for PostgreSQL, and pg_resp.bind_address's 127.0.0.1
# default. `arms.sh lockdown` then verifies the end state the requirement
# actually cares about: nothing listening on 0.0.0.0/:: except sshd.
#
# Using `-p 127.0.0.1:PORT:PORT` instead would force bridge networking back on
# and reintroduce exactly the NAT + userland-proxy per-round-trip cost this
# file exists to avoid — moving results in pg_resp's favour, the direction
# bible §0.5 requires the most suspicion of. Same guarantee, no confound.
#
# PORT MAP (all on 127.0.0.1)
#   6379  P-def / P-opt   pg_resp inside the PostgreSQL instance (native)
#   6380  R-def           Redis 8.2, stock config
#   6381  R-opt           Redis 8.2, cache-tuned
#   6382  V-opt           Valkey 8.1, cache-tuned
#   6383  K-pg            Redka against the same PostgreSQL instance
#
# Usage:
#   bench/harness/arms.sh up   <arm>        # start it and verify it serves
#   bench/harness/arms.sh verify <arm>      # verify only
#   bench/harness/arms.sh down <arm>|all    # stop and remove
#   bench/harness/arms.sh build-redka       # build the K-pg image from ref/redka
#   bench/harness/arms.sh digests           # print pinned images + digests
#   bench/harness/arms.sh lockdown          # prove nothing listens off-loopback
#
# TWO HOST MODES
#   PG_MODE=native     (default) PostgreSQL runs on the host, as on the dev box.
#   PG_MODE=container  PostgreSQL runs from PG_IMAGE. Required on the official
#                      bench box, which is frozen as bootstrapped (docker, git,
#                      python3, tmux, rsync — no compiler, no PG, box
#                      requirement 5), so a native PostgreSQL cannot be built
#                      there at all. The image is docker/pg_resp.Dockerfile's
#                      output, i.e. the same artifact W9 ships and G1 measures.
#
# Env: RESP_PASSWORD (P-* auth), SERVER_CPUS (e.g. 0-3 — pins every server
# container's cpuset; the client is pinned separately by sweep.py), PG_IMAGE.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CONFIGS="$REPO/bench/configs"

REDIS_IMAGE="redis:8.2-alpine"
VALKEY_IMAGE="valkey/valkey:8.1-alpine"
REDKA_IMAGE="pg_resp-bench-redka:pinned"

# The PostgreSQL instance both P-* and K-pg use. K-pg pointing at the SAME
# instance is the entire point of the arm: it isolates architecture
# (SQL-translation vs in-process) instead of comparing two different databases.
PG_PORT="${PG_PORT:-28818}"
PG_MODE="${PG_MODE:-native}"
PG_IMAGE="${PG_IMAGE:-pg_resp:0.1.0-rc}"
PG_CONTAINER="pg_resp_bench_pg"
# In container mode the superuser is the image's own `postgres`, not the invoking
# host user — getting this wrong yields a redka DSN that cannot authenticate,
# and redka's response to an unusable DSN is a SILENT SQLITE FALLBACK that
# answers every RESP command correctly (ENV.md §6 / progress report D.8). That
# is the failure this default exists to avoid.
if [[ "$PG_MODE" == container ]]; then
  PG_USER="${PG_USER:-postgres}"
else
  PG_USER="${PG_USER:-$(id -un)}"
fi
PG_DB="${PG_DB:-postgres}"
SERVER_CPUS="${SERVER_CPUS:-}"

# Applied to every server container so the server side stays on its own
# physical cores (ENV.md §9). Empty = unpinned, same as the dev box.
cpuset_args() {
  [[ -n "$SERVER_CPUS" ]] && printf '%s\n%s\n' --cpuset-cpus "$SERVER_CPUS"
}

# PostgreSQL's `include` is a config-file directive, not something -c can set,
# and the postgres image's entrypoint owns postgresql.conf. So the arm's conf
# file is translated into explicit `-c key=value` postmaster arguments: the
# conf files stay the single source of truth, and the settings that actually
# reach the server are visible in `docker inspect` rather than buried in a
# mount. Comments and quotes are stripped; nothing else is interpreted.
pg_conf_args() {
  local conf="$1"
  sed -e 's/#.*$//' -e '/^[[:space:]]*$/d' "$conf" \
    | sed -e "s/^[[:space:]]*//" -e "s/[[:space:]]*=[[:space:]]*/=/" -e "s/'//g" \
    | while IFS= read -r kv; do [[ -n "$kv" ]] && printf '%s\n%s\n' -c "$kv"; done
}

port_for() {
  case "$1" in
    P-def|P-opt) echo 6379 ;;
    R-def)       echo 6380 ;;
    R-opt)       echo 6381 ;;
    V-opt)       echo 6382 ;;
    K-pg)        echo 6383 ;;
    *) echo "unknown arm: $1" >&2; return 1 ;;
  esac
}

container_for() { echo "pg_resp_bench_${1//-/_}"; }

# This machine has no host redis-cli (the compat matrix has always used a
# containerised one). Prefer a real host binary when present; otherwise borrow
# the client out of the pinned Redis image. --network host keeps it reaching the
# same loopback every arm is measured over.
redis_cli_cmd() {
  if command -v redis-cli >/dev/null 2>&1; then
    echo "redis-cli"
  else
    echo "docker run --rm --network host $REDIS_IMAGE redis-cli"
  fi
}

# Prove the arm executes a real command. Same discipline as sweep.py's probe,
# duplicated here on purpose so `up` cannot report success for a server that
# answers PING but cannot serve a GET.
verify_arm() {
  local arm="$1" port
  port="$(port_for "$arm")"
  local key="pg_resp_arm_probe_${arm}"
  local val="probe-ok-${arm}"
  local cli
  read -r -a cli <<< "$(redis_cli_cmd)"
  cli+=(-h 127.0.0.1 -p "$port")
  if [[ -n "${RESP_PASSWORD:-}" && "$arm" == P-* ]]; then
    cli+=(-a "$RESP_PASSWORD" --no-auth-warning)
  fi

  local out
  if ! out="$("${cli[@]}" PING 2>&1)"; then
    echo "FAIL $arm: no response on 127.0.0.1:$port ($out)" >&2
    return 1
  fi
  case "$out" in
    *NOAUTH*|*WRONGPASS*)
      echo "FAIL $arm: authentication refused ($out) — set RESP_PASSWORD" >&2
      return 1 ;;
  esac

  "${cli[@]}" SET "$key" "$val" > /dev/null
  local got
  got="$("${cli[@]}" --no-raw GET "$key" 2>&1)"
  if [[ "$got" != *"$val"* ]]; then
    echo "FAIL $arm: SET/GET did not round-trip (got: $got)" >&2
    return 1
  fi
  "${cli[@]}" DEL "$key" > /dev/null
  # Valkey reports BOTH a `valkey_version` and a compatibility `redis_version`
  # (7.2.4 on Valkey 8.1.9), so prefer the former — recording the compat field
  # would put a wrong version number in ENV.md.
  local info ver
  info="$("${cli[@]}" INFO server 2>/dev/null | tr -d '\r' || true)"
  ver="$(grep -iE '^valkey_version' <<< "$info" | head -1 || true)"
  [[ -z "$ver" ]] && ver="$(grep -iE '^(redis_version|server_version)' <<< "$info" | head -1 || true)"
  echo "OK   $arm on :$port — SET/GET round-tripped${ver:+ — $ver}"
}

build_redka() {
  # Built from the pinned reference clone (docs/refs/PINS.md: d3c353f02470)
  # using redka's own Dockerfile, so the arm is upstream's build, not ours.
  echo "building $REDKA_IMAGE from ref/redka (pinned d3c353f02470)"
  docker build -q -t "$REDKA_IMAGE" "$REPO/ref/redka"
  docker run --rm --entrypoint redka "$REDKA_IMAGE" -h 2>&1 | head -3 || true
}

# Start (or restart) the PostgreSQL container for an arm, with that arm's conf.
# The data directory is deliberately NOT a volume: the cache is ephemeral by
# design (D5), and a fresh initdb per arm means no arm inherits the previous
# arm's page cache or bloat. It costs a few seconds and removes a confound.
pg_up() {
  local arm="$1" conf="$2"
  local -a args
  mapfile -t args < <(cpuset_args)
  local -a conf_args
  mapfile -t conf_args < <(pg_conf_args "$conf")

  docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
  # POSTGRES_HOST_AUTH_METHOD=trust: the box is loopback-only behind an edge
  # firewall that permits TCP 22 alone, and PostgreSQL here holds nothing but an
  # ephemeral benchmark keyspace. The RESP surface is separately authenticated
  # via pg_resp.password below, which is what the arms are measured through.
  docker run -d --name "$PG_CONTAINER" --network host "${args[@]}" \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    "$PG_IMAGE" \
    -c "listen_addresses=127.0.0.1" -c "port=$PG_PORT" \
    "${conf_args[@]}" \
    ${RESP_PASSWORD:+-c "pg_resp.password=$RESP_PASSWORD"} >/dev/null

  # P-* readiness is the RESP port, not PostgreSQL's — the bgworker comes up
  # after the postmaster. For K-pg's instance there is no RESP port, so the
  # caller waits on PostgreSQL instead.
  if [[ "$arm" == P-* ]]; then
    for _ in $(seq 1 60); do
      if verify_arm "$arm" 2>/dev/null; then return 0; fi
      sleep 0.5
    done
    echo "FAIL $arm: PostgreSQL container never served RESP on :$(port_for "$arm")" >&2
    docker logs --tail 40 "$PG_CONTAINER" >&2 || true
    return 1
  fi

  for _ in $(seq 1 60); do
    if docker exec "$PG_CONTAINER" pg_isready -h 127.0.0.1 -p "$PG_PORT" -q 2>/dev/null; then
      echo "OK   PostgreSQL up for $arm ($(basename "$conf"))"; return 0
    fi
    sleep 0.5
  done
  echo "FAIL PostgreSQL container never became ready for $arm" >&2
  docker logs --tail 40 "$PG_CONTAINER" >&2 || true
  return 1
}

up_arm() {
  local arm="$1" port name
  port="$(port_for "$arm")"
  name="$(container_for "$arm")"

  case "$arm" in
    P-def|P-opt)
      local conf
      conf="$CONFIGS/$([[ $arm == P-def ]] && echo pg-default.conf || echo pg-tuned.conf)"
      if [[ "$PG_MODE" != container ]]; then
        cat >&2 <<EOF
$arm is not a container in PG_MODE=native. Configure the PostgreSQL instance
with $conf, restart it, then: bench/harness/arms.sh verify $arm
EOF
        return 2
      fi
      pg_up "$arm" "$conf" ;;
    R-def)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host $(cpuset_args | tr '\n' ' ') \
        -v "$CONFIGS/redis-default.conf:/etc/redis.conf:ro" \
        "$REDIS_IMAGE" redis-server /etc/redis.conf --port "$port" >/dev/null ;;
    R-opt)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host $(cpuset_args | tr '\n' ' ') \
        -v "$CONFIGS/redis-cache.conf:/etc/redis.conf:ro" \
        "$REDIS_IMAGE" redis-server /etc/redis.conf --port "$port" >/dev/null ;;
    V-opt)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host $(cpuset_args | tr '\n' ' ') \
        -v "$CONFIGS/valkey-cache.conf:/etc/valkey.conf:ro" \
        "$VALKEY_IMAGE" valkey-server /etc/valkey.conf --port "$port" >/dev/null ;;
    K-pg)
      docker image inspect "$REDKA_IMAGE" >/dev/null 2>&1 || build_redka
      # K-pg's PostgreSQL is configured in the competitor's favour (A.3.2) and
      # deliberately does NOT load pg_resp — see pg-kpg.conf's closing note.
      if [[ "$PG_MODE" == container ]]; then
        pg_up K-pg "$CONFIGS/pg-kpg.conf"
      fi
      docker rm -f "$name" >/dev/null 2>&1 || true
      # Redka takes its data source as a positional argument.
      # -h 127.0.0.1, not 0.0.0.0: with --network host that bind address is the
      # host's, so 0.0.0.0 would publish the arm to the internet (box
      # requirement 2).
      docker run -d --name "$name" --network host $(cpuset_args | tr '\n' ' ') \
        --entrypoint redka "$REDKA_IMAGE" \
        -h 127.0.0.1 -p "$port" \
        "postgres://${PG_USER}@127.0.0.1:${PG_PORT}/${PG_DB}?sslmode=disable" >/dev/null ;;
  esac

  # Give the server a moment, then insist it actually serves.
  for _ in $(seq 1 20); do
    if verify_arm "$arm" 2>/dev/null; then return 0; fi
    sleep 0.5
  done
  echo "FAIL $arm did not become ready; container logs:" >&2
  docker logs --tail 30 "$name" >&2 || true
  return 1
}

down_arm() {
  if [[ "$1" == all ]]; then
    for a in R-def R-opt V-opt K-pg; do docker rm -f "$(container_for "$a")" >/dev/null 2>&1 || true; done
    docker rm -f "$PG_CONTAINER" >/dev/null 2>&1 || true
    echo "all containerised arms removed"
  else
    docker rm -f "$(container_for "$1")" >/dev/null 2>&1 || true
    echo "removed $(container_for "$1")"
  fi
}

# ENV.md §8: exactly one arm live per measured run.
exclusive() {
  local keep="$1" a
  for a in R-def R-opt V-opt K-pg; do
    [[ "$a" == "$keep" ]] && continue
    docker rm -f "$(container_for "$a")" >/dev/null 2>&1 || true
  done
  # K-pg drags a PostgreSQL and a network along with it.
  if [[ "$keep" != K-pg ]]; then
    docker rm -f kpg_redka kpg_pg >/dev/null 2>&1 || true
  fi
  echo "stopped every arm except $keep"
  # Then prove it, rather than assuming the stops worked.
  local port live=()
  for a in P-def R-def R-opt V-opt K-pg; do
    port="$(port_for "$a")"
    [[ "$port" == "$(port_for "$keep")" ]] && continue
    if timeout 1 bash -c "</dev/tcp/127.0.0.1/$port" 2>/dev/null; then live+=("$a:$port"); fi
  done
  if (( ${#live[@]} )); then
    echo "FAIL still answering: ${live[*]}" >&2
    return 1
  fi

  # In container mode there is exactly one PostgreSQL container serving whichever
  # arm is under test, and its postmaster arguments say which arm that is. A port
  # probe cannot see this: if K-pg were measured against a PostgreSQL that still
  # had shared_preload_libraries='pg_resp.so' from the previous arm, pg_resp's
  # background worker would be burning server cores throughout K-pg's run — an
  # exclusivity violation entirely inside another arm's process tree, and one
  # that would make K-pg look worse, i.e. flatter pg_resp's structural claim.
  # So assert the config matches the arm.
  if [[ "$PG_MODE" == container ]] && docker inspect "$PG_CONTAINER" >/dev/null 2>&1; then
    local pgargs
    pgargs="$(docker inspect --format '{{join .Args " "}}' "$PG_CONTAINER" 2>/dev/null || true)"
    case "$keep" in
      P-*)
        if [[ "$pgargs" != *pg_resp.so* ]]; then
          echo "FAIL $keep: PostgreSQL container is not loading pg_resp.so (args: $pgargs)" >&2
          return 1
        fi ;;
      K-pg)
        if [[ "$pgargs" == *pg_resp.so* ]]; then
          echo "FAIL K-pg: PostgreSQL container still loads pg_resp.so — pg_resp's" \
               "bgworker would run through K-pg's entire measurement (args: $pgargs)" >&2
          return 1
        fi ;;
    esac
    echo "OK   PostgreSQL container config matches $keep"
  fi
  echo "OK   only $keep is live"
}

# Box requirement 2: prove the end state, do not assume the config took effect.
# Anything listening on 0.0.0.0 or :: other than sshd is a finding, and this
# output is pasted into ENV.md verbatim.
lockdown() {
  echo "# ss -tlnp on $(hostname) at $(date -u +%FT%TZ)"
  ss -tlnp || true
  echo
  local bad
  bad="$(ss -tlnH | awk '{print $4}' | grep -E '^(0\.0\.0\.0|\[?::\]?):' || true)"
  local offenders=""
  while IFS= read -r a; do
    [[ -z "$a" ]] && continue
    # sshd on :22 is the one permitted exception (it is also the only port the
    # Hetzner edge firewall admits).
    [[ "$a" == *":22" ]] && continue
    offenders+="$a "
  done <<< "$bad"
  if [[ -n "$offenders" ]]; then
    echo "FAIL listening off-loopback on: $offenders" >&2
    return 1
  fi
  echo "OK   nothing listens on 0.0.0.0/:: except sshd on :22"
}

digests() {
  for img in "$REDIS_IMAGE" "$VALKEY_IMAGE"; do
    docker image inspect --format '{{.RepoTags}} {{index .RepoDigests 0}}' "$img" 2>/dev/null \
      || echo "$img not pulled"
  done
  docker image inspect --format '{{.RepoTags}} (built locally from ref/redka d3c353f02470)' \
    "$REDKA_IMAGE" 2>/dev/null || echo "$REDKA_IMAGE not built"
  docker image inspect --format '{{.RepoTags}} (built locally from ref/memtier_benchmark 272eeb647df5)' \
    pg_resp-bench-memtier:pinned 2>/dev/null || echo "memtier image not built"
  if [[ "$PG_MODE" == container ]]; then
    docker image inspect --format '{{.RepoTags}} id={{.Id}} (docker/pg_resp.Dockerfile on postgres:18)' \
      "$PG_IMAGE" 2>/dev/null || echo "$PG_IMAGE not built"
  fi
}

cmd="${1:-}"; shift || true
case "$cmd" in
  up)          up_arm "$1" ;;
  verify)      verify_arm "$1" ;;
  down)        down_arm "${1:-all}" ;;
  exclusive)   exclusive "$1" ;;
  build-redka) build_redka ;;
  digests)     digests ;;
  lockdown)    lockdown ;;
  *) sed -n '1,60p' "${BASH_SOURCE[0]}"; exit 1 ;;
esac
