#!/usr/bin/env bash
# The 44-byte cliff probe of ENV.md §22, as a script so the before/after
# comparison across a tcp_wmem change is run identically both times.
#
# Before the change the cliff sits exactly at 16,384 B (the default send buffer):
#   16340 -> ~25,000 ops/s      16384 -> ~24.6 ops/s @ ~40.7 ms
# If raising tcp_wmem fixes it, every row here stays fast.
set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.."
export PG_MODE=container SERVER_CPUS=0-3
export RESP_PASSWORD="$(cat ~/.pg_resp_bench_password)"
PW="$RESP_PASSWORD"
echo "# tcp_wmem at probe time: $(sysctl -n net.ipv4.tcp_wmem 2>/dev/null || cat /proc/sys/net/ipv4/tcp_wmem)"
bench/harness/arms.sh down all >/dev/null 2>&1
bench/harness/arms.sh up "${1:-P-opt}" >/dev/null 2>&1
port=6379; [[ "${1:-P-opt}" == R-opt ]] && port=6381
A=(); [[ "${1:-P-opt}" == P-* ]] && A=(--authenticate="$PW")
for ds in 4096 8192 16000 16340 16384 17000 32768; do
  n=2000; [[ $ds -ge 16384 ]] && n=300
  out=$(docker run --rm --network host --cpuset-cpus 4-7 pg_resp-bench-memtier:pinned \
    memtier_benchmark --host=127.0.0.1 --port=$port "${A[@]}" --protocol=redis --ratio=1:0 \
    --key-pattern=P:P --key-maximum=100000 --hide-histogram --clients=1 --threads=1 \
    --pipeline=1 --data-size=$ds --requests=$n 2>&1 | grep -E "^Sets")
  printf "  data-size=%-6s %s\n" "$ds" "$(awk '{printf "%11.1f ops/s  %8.3f ms avg", $2, $5}' <<< "$out")"
done
bench/harness/arms.sh down all >/dev/null 2>&1
