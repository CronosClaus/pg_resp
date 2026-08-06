#!/usr/bin/env bash
# G1 gate: fresh machine -> running demo in <= 3 commands, <= 5 minutes.
#
# RUN THIS ON A FRESH HOST WITH NO pg_resp LAYERS IN ITS DOCKER CACHE. That is the
# whole point: G1 is dominated by the image pull, and a machine that has built or
# pulled this image before measures a cost no new user pays. A development machine
# with warm layers will report a time that is not the gate.
#
# It also verifies the pull works WITHOUT credentials, which is the pause-2 check:
# GHCR packages are private by default, independently of repository visibility.
#
# Usage:   bash g1-quickstart-timing.sh [image]
# Default image: ghcr.io/cronosclaus/pg_resp:0.1.0-rc
set -uo pipefail
IMAGE="${1:-ghcr.io/cronosclaus/pg_resp:0.1.0-rc}"

echo "=== G1 quickstart timing ==="
echo "host   : $(uname -srm)"
echo "docker : $(docker --version)"
echo "image  : $IMAGE"
echo "started: $(date -u +%FT%TZ)"
echo

# Prove the cache is cold, so the number means something.
if docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "!! WARNING: image already present locally. This run will NOT measure G1."
  echo "!! Remove it first (docker rmi $IMAGE) or use a fresh host."
fi

# Prove no credentials are in play — this is the pause-2 verification.
echo "--- docker config (must contain no ghcr.io auth entry) ---"
if [ -f "${HOME}/.docker/config.json" ]; then
  grep -c 'ghcr.io' "${HOME}/.docker/config.json" 2>/dev/null | sed 's/^/ghcr.io entries in docker config: /'
else
  echo "no ~/.docker/config.json — unauthenticated by construction"
fi
echo

START=$(date +%s)

echo "### COMMAND 1 — pull and run"
CMD1_START=$(date +%s)
docker run -d --name pg_resp_g1 \
  -e POSTGRES_PASSWORD=postgres \
  -p 127.0.0.1:6379:6379 -p 127.0.0.1:5432:5432 \
  "$IMAGE" || { echo "G1 FAILED at command 1 — if this is a 'denied' error the package is still PRIVATE"; exit 1; }
echo "  command 1 wall clock: $(( $(date +%s) - CMD1_START ))s"

echo "### COMMAND 2 — wait until it serves"
CMD2_START=$(date +%s)
for i in $(seq 1 150); do
  docker exec pg_resp_g1 pg_isready -U postgres -q 2>/dev/null && break
  sleep 1
done
docker exec pg_resp_g1 pg_isready -U postgres || { echo "G1 FAILED at command 2"; exit 1; }
echo "  command 2 wall clock: $(( $(date +%s) - CMD2_START ))s"

echo "### COMMAND 3 — talk to it with redis-cli"
CMD3_START=$(date +%s)
docker run --rm --network host redis:8.2-alpine redis-cli -h 127.0.0.1 -p 6379 SET greeting hello
GOT=$(docker run --rm --network host redis:8.2-alpine redis-cli -h 127.0.0.1 -p 6379 GET greeting)
echo "  GET returned: $GOT"
echo "  command 3 wall clock: $(( $(date +%s) - CMD3_START ))s"

TOTAL=$(( $(date +%s) - START ))
echo
echo "=== RESULT ==="
echo "total wall clock : ${TOTAL}s  (gate: <= 300s)"
echo "commands used    : 3          (gate: <= 3)"
echo "GET round-trip   : $GOT"
if [ "$GOT" = "hello" ] && [ "$TOTAL" -le 300 ]; then
  echo "G1: PASS"
else
  echo "G1: FAIL"
fi
echo "finished: $(date -u +%FT%TZ)"

echo
echo "--- cleanup ---"
docker rm -f pg_resp_g1 >/dev/null 2>&1 || true
