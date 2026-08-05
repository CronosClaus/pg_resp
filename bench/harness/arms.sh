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
PG_USER="${PG_USER:-$(id -un)}"
PG_DB="${PG_DB:-postgres}"

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

up_arm() {
  local arm="$1" port name
  port="$(port_for "$arm")"
  name="$(container_for "$arm")"

  case "$arm" in
    P-def|P-opt)
      cat >&2 <<EOF
$arm is not a container. Configure the PostgreSQL instance with
  bench/configs/$([[ $arm == P-def ]] && echo pg-default.conf || echo pg-tuned.conf)
restart it, then: bench/harness/arms.sh verify $arm
EOF
      return 2 ;;
    R-def)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host \
        -v "$CONFIGS/redis-default.conf:/etc/redis.conf:ro" \
        "$REDIS_IMAGE" redis-server /etc/redis.conf --port "$port" >/dev/null ;;
    R-opt)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host \
        -v "$CONFIGS/redis-cache.conf:/etc/redis.conf:ro" \
        "$REDIS_IMAGE" redis-server /etc/redis.conf --port "$port" >/dev/null ;;
    V-opt)
      docker rm -f "$name" >/dev/null 2>&1 || true
      docker run -d --name "$name" --network host \
        -v "$CONFIGS/valkey-cache.conf:/etc/valkey.conf:ro" \
        "$VALKEY_IMAGE" valkey-server /etc/valkey.conf --port "$port" >/dev/null ;;
    K-pg)
      docker image inspect "$REDKA_IMAGE" >/dev/null 2>&1 || build_redka
      docker rm -f "$name" >/dev/null 2>&1 || true
      # Redka takes its data source as a positional argument.
      docker run -d --name "$name" --network host \
        --entrypoint redka "$REDKA_IMAGE" \
        -h 0.0.0.0 -p "$port" \
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
  echo "OK   only $keep is live"
}

digests() {
  for img in "$REDIS_IMAGE" "$VALKEY_IMAGE"; do
    docker image inspect --format '{{.RepoTags}} {{index .RepoDigests 0}}' "$img" 2>/dev/null \
      || echo "$img not pulled"
  done
  docker image inspect --format '{{.RepoTags}} (built locally from ref/redka d3c353f02470)' \
    "$REDKA_IMAGE" 2>/dev/null || echo "$REDKA_IMAGE not built"
}

cmd="${1:-}"; shift || true
case "$cmd" in
  up)          up_arm "$1" ;;
  verify)      verify_arm "$1" ;;
  down)        down_arm "${1:-all}" ;;
  exclusive)   exclusive "$1" ;;
  build-redka) build_redka ;;
  digests)     digests ;;
  *) sed -n '1,40p' "${BASH_SOURCE[0]}"; exit 1 ;;
esac
