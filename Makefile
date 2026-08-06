# `|| true` per client used to swallow every failure here: the target exited 0
# whether five clients passed or five failed, and a filtered log looked identical
# either way. That nearly shipped the v0.1.0 tag. Failures are now collected and
# the target exits non-zero — see CLAUDE.md, "absence of a failure token is not a
# pass".
.PHONY: compat harness-test pgxn-dist

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
	fails=""; \
	for client in redis-cli redis-py node-redis go-redis jedis; do \
		echo "=== Testing $$client ===" && \
		if ! docker compose run --rm $$client; then fails="$$fails $$client"; fi; \
		echo "" && \
		docker compose restart pg_resp > /dev/null 2>&1 && \
		sleep 2; \
	done && \
	docker compose down && \
	if [ -n "$$fails" ]; then \
		echo "=== COMPAT FAILED:$$fails ==="; exit 1; \
	else \
		echo "=== All 5 clients PASSED ==="; \
	fi


# Rebuild the PGXN source distribution, reproducibly, from the immutable tag.
#
# The dist is `git archive` of the tag plus two files and minus one directory's raw
# contents — all three deliberate and documented in
# docs/launch/LAUNCH-DAY-RUNBOOK.md:
#   + META.json                                  (PGXN needs it at root; not in the tag,
#                                                 because PGXN was approved after tagging
#                                                 and a tag that moves is worse)
#   + bench/results/RAW-ARTIFACTS-NOT-IN-DIST.md (so nothing is quietly dropped)
#   - bench/results/* except *.md                (5.4 MB / 291 raw artifact files)
#
# `zip -X` drops extra fields; every file's mtime is the tag's commit date (git
# archive does this for tag content, and the recipe stamps the two injected files to
# match); and the member order is `LC_ALL=C sort`ed rather than filesystem order. The
# output is therefore byte-identical on any machine at the same tag — verified by
# three consecutive builds.
# Verify against the recorded sha256 in docs/launch/DIST-SHA256.txt.
PGXN_TAG ?= v0.1.0
PGXN_VER ?= 0.1.0

pgxn-dist:
	@rm -rf build/pgxn && mkdir -p build/pgxn dist
	@git archive --prefix=pg_resp-$(PGXN_VER)/ $(PGXN_TAG) | tar -x -C build/pgxn
	@find build/pgxn/pg_resp-$(PGXN_VER)/bench/results -type f ! -name '*.md' -delete
	@find build/pgxn/pg_resp-$(PGXN_VER)/bench/results -type d -empty -delete
	@cp META.json build/pgxn/pg_resp-$(PGXN_VER)/META.json
	@cp docs/launch/RAW-ARTIFACTS-NOT-IN-DIST.md \
	    build/pgxn/pg_resp-$(PGXN_VER)/bench/results/RAW-ARTIFACTS-NOT-IN-DIST.md
	@# Normalise EVERY mtime — files and directories — to the tag's commit date.
	@# Pinning only the two injected files was not enough: deleting the raw artifacts
	@# and copying files in updates the containing DIRECTORY mtimes, and `find -print`
	@# fed those directories to zip. Measured across three builds two seconds apart:
	@# three distinct hashes. Normalising everything, and passing only files to zip
	@# (which recreates paths anyway), removes every wall-clock input.
	@find build/pgxn -exec touch -d "@$$(git log -1 --format=%ct $(PGXN_TAG))" {} +
	@rm -f dist/pg_resp-$(PGXN_VER).zip
	@cd build/pgxn && find pg_resp-$(PGXN_VER) -type f -print | LC_ALL=C sort | \
	    zip -qX9 ../../dist/pg_resp-$(PGXN_VER).zip -@
	@echo "dist/pg_resp-$(PGXN_VER).zip"
	@sha256sum dist/pg_resp-$(PGXN_VER).zip
	@echo "expected (docs/launch/DIST-SHA256.txt):"
	@cat docs/launch/DIST-SHA256.txt 2>/dev/null || echo "  (not recorded yet)"
