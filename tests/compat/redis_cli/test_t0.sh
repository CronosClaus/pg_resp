#!/usr/bin/env bash
# redis-cli compat check against pg_resp's T0/T1/T2 command set.
#
# VERIFIED FOR REAL via the dockerized compat matrix (bible §5 Phase 1 gate,
# extended to T1/T2 in Phase 2's END-STEP): 11/11 T0 checks passed using the
# official redis:7 image's redis-cli, no dependency issues at all (this
# machine's own Ubuntu redis-tools package has a deep transitive
# shared-library chain that made running the literal binary locally
# impractical — see reports/phase1.md — but that's irrelevant once running
# inside the redis:7 container image).
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

# --- T1/T2 (bible §3.4, phase 2) ---
check "dbsize before flush nonzero" "not-empty" "$([ "$($RC DBSIZE)" != "0" ] && echo not-empty || echo empty)"
check "flushdb" "OK" "$($RC FLUSHDB)"
check "dbsize after flush" "0" "$($RC DBSIZE)"
check "setex" "OK" "$($RC SETEX sk 50 v)"
check "setex ttl" "50" "$($RC TTL sk)"
check "setnx new" "1" "$($RC SETNX nk v1)"
check "setnx existing" "0" "$($RC SETNX nk v2)"
$RC SET gk v > /dev/null
check "getdel" "v" "$($RC GETDEL gk)"
check "getdel removed key" "" "$($RC GET gk)"
$RC SET gek v EX 100 > /dev/null
check "getex persist" "v" "$($RC GETEX gek PERSIST)"
check "ttl after getex persist" "-1" "$($RC TTL gek)"
$RC SET pk v EX 10 > /dev/null
check "persist" "1" "$($RC PERSIST pk)"
check "ttl after persist" "-1" "$($RC TTL pk)"
check "type string" "string" "$($RC TYPE pk)"
check "type none" "none" "$($RC TYPE missingkey)"
$RC SET "user:1" a > /dev/null
$RC SET "user:2" b > /dev/null
check "keys pattern count" "2" "$($RC KEYS 'user:*' | wc -l)"
check "info has used_memory" "yes" "$($RC INFO | grep -q used_memory && echo yes || echo no)"

echo
if [ "$fail" -eq 0 ]; then
  echo "all checks passed"
else
  echo "$fail check(s) failed"
  exit 1
fi
