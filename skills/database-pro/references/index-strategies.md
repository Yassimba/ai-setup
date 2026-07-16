# Index Strategies

## Index Selection Methodology

```sql
-- PostgreSQL: Find queries missing indexes
SELECT query, calls, total_exec_time, mean_exec_time
FROM pg_stat_statements
WHERE mean_exec_time > 100
ORDER BY total_exec_time DESC
LIMIT 20;

-- PostgreSQL: Find sequential scans on large tables
SELECT schemaname, tablename, seq_scan, seq_tup_read,
       idx_scan, seq_tup_read / seq_scan as avg_seq_tup_read
FROM pg_stat_user_tables
WHERE seq_scan > 0
  AND seq_tup_read / seq_scan > 10000
ORDER BY seq_tup_read DESC;

-- MySQL: Check table scans
SELECT * FROM sys.statements_with_full_table_scans
WHERE db = 'your_database'
ORDER BY exec_count DESC;
```

## B-Tree Indexes (Default)

### Single Column Indexes

```sql
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_orders_user_id ON orders(user_id);
CREATE INDEX idx_products_price ON products(price);
CREATE UNIQUE INDEX idx_users_username ON users(username);
```

### Multi-Column Indexes

```sql
-- Order matters: most selective column first
CREATE INDEX idx_orders_status_created
ON orders(status, created_at);

-- Good for: WHERE status = 'pending'
-- Good for: WHERE status = 'pending' AND created_at > '2024-01-01'
-- NOT good for: WHERE created_at > '2024-01-01' (status not specified)
```

### Column Order Guidelines

```sql
-- Rule 1: Equality before range
CREATE INDEX idx_events_type_timestamp
ON events(type, timestamp);  -- type = 'click' AND timestamp > ...

-- Rule 2: High selectivity first
CREATE INDEX idx_orders_user_status
ON orders(user_id, status);  -- user_id is more selective than status

-- Rule 3: Match query patterns
-- Query: WHERE country = 'US' AND city = 'NYC' AND zip = '10001'
CREATE INDEX idx_locations_country_city_zip
ON locations(country, city, zip);
```

## Covering Indexes

### PostgreSQL INCLUDE Clause

```sql
-- Include non-key columns for index-only scans
CREATE INDEX idx_users_email_covering
ON users(email) INCLUDE (name, created_at);

-- Query can be satisfied entirely from index
EXPLAIN (ANALYZE, BUFFERS)
SELECT name, created_at FROM users WHERE email = 'user@example.com';
-- Should show "Index Only Scan"
```

### MySQL Covering Indexes

```sql
-- MySQL: Add columns to end of index
CREATE INDEX idx_orders_user_covering
ON orders(user_id, status, created_at, total);

-- Query uses covering index
EXPLAIN SELECT status, created_at, total FROM orders WHERE user_id = 123;
-- Should show "Using index" in Extra column
```

## Partial Indexes

### PostgreSQL Partial Indexes

```sql
-- Index only active users
CREATE INDEX idx_users_active_email
ON users(email) WHERE active = true;

-- Index only recent orders
CREATE INDEX idx_orders_recent
ON orders(user_id, created_at)
WHERE created_at > NOW() - INTERVAL '30 days';

-- Index only pending/processing orders (ignore completed)
CREATE INDEX idx_orders_active
ON orders(status, user_id)
WHERE status IN ('pending', 'processing');
-- Smaller index = better performance + less storage
```

### MySQL Filtered Indexes (8.0+)

```sql
-- MySQL 8.0+ supports functional indexes for similar effect
CREATE INDEX idx_users_active
ON users((CASE WHEN active = 1 THEN email END));
```

## Expression Indexes

### PostgreSQL Function Indexes

```sql
-- Index for case-insensitive search
CREATE INDEX idx_users_email_lower ON users(LOWER(email));
-- Query must match: WHERE LOWER(email) = LOWER('User@Example.com')

-- Index for JSONB queries
CREATE INDEX idx_users_settings_theme ON users((settings->>'theme'));

-- Index for date truncation
CREATE INDEX idx_orders_date ON orders(DATE(created_at));
```

### MySQL Generated Column Indexes

```sql
-- Create generated column, then index it
ALTER TABLE users
ADD COLUMN email_lower VARCHAR(255)
GENERATED ALWAYS AS (LOWER(email)) STORED;

CREATE INDEX idx_users_email_lower ON users(email_lower);
```

## Specialized Index Types

### GIN Indexes (Full-Text, Arrays, JSONB)

```sql
-- Full-text search
CREATE INDEX idx_posts_search
ON posts USING GIN(to_tsvector('english', title || ' ' || content));

SELECT * FROM posts
WHERE to_tsvector('english', title || ' ' || content)
      @@ to_tsquery('english', 'database & optimization');

-- Array search
CREATE INDEX idx_products_tags ON products USING GIN(tags);
SELECT * FROM products WHERE tags @> ARRAY['electronics', 'sale'];

-- JSONB search
CREATE INDEX idx_users_metadata ON users USING GIN(metadata);
SELECT * FROM users WHERE metadata @> '{"plan": "premium"}';
```

