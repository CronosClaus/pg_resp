-- ARM B: invalidation as a schema property.
--
-- The application in this arm contains NO cache-invalidation code at all — see
-- app/main.go. Everything below is what replaces it. If this file is wrong the
-- arm silently measures nothing, so the last block verifies its own work.

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'demo_user') THEN
    CREATE ROLE demo_user LOGIN PASSWORD 'demo_pass';
  END IF;
END $$;

DROP TABLE IF EXISTS products CASCADE;
CREATE TABLE products (
    id int PRIMARY KEY,
    name text,
    price numeric
);

GRANT ALL ON products TO demo_user;

-- The extension itself. pg_resp is in shared_preload_libraries in the image, so
-- the worker is already serving; this makes the resp.* SQL surface available.
CREATE EXTENSION IF NOT EXISTS pg_resp;

-- The D12 grant recipe, applied to a real non-superuser role. resp.* is revoked
-- from PUBLIC at install time, so without these two grants the CREATE TRIGGER
-- below fails with "permission denied for schema resp" — the USAGE half being
-- the one that is easy to forget. Recipe and rationale: docs/ops.md.
GRANT USAGE ON SCHEMA resp TO demo_user;
GRANT EXECUTE ON FUNCTION resp.evict() TO demo_user;

-- Create the trigger AS demo_user, not as the superuser running this script.
-- Deliberate: it exercises the documented grant recipe end to end instead of
-- quietly relying on superuser privileges a real deployment would not have.
SET ROLE demo_user;

CREATE TRIGGER products_cache_evict
AFTER UPDATE OR DELETE ON products
FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');

RESET ROLE;

-- Fail loudly rather than silently measure nothing.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'products_cache_evict') THEN
    RAISE EXCEPTION 'ARM B setup failed: products_cache_evict trigger was not created';
  END IF;
  RAISE NOTICE 'ARM B ready: invalidation is a schema property (trigger installed)';
END $$;
