#!/usr/bin/env bash
# Bring the bench box's checkout to origin/master, safely.
#
# WHY THIS EXISTS
# ---------------
# Benchmark results are produced ON the box as untracked files, rsynced back, and
# committed from the workstation. The next `git pull` on the box then aborts with
# "untracked working tree files would be overwritten by merge" for every result
# file — because the file now exists in both places. This bit twice: once
# silently, leaving the box running a sweep.py without --warmup-keys, and once
# while rebuilding the image, where a background build started against the OLD
# tree because the pull had failed and nothing checked.
#
# So the removal is scripted rather than hand-typed, and it REFUSES unless every
# file it would delete is byte-identical to what origin/master already has. A file
# the box produced and the repo has not seen is data, and data is never deleted to
# make a merge convenient.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../../.."

git fetch -q origin
differing=0 checked=0
while IFS= read -r f; do
  git cat-file -e "origin/master:$f" 2>/dev/null || continue   # not upstream: leave it alone
  checked=$((checked+1))
  if ! git show "origin/master:$f" | diff -q - "$f" >/dev/null 2>&1; then
    echo "DIFFERS (will NOT delete): $f" >&2
    differing=$((differing+1))
  fi
done < <(git ls-files --others --exclude-standard)

if (( differing > 0 )); then
  echo "REFUSING to sync: $differing untracked file(s) differ from origin/master." >&2
  echo "The box holds data the repo does not. rsync it back and commit it first." >&2
  exit 1
fi

if (( checked > 0 )); then
  echo "removing $checked untracked file(s) already committed upstream (verified identical)"
  while IFS= read -r f; do
    git cat-file -e "origin/master:$f" 2>/dev/null && rm -f "$f"
  done < <(git ls-files --others --exclude-standard)
fi

git pull -q --ff-only
echo "box synced to $(git rev-parse --short HEAD)"
