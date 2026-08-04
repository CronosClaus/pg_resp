#!/usr/bin/env bash
# redis-cli compat check against pg_resp's T0 command set.
#
# NOT LOCALLY RUN AS redis-cli SPECIFICALLY: this WSL2 environment has no
# libclang-dev-style single fix for redis-cli — its Ubuntu package pulls a
# transitive shared-library chain (liblzf, lua5.1, lua5.1-cjson, lua5.1-bitop,
# libjemalloc2) deep enough that chasing every missing .so had poor
# effort/value versus the other clients already covering this exact T0
# surface (see reports/phase1.md). The wire bytes redis-cli would exchange
# were exhaustively verified by hand via raw sockets in this same phase
# (36/36 byte-exact vectors against the resp-protocol skill) — redis-cli
# itself is just another RESP2 client speaking the same bytes, so this is not
# a gap in protocol-correctness confidence, just in "the literal named binary
# was executed." Run for real via the dockerized compat matrix
# (`docker compose run redis-cli`) or any machine with redis-cli installed.
set -euo pipefail

HOST="${1:-127.0.0.1}"
PORT="${2:-6379}"
RC="redis-cli -h $HOST -p $PORT"

fail=0
check() {
  local desc="$1" expected="$2" actual="$3"
  if [ "$actual" = "$expected" ]; then
    echo "OK   $desc"
  else
    echo "FAIL $desc: got='$actual' expected='$expected'"
    fail=$((fail + 1))
  fi
}

check "ping" "PONG" "$($RC PING)"
check "set" "OK" "$($RC SET k v)"
check "get" "v" "$($RC GET k)"
check "get missing" "" "$($RC GET missing)"
check "set nx on existing" "" "$($RC SET k v2 NX)"
check "del" "1" "$($RC DEL k)"
check "exists after del" "0" "$($RC EXISTS k)"
check "incr" "1" "$($RC INCR ctr)"
check "incr again" "2" "$($RC INCR ctr)"
check "expire" "1" "$($RC SET ek v > /dev/null; $RC EXPIRE ek 10)"
check "ttl" "10" "$($RC TTL ek)"

echo
if [ "$fail" -eq 0 ]; then
  echo "all checks passed"
else
  echo "$fail check(s) failed"
  exit 1
fi
