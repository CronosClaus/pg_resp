#!/usr/bin/env bash
# STAGE B — the full approved grid of bible §10, warm-up v2.
#
#   6 arms x 18 workloads = 108 cells
#   value sizes 64 / 1024 / 16384  x  pipeline 1 / 16  x  connections 1 / 8 / 64
#   each cell 3 x 60 s, spread-gated at 8%, one arm live at a time
#
# Plus two labelled side sets that never mix into the ranked grid:
#   grid-anomaly/        the 64 B peak comparison with --require-saturation
#                        enforced (ENV.md §21.3)
#   grid-supplementary/  the same cell with Redis --io-threads 4, a robustness
#                        check and NOT a ranked arm
#
# DESIGNED TO SURVIVE THE SESSION THAT LAUNCHED IT
# ------------------------------------------------
# Everything is written to disk as it happens: raw files and headers by sweep.py,
# a one-line-per-cell progress record here, and a full log via tee. Nothing
# depends on an SSH connection staying up or on any agent session being alive.
#
# It is also IDEMPOTENT PER CELL: a cell whose .json artifact already exists is
# skipped. So relaunching after an interruption resumes rather than redoing, and
# relaunching after completion is a no-op.
#
# HARD STOP: if more than 20% of attempted cells VOID (once at least 10 have been
# attempted), the grid stops and writes a STOP marker rather than grinding through
# 100 more cells of a broken protocol. Diagnosis happens in
# reports/phase4-night2.md, the box is left up, and the protocol is NOT changed
# on the fly.
set -uo pipefail
cd ~/pg_resp

export PG_MODE=container
export SERVER_CPUS=0-3
export MEMTIER_CPUSET=4-7
export RESP_PASSWORD="$(cat ~/.pg_resp_bench_password)"

GRID=bench/results/grid
ANOM=bench/results/grid-anomaly
SUPP=bench/results/grid-supplementary
PROGRESS=~/logs/grid-progress.tsv
STOPFILE=~/logs/grid-STOPPED
mkdir -p "$GRID" "$ANOM" "$SUPP" ~/logs
[[ -f "$PROGRESS" ]] || printf 'utc\tarm\tworkload\tstatus\tops_sec\tp99_ms\thit_pct\tspread_pct\tpublishable\tcpu_peak\n' > "$PROGRESS"

WARMUP_KEYS=200000
ARMS_ALL=(P-def P-opt R-def R-opt V-opt K-pg)
SIZES=(64 1024 16384)
PIPES=(1 16)
CONNS=(1 8 64)

ATTEMPTED=0
VOIDED=0

port_for() {
  case "$1" in
    P-def|P-opt) echo 6379 ;; R-def) echo 6380 ;; R-opt) echo 6381 ;;
    V-opt) echo 6382 ;; K-pg) echo 6383 ;;
  esac
}

# --clients is per-thread and they multiply. Threads capped at 4 because the
# client side owns exactly 4 logical CPUs (ENV.md §16).
ct_for() {
  case "$1" in
    1) echo "1 1" ;;
    8) echo "2 4" ;;
    64) echo "16 4" ;;
    *) echo "1 1" ;;
  esac
}

