#!/bin/bash
# Setup script that initializes the database based on the INVALIDATION mode.
# Called by docker-compose after pg_resp starts.

set -e

INVALIDATION=${INVALIDATION:-trigger}
INIT_SCRIPT="/app/init-arm-a.sql"

if [ "$INVALIDATION" = "trigger" ]; then
    INIT_SCRIPT="/app/init-arm-b.sql"
fi

echo "Initializing database for INVALIDATION=$INVALIDATION (using $INIT_SCRIPT)"

# Wait for pg_resp to be ready
for i in {1..30}; do
    if pg_isready -h pg_resp -U postgres -d postgres 2>/dev/null; then
        break
    fi
    echo "Waiting for postgres... ($i/30)"
    sleep 1
done

# Extra wait for bgworker to fully initialize
sleep 2

# Run the init script (password-less connection within the container)
# Continue on error to allow CREATE TRIGGER to fail gracefully if resp.evict is not available
export PGPASSWORD=postgres
psql -h pg_resp -U postgres -d postgres -v ON_ERROR_STOP=0 -f "$INIT_SCRIPT"

echo "Database initialized"
