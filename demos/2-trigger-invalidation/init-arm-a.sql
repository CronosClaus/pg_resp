-- ARM A: application-side cache invalidation, with a deliberate realistic bug.
--
-- Note what is NOT here: no trigger, and no resp.* grants. Invalidation in this
-- arm lives entirely in app/main.go, where the primary PUT path remembers to
-- delete the cache key and the later-added bulk-reprice path does not. That
-- omission is the bug being measured.

-- Create the demo user
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'demo_user') THEN
    CREATE ROLE demo_user LOGIN PASSWORD 'demo_pass';
  END IF;
END $$;

-- Create the products table
DROP TABLE IF EXISTS products CASCADE;
CREATE TABLE products (
    id int PRIMARY KEY,
    name text,
    price numeric
);

GRANT ALL ON products TO demo_user;

-- Deliberately absent: CREATE TRIGGER. Compare init-arm-b.sql.
