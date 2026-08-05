-- ARM B initialization: trigger-based invalidation
-- No application-side cache invalidation logic; the schema guarantees correctness.

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

-- ARM B: trigger-based invalidation. Every UPDATE or DELETE on products
-- automatically evicts the cached entry via resp.evict().
-- The application code does not call cache.Del() — the database schema
-- guarantees cache correctness via the trigger.
CREATE TRIGGER products_cache_evict
    AFTER UPDATE OR DELETE ON products
    FOR EACH ROW EXECUTE FUNCTION resp.evict('product:', 'id');
