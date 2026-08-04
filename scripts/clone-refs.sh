#!/usr/bin/env bash
# Clones the bible §4 reference corpus into /ref (gitignored) with sparse
# checkouts where specified, and records exact commit pins in docs/refs/PINS.md.
# Idempotent-ish: skips repos already present. Run from the repo root.
set -euo pipefail

REF=ref
PINS=docs/refs/PINS.md
mkdir -p "$REF" docs/refs

sparse_clone() { # url dir branch paths...
  local url=$1 dir=$2 branch=$3; shift 3
  if [ -d "$REF/$dir/.git" ]; then echo "skip $dir (exists)"; return; fi
  git clone --depth 1 --branch "$branch" --filter=blob:none --sparse "$url" "$REF/$dir"
  git -C "$REF/$dir" sparse-checkout set "$@"
}

shallow_clone() { # url dir [branch]
  local url=$1 dir=$2 branch=${3:-}
  if [ -d "$REF/$dir/.git" ]; then echo "skip $dir (exists)"; return; fi
  if [ -n "$branch" ]; then
    git clone --depth 1 --branch "$branch" "$url" "$REF/$dir"
  else
    git clone --depth 1 "$url" "$REF/$dir"
  fi
}

# --- Postgres: canonical bgworker + contrib gold standard (bible §4) ---
sparse_clone https://github.com/postgres/postgres.git postgres REL_18_STABLE \
  src/test/modules/worker_spi \
  src/backend/postmaster \
  src/include/postmaster \
  src/backend/storage/ipc \
  src/include/storage \
  contrib/pg_stat_statements

# --- pgrx: the APIs v0.1 lives on ---
shallow_clone https://github.com/pgcentralfoundation/pgrx.git pgrx

# --- Valkey: BSD-3 behavioral reference (NEVER Redis source — bible D8) ---
sparse_clone https://github.com/valkey-io/valkey.git valkey unstable \
  src \
  tests/unit/type

# --- Redka: closest competitor, positioning study ---
shallow_clone https://github.com/nalgeon/redka.git redka

# --- Omnigres: prior art for a socket server inside PG ---
sparse_clone https://github.com/omnigres/omnigres.git omnigres master \
  extensions/omni_httpd

# --- pg_net: bgworker + network + GUC packaging precedent ---
shallow_clone https://github.com/supabase/pg_net.git pg_net

# --- memtier_benchmark: the bench tool's knobs (GPLv2 — used, never linked) ---
shallow_clone https://github.com/RedisLabs/memtier_benchmark.git memtier_benchmark

# --- RESP2 spec: pointer only; the digest is written fresh, text not vendored ---
cat > docs/refs/resp2-spec.md << 'SPEC'
# RESP2 spec — pointer
Ground truth: https://redis.io/docs/latest/develop/reference/protocol-spec/
Phase 0 task: read it and fill .claude/skills/resp-protocol/SKILL.md.
Do not paste the page here; write the distilled facts and test vectors in the skill.
SPEC

# --- Record pins ---
{
  echo "# Reference corpus pins — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo
  echo "| repo | branch | commit |"
  echo "|---|---|---|"
  for d in "$REF"/*/; do
    name=$(basename "$d")
    branch=$(git -C "$d" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "?")
    commit=$(git -C "$d" rev-parse --short=12 HEAD 2>/dev/null || echo "?")
    echo "| $name | $branch | $commit |"
  done
} > "$PINS"

echo
echo "Done. Pins written to $PINS:"
cat "$PINS"
