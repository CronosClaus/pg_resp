#!/usr/bin/env bash
# STAGE A-OFFICIAL — the D14 decision curve (ENV.md §18).
#
# Publishes the full throughput-vs-p99 curve for BOTH arms of the structural
# claim (G3): P-opt and K-pg. Identical memtier client configuration on both
# arms of every compared cell, per the D14 amendment.
#
# EVERY CELL STARTS FROM A FRESH ARM. Without this the curve is biased by run
# order: K-pg's store is a PostgreSQL table with no cache bound, so a 60s
# SET-only warm-up plus 3x60s of measurement leaves hundreds of thousands of
# rows behind, and each later cell would measure a bigger table than the one
# before it. Restarting the arm re-initdbs, so cell N and cell N+1 begin
# identically. P-opt is restarted too, so the treatment is equal.
set -uo pipefail
cd ~/pg_resp

export PG_MODE=container
export SERVER_CPUS=0-3
export MEMTIER_CPUSET=4-7
export RESP_PASSWORD="$(cat ~/.pg_resp_bench_password)"

OUT=bench/results/stage-a
mkdir -p "$OUT"

# label:data_size:pipeline:clients:threads  (clients*threads = total connections)
LADDER=(
  "d1024-p1-c1:1024:1:1:1"
  "d1024-p1-c8:1024:1:2:4"
  "d1024-p1-c64:1024:1:16:4"
  "d1024-p16-c1:1024:16:1:1"
  "d1024-p16-c8:1024:16:2:4"
  "d1024-p16-c32:1024:16:8:4"
  "d1024-p16-c64:1024:16:16:4"
  "d64-p16-c8:64:16:2:4"
  "d64-p16-c32:64:16:8:4"
)

FAILED=()
for arm in P-opt K-pg; do
  port=6379; [[ "$arm" == K-pg ]] && port=6383
  auth=()
  [[ "$arm" == P-* ]] && auth=(--auth-from-pg --psql bench/harness/box/psql --pg-host 127.0.0.1 --pg-port 28818)

  for spec in "${LADDER[@]}"; do
    IFS=: read -r label ds pl cl th <<< "$spec"
    echo
    echo "======== STAGE A  $arm  $label ========"

    bench/harness/arms.sh down all >/dev/null 2>&1 || true
    if ! bench/harness/arms.sh up "$arm" >/dev/null 2>&1; then
      echo "STAGE-A $arm $label: STAND-UP FAILED"; FAILED+=("$arm/$label:standup"); continue
    fi
    if ! bench/harness/arms.sh exclusive "$arm"; then
      echo "STAGE-A $arm $label: EXCLUSIVITY FAILED"; FAILED+=("$arm/$label:exclusive"); continue
    fi

    python3 bench/harness/sweep.py \
      --arm "$arm" --port "$port" \
      --data-size "$ds" --pipeline "$pl" --clients "$cl" --threads "$th" \
      --test-time 60 --run-count 3 --warmup-time 60 \
      --env-class dedicated \
      --client-cpus 4-7 --client-pin-mechanism docker-cpuset \
      --memtier bench/harness/box/memtier_benchmark \
      --require-exclusive --rerun-on-spread \
      --out-dir "$OUT" \
      --server-note "Stage A official; fresh arm per cell; CCX33 8 vCPU; server 0-3 client 4-7" \
      "${auth[@]}"
    rc=$?
    [[ $rc -ne 0 ]] && { echo "STAGE-A $arm $label: SWEEP rc=$rc"; FAILED+=("$arm/$label:rc$rc"); }
  done
done

bench/harness/arms.sh down all >/dev/null 2>&1 || true
echo
echo "======== STAGE A SUMMARY ========"
if (( ${#FAILED[@]} )); then echo "FAILURES: ${FAILED[*]}"; else echo "all Stage A cells completed"; fi
echo "### STAGE A COMPLETE"
