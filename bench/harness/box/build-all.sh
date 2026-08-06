#!/usr/bin/env bash
# Box bootstrap: build/pull every image the six arms need. Frozen host — all
# tooling lives in containers (Phase 4 box requirement 5).
set -euo pipefail
cd ~/pg_resp
git pull -q --ff-only
echo "### repo HEAD: $(git rev-parse --short HEAD)"

echo "### [1/5] pull pinned incumbent images"
docker pull -q redis:8.2-alpine
docker pull -q valkey/valkey:8.1-alpine

echo "### [2/5] build memtier_benchmark @272eeb647df5"
docker build -q -t pg_resp-bench-memtier:pinned \
  -f docker/memtier.Dockerfile ref/memtier_benchmark

echo "### [3/5] build redka @d3c353f02470 (upstream's own Dockerfile)"
docker build -q -t pg_resp-bench-redka:pinned ref/redka

echo "### [4/5] build pg_resp on postgres:18 (the W9/G1 artifact)"
docker build -t pg_resp:0.1.0-rc -f docker/pg_resp.Dockerfile .

echo "### [5/5] digests"
for i in redis:8.2-alpine valkey/valkey:8.1-alpine; do
  docker image inspect --format '{{.RepoTags}} {{index .RepoDigests 0}}' "$i"
done
for i in pg_resp-bench-memtier:pinned pg_resp-bench-redka:pinned pg_resp:0.1.0-rc; do
  docker image inspect --format '{{.RepoTags}} local-build id={{.Id}}' "$i"
done
echo "### BUILD-ALL COMPLETE rc=0"
