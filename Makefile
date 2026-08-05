.PHONY: compat harness-test

# Golden tests for the benchmark harness's table generator. Pure stdlib, runs in
# under a second, and pins the cross-payload bug that once produced a 312x
# headline (see bench/harness/test_curve.py). Run this before trusting any
# generated table.
harness-test:
	@python3 -m unittest discover -s bench/harness -p 'test_*.py' -v


compat:
	@cd tests/compat && \
	echo "=== Rebuilding pg_resp docker image ===" && \
	docker compose build pg_resp && \
	echo "" && \
	echo "=== Starting pg_resp server ===" && \
	docker compose up -d pg_resp && \
	echo "Waiting for pg_resp to be healthy..." && \
	sleep 5 && \
	echo "" && \
	for client in redis-cli redis-py node-redis go-redis jedis; do \
		echo "=== Testing $$client ===" && \
		docker compose run --rm $$client || true && \
		echo "" && \
		docker compose restart pg_resp > /dev/null 2>&1 && \
		sleep 2; \
	done && \
	echo "=== All clients tested ===" && \
	docker compose down