### GiST Indexes (Spatial, Range)

```sql
-- PostGIS spatial index
CREATE INDEX idx_locations_geom ON locations USING GIST(geom);
-- Enables: WHERE ST_DWithin(geom, point, 1000)

-- Range types
CREATE INDEX idx_events_time_range ON events USING GIST(time_range);
SELECT * FROM events WHERE time_range && '[2024-01-01, 2024-01-31]'::tstzrange;

-- Nearest neighbor (KNN)
CREATE INDEX idx_locations_gist ON locations USING GIST(coordinates);
-- Enables: ORDER BY coordinates <-> point('0,0') LIMIT 10
```

### BRIN Indexes (Large, Naturally Ordered Tables)

```sql
-- Time-series data (insert-only, sorted by time)
CREATE INDEX idx_metrics_time_brin ON metrics USING BRIN(timestamp);
-- Very small index, good for WHERE timestamp > NOW() - INTERVAL '1 day'

-- Works well with:
-- - Log tables
-- - Time-series metrics
-- - Append-only tables with natural order
```

### MySQL Full-Text Indexes

```sql
CREATE FULLTEXT INDEX idx_posts_content ON posts(title, content);

SELECT * FROM posts
WHERE MATCH(title, content)
      AGAINST('database optimization' IN NATURAL LANGUAGE MODE);

-- Boolean mode for complex searches
SELECT * FROM posts
WHERE MATCH(title, content)
      AGAINST('+database -mysql' IN BOOLEAN MODE);
```

## Index Maintenance

### PostgreSQL Maintenance

```sql
-- Update statistics
ANALYZE users;

-- Rebuild bloated index (non-blocking)
REINDEX INDEX CONCURRENTLY idx_users_email;

-- Check index bloat
SELECT
    schemaname, tablename, indexname,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size,
    idx_scan as scans,
    idx_tup_read as tuples_read,
    idx_tup_fetch as tuples_fetched
FROM pg_stat_user_indexes
ORDER BY pg_relation_size(indexrelid) DESC;

-- Find unused indexes
SELECT
    schemaname, tablename, indexname,
    idx_scan,
    pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE idx_scan = 0
  AND indexrelname NOT LIKE 'pg_toast%'
ORDER BY pg_relation_size(indexrelid) DESC;

-- Find duplicate indexes
SELECT
    pg_size_pretty(SUM(pg_relation_size(idx))::BIGINT) as size,
    (array_agg(idx))[1] as idx1,
    (array_agg(idx))[2] as idx2
FROM (
    SELECT
        indexrelid::regclass as idx,
        (indrelid::text ||E'\n'|| indclass::text ||E'\n'||
         indkey::text ||E'\n'|| COALESCE(indexprs::text,'')||E'\n'||
         COALESCE(indpred::text,'')) as key
    FROM pg_index
) sub
GROUP BY key
HAVING COUNT(*) > 1
ORDER BY SUM(pg_relation_size(idx)) DESC;
```

### MySQL Maintenance

```sql
-- Update statistics
ANALYZE TABLE users;

-- Rebuild index
ALTER TABLE users DROP INDEX idx_users_email, ADD INDEX idx_users_email(email);

-- Check index usage
SELECT
    object_schema, object_name, index_name,
    count_star, count_read, count_fetch
FROM performance_schema.table_io_waits_summary_by_index_usage
WHERE object_schema = 'your_database'
ORDER BY count_star DESC;

-- Find unused indexes
SELECT object_schema, object_name, index_name
FROM performance_schema.table_io_waits_summary_by_index_usage
WHERE index_name IS NOT NULL
  AND count_star = 0
  AND object_schema = 'your_database';
```

## Index Anti-Patterns

| Anti-Pattern | Issue | Solution |
|-------------|-------|----------|
| Index every column | Write overhead, storage waste | Index based on query patterns |
| Redundant indexes | `(a)` + `(a,b)` | Keep only `(a,b)` |
| Wrong column order | `(created_at, user_id)` for `WHERE user_id = ?` | Put filtered columns first |
| Over-covering | Including rarely-used columns | Include only frequently accessed columns |
| Ignoring WHERE clause | Full index for 5% of data | Use partial indexes |
| Expression mismatch | Index `email`, query `LOWER(email)` | Create expression index |

## Index Design Checklist

1. **Analyze queries**: Use pg_stat_statements or slow query log
2. **Check execution plans**: Look for Seq Scan on large tables
3. **Design indexes**: Equality -> Range -> Include
4. **Create concurrently**: `CREATE INDEX CONCURRENTLY` to avoid locking
5. **Validate improvement**: Compare before/after EXPLAIN
6. **Monitor usage**: Remove unused indexes after 30 days
7. **Maintain regularly**: VACUUM, ANALYZE, REINDEX as needed
