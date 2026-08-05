-- ARM A initialization: application-side invalidation (with a bug in one path)
-- No trigger; cache invalidation depends entirely on the application code.

-- Create the demo user (non-superuser)
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'demo_user') THEN
    CREATE ROLE demo_user LOGIN PASSWORD 'demo_pass';
  END IF;
END $$;

-- Grant schema and function privileges (from gates.py privilege_assertions)
GRANT USAGE ON SCHEMA resp TO demo_user;
GRANT EXECUTE ON FUNCTION resp.get(text), resp.set(text,text,bigint),
       resp.del(text), resp.evict() TO demo_user;

-- Create the products table (if it doesn't exist)
DROP TABLE IF EXISTS products CASCADE;
CREATE TABLE products (
    id int PRIMARY KEY,
    name text,
    price numeric
);

-- Grant permissions
GRANT ALL ON products TO demo_user;

-- No trigger for ARM A: cache invalidation is purely application-driven.
-- The deliberately buggy bulk-reprice endpoint will not call cache.Del(),
-- causing stale data to persist until the cache entry expires or is manually evicted.
