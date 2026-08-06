#!/usr/bin/env bash
# STAGE A-SMOKE (box requirement 1): one ~10s throwaway cell per arm, all six.
#
# Purpose is NOT measurement. The containerised-arm measurement path has never
# executed anywhere — on the dev box memtier could not reach a single
# containerised arm (progress report D.2) — so this is the first execution of
# that code path in the project's history. SMOKE NUMBERS ARE NEVER PUBLISHED:
# they are 1-run cells, which sweep.py stamps unpublishable, written to a
# separate smoke/ directory.
#
# A 5s warm-up is included because sweep.py VOIDS a 0% hit-rate run (by design),
# and a cold 10s cell can trip that — which would look like a broken path
# instead of the proof this stage exists to produce.
set -uo pipefail
cd ~/pg_resp

export PG_MODE=container
export SERVER_CPUS=0-3
export MEMTIER_CPUSET=4-7
export RESP_PASSWORD="$(cat ~/.pg_resp_bench_password)"

OUT=bench/results/smoke
mkdir -p "$OUT"

declare -A PORTS=([P-def]=6379 [P-opt]=6379 [R-def]=6380 [R-opt]=6381 [V-opt]=6382 [K-pg]=6383)
FAILED=()

for arm in P-def P-opt K-pg; do
  echo
  echo "================ SMOKE $arm ================"
  bench/harness/arms.sh down all >/dev/null 2>&1 || true

  if ! bench/harness/arms.sh up "$arm"; then
    echo "SMOKE $arm: STAND-UP FAILED"; FAILED+=("$arm:standup"); continue
  fi
  if ! bench/harness/arms.sh exclusive "$arm"; then
    echo "SMOKE $arm: EXCLUSIVITY FAILED"; FAILED+=("$arm:exclusive"); continue
  fi

  # A.3.1 / G3 validity: prove K-pg is PostgreSQL-backed, not silently SQLite.
  if [[ "$arm" == K-pg ]]; then
    echo "--- K-pg PG-backed check (A.3.1) ---"
    before=$(bench/harness/box/psql -h 127.0.0.1 -p 28818 -U postgres -d postgres -tAc \
      "SELECT coalesce(sum(n_tup_ins),0) FROM pg_stat_user_tables" 2>/dev/null | tr -d '[:space:]')
    docker run --rm --network host pg_resp-bench-memtier:pinned memtier_benchmark \
      --host=127.0.0.1 --port=6383 --protocol=redis --ratio=1:0 --data-size=64 \
      --key-pattern=P:P --key-maximum=2000 --clients=2 --threads=1 --requests=1000 \
      --hide-histogram >/dev/null 2>&1
    after=$(bench/harness/box/psql -h 127.0.0.1 -p 28818 -U postgres -d postgres -tAc \
      "SELECT coalesce(sum(n_tup_ins),0) FROM pg_stat_user_tables" 2>/dev/null | tr -d '[:space:]')
    tables=$(bench/harness/box/psql -h 127.0.0.1 -p 28818 -U postgres -d postgres -tAc \
      "SELECT string_agg(relname||'='||n_tup_ins, ' ' ORDER BY relname) FROM pg_stat_user_tables" 2>/dev/null)
    echo "K-pg n_tup_ins: before=$before after=$after"
    echo "K-pg tables: $tables"
    if [[ -z "$after" || "$after" == "$before" ]]; then
      echo "K-pg PG-BACKED CHECK FAILED — rows did not grow; arm may be SQLite-backed"
      FAILED+=("K-pg:not-pg-backed")
    else
      echo "OK   K-pg verified PG-backed via table growth"
    fi
  fi

  auth=()
  if [[ "$arm" == P-* ]]; then
    auth=(--auth-from-pg --psql bench/harness/box/psql --pg-host 127.0.0.1 --pg-port 28818)
  fi

  python3 bench/harness/sweep.py \
    --arm "$arm" --port "${PORTS[$arm]}" \
    --data-size 1024 --pipeline 16 --clients 8 --threads 4 \
    --test-time 10 --run-count 1 --warmup-time 5 \
    --env-class dedicated \
    --client-cpus 4-7 --client-pin-mechanism docker-cpuset \
    --memtier bench/harness/box/memtier_benchmark \
    --require-exclusive \
    --out-dir "$OUT" \
    --server-note "SMOKE — never publishable" \
    "${auth[@]}"
  rc=$?
  [[ $rc -ne 0 ]] && { echo "SMOKE $arm: SWEEP rc=$rc"; FAILED+=("$arm:sweep-rc$rc"); }
done

bench/harness/arms.sh down all >/dev/null 2>&1 || true
echo
echo "================ SMOKE SUMMARY ================"
if (( ${#FAILED[@]} )); then
  echo "FAILURES: ${FAILED[*]}"
else
  echo "all six arms smoked clean"
fi
echo "### SMOKE COMPLETE"
