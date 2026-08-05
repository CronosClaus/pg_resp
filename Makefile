.PHONY: compat

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