# Find a cell's artifact by arm+workload, NOT by today's date.
#
# sweep.py names files <UTC-date>-<arm>-<workload>.json, and this grid runs for
# ~6.5 h starting in the evening, so it crosses midnight UTC. Keying on
# `date -u +%F` would mean that after midnight every cell completed before
# midnight stopped being recognised: already-done cells would be re-run, and
# record() would report blank metrics for cells that had just succeeded. Glob
# instead, newest last.
artifact_for() { # outdir arm workload
  ls -1 "$1"/*-"$2"-"$3".json 2>/dev/null | tail -1
}

record() { # arm workload status outdir
  local arm="$1" wl="$2" status="$3" dir="$4"
  local json; json="$(artifact_for "$dir" "$arm" "$wl")"
  local ops="" p99="" hit="" spread="" pub="" cpu=""
  if [[ -n "$json" && -f "$json" ]]; then
    read -r ops p99 hit spread pub cpu < <(python3 - "$json" <<'PY'
import json,sys
c=json.load(open(sys.argv[1]))
sp=c.get("spread_pct")
print(f'{c.get("ops_sec","")} {c.get("p99","")} {c.get("hit_rate_pct","")} '
      f'{sp if sp is not None else ""} {c.get("publishable","")} {c.get("cpu_peak_pct","")}')
PY
)
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$(date -u +%FT%TZ)" "$arm" "$wl" "$status" "$ops" "$p99" "$hit" "$spread" "$pub" "$cpu" >> "$PROGRESS"
}

# Hard stop: a systemically broken protocol should not consume the night.
check_stop() {
  (( ATTEMPTED >= 10 )) || return 0
  local pct=$(( VOIDED * 100 / ATTEMPTED ))
  (( pct > 20 )) || return 0
  {
    echo "GRID STOPPED $(date -u +%FT%TZ)"
    echo "voided $VOIDED of $ATTEMPTED attempted cells = ${pct}% > 20% threshold"
    echo "Box left UP for diagnosis. Do NOT improvise a protocol change."
    echo "Diagnose in reports/phase4-night2.md; progress log: $PROGRESS"
  } | tee "$STOPFILE"
  echo "### GRID STOPPED — VOID RATE ${pct}%"
  exit 3
}

run_cell() { # arm data_size pipeline conns outdir [extra sweep args...]
  local arm="$1" ds="$2" pl="$3" conns="$4" dir="$5"; shift 5
  local wl="d${ds}-p${pl}-c${conns}"
  local existing; existing="$(artifact_for "$dir" "$arm" "$wl")"
  if [[ -n "$existing" ]]; then
    echo "SKIP $arm $wl (artifact exists: $(basename "$existing"))"; return 0
  fi
  local cl th; read -r cl th <<< "$(ct_for "$conns")"

  echo
  echo "======== $(date -u +%TZ)  $arm  $wl  -> $dir ========"
  bench/harness/arms.sh down all >/dev/null 2>&1 || true
  # A stand-up or exclusivity failure counts as attempted AND voided: a
  # systemic failure to start arms is exactly the "systemic harness failure"
  # the hard stop exists for, and not counting it would let the grid grind
  # through 108 failures while the void rate read 0%.
  if ! bench/harness/arms.sh up "$arm" >/dev/null 2>&1; then
    echo "STAND-UP FAILED $arm $wl"; record "$arm" "$wl" standup-failed "$dir"
    ATTEMPTED=$((ATTEMPTED+1)); VOIDED=$((VOIDED+1)); check_stop; return 1
  fi
  if ! bench/harness/arms.sh exclusive "$arm"; then
    echo "EXCLUSIVITY FAILED $arm $wl"; record "$arm" "$wl" exclusive-failed "$dir"
    ATTEMPTED=$((ATTEMPTED+1)); VOIDED=$((VOIDED+1)); check_stop; return 1
  fi

  local auth=()
  [[ "$arm" == P-* ]] && auth=(--auth-from-pg --psql bench/harness/box/psql --pg-host 127.0.0.1 --pg-port 28818)

  ATTEMPTED=$((ATTEMPTED+1))
  python3 bench/harness/sweep.py \
    --arm "$arm" --port "$(port_for "$arm")" \
    --data-size "$ds" --pipeline "$pl" --clients "$cl" --threads "$th" \
    --test-time 60 --run-count 3 --warmup-keys "$WARMUP_KEYS" \
    --env-class dedicated \
    --client-cpus 4-7 --client-pin-mechanism docker-cpuset \
    --memtier bench/harness/box/memtier_benchmark \
    --require-exclusive --rerun-on-spread \
    --out-dir "$dir" \
    "${auth[@]}" "$@"
  local rc=$?
  if [[ $rc -eq 0 ]]; then
    record "$arm" "$wl" ok "$dir"
  else
    VOIDED=$((VOIDED+1))
    record "$arm" "$wl" "void-rc$rc" "$dir"
  fi

  check_stop
  return 0
}

echo "### GRID START $(date -u +%FT%TZ)  head=$(git rev-parse --short HEAD)"
echo "### warm-up v2, ${WARMUP_KEYS} keys; 6 arms x 18 workloads; 3x60s per cell"

for arm in "${ARMS_ALL[@]}"; do
  for ds in "${SIZES[@]}"; do
    for pl in "${PIPES[@]}"; do
      for conns in "${CONNS[@]}"; do
        run_cell "$arm" "$ds" "$pl" "$conns" "$GRID"
      done
    done
  done
done

echo
echo "### MAIN GRID DONE $(date -u +%FT%TZ) — attempted=$ATTEMPTED voided=$VOIDED"

# ---- Anomaly protocol (ENV.md §21.3): the 64 B peak cell, saturation ENFORCED.
echo "### ANOMALY SET — 64 B peak, --require-saturation enforced"
for arm in P-opt R-opt; do
  run_cell "$arm" 64 16 64 "$ANOM" --require-saturation \
    --server-note "ANOMALY PROTOCOL cell, saturation enforced (ENV.md §21.3)"
done

# ---- Supplementary robustness check: the incumbent gets its I/O threads.
# Labelled, unranked. Redis 8's default is one I/O thread, so the ranked arm
# stays single-threaded; this answers "does the result survive giving the
# incumbent its I/O parallelism?" without redefining the pre-registered arm.
echo "### SUPPLEMENTARY — R-opt with io-threads=4 (robustness check, NOT a ranked arm)"
# Exported and then unset explicitly: a `VAR=x func` prefix on a SHELL FUNCTION
# does not reliably scope to that call in bash, and a leaked io-threads setting
# would silently turn later ranked cells into unranked ones.
export REDIS_EXTRA_ARGS="--io-threads 4"
run_cell R-opt 64 16 64 "$SUPP" \
  --server-note "ROBUSTNESS CHECK, NOT A RANKED ARM: Redis io-threads=4 (ENV.md §21.3 item 4)"
unset REDIS_EXTRA_ARGS

bench/harness/arms.sh down all >/dev/null 2>&1 || true
echo
echo "### GRID COMPLETE $(date -u +%FT%TZ) — attempted=$ATTEMPTED voided=$VOIDED"
echo "### cells on disk: $(ls "$GRID"/*.json 2>/dev/null | wc -l) grid, $(ls "$ANOM"/*.json 2>/dev/null | wc -l) anomaly, $(ls "$SUPP"/*.json 2>/dev/null | wc -l) supplementary"
