# Database Dialect Differences (PostgreSQL vs MySQL)

## Auto-Incrementing Primary Keys

```sql
-- PostgreSQL
CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,  -- or BIGSERIAL
    name VARCHAR(100)
);
-- Alternative (PostgreSQL 10+)
CREATE TABLE users (
    user_id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name VARCHAR(100)
);

-- MySQL
CREATE TABLE users (
    user_id INT AUTO_INCREMENT PRIMARY KEY,
    name VARCHAR(100)
);
```

## String Concatenation

```sql
-- PostgreSQL (strict — automatic casting)
SELECT first_name || ' ' || last_name AS full_name FROM users;
SELECT CONCAT(first_name, ' ', last_name) AS full_name FROM users;  -- NULL-safe

-- MySQL (automatic type conversion)
SELECT CONCAT(first_name, ' ', last_name) AS full_name FROM users;
-- Note: || is logical OR in MySQL, not concatenation
```

## Date/Time Functions

```sql
-- Current timestamp
-- PostgreSQL
SELECT CURRENT_TIMESTAMP, NOW(), CURRENT_DATE, CURRENT_TIME;
-- MySQL
SELECT CURRENT_TIMESTAMP, NOW(), CURDATE(), CURTIME();

-- Date arithmetic
-- PostgreSQL
SELECT order_date + INTERVAL '7 days' FROM orders;
SELECT order_date - INTERVAL '1 month' FROM orders;
SELECT AGE(CURRENT_DATE, birth_date) FROM users;  -- Interval type

-- MySQL
SELECT DATE_ADD(order_date, INTERVAL 7 DAY) FROM orders;
SELECT DATE_SUB(order_date, INTERVAL 1 MONTH) FROM orders;
SELECT DATEDIFF(CURRENT_DATE, birth_date) FROM users;  -- Days only

-- Date formatting
-- PostgreSQL
SELECT TO_CHAR(order_date, 'YYYY-MM-DD') FROM orders;
-- MySQL
SELECT DATE_FORMAT(order_date, '%Y-%m-%d') FROM orders;
```

## LIMIT/OFFSET (Pagination)

```sql
-- Both PostgreSQL and MySQL
SELECT * FROM products
ORDER BY product_id
LIMIT 10 OFFSET 20;
```

## Boolean Data Type

```sql
-- PostgreSQL (native BOOLEAN)
CREATE TABLE users (
    user_id SERIAL PRIMARY KEY,
    is_active BOOLEAN DEFAULT true
);
SELECT * FROM users WHERE is_active = true;

-- MySQL (TINYINT(1) or BOOLEAN alias)
CREATE TABLE users (
    user_id INT AUTO_INCREMENT PRIMARY KEY,
    is_active BOOLEAN DEFAULT 1  -- Stored as TINYINT(1)
);
SELECT * FROM users WHERE is_active = 1;
```

## JSON/JSONB Support

```sql
-- PostgreSQL (JSONB — binary, indexable)
CREATE TABLE events (
    event_id SERIAL PRIMARY KEY,
    event_data JSONB NOT NULL
);
SELECT event_data->>'user_id' as user_id FROM events;
SELECT * FROM events WHERE event_data @> '{"action": "login"}';
CREATE INDEX idx_events_data ON events USING GIN (event_data);

-- MySQL (8.0+)
CREATE TABLE events (
    event_id INT AUTO_INCREMENT PRIMARY KEY,
    event_data JSON NOT NULL
);
SELECT JSON_EXTRACT(event_data, '$.user_id') as user_id FROM events;
SELECT * FROM events WHERE JSON_EXTRACT(event_data, '$.action') = 'login';
CREATE INDEX idx_events_user ON events ((CAST(event_data->>'$.user_id' AS UNSIGNED)));
```

## String Comparison (Case Sensitivity)

```sql
-- PostgreSQL (case-sensitive by default)
SELECT * FROM users WHERE email = 'USER@EXAMPLE.COM';  -- Won't match 'user@example.com'
SELECT * FROM users WHERE LOWER(email) = LOWER('USER@EXAMPLE.COM');
SELECT * FROM users WHERE email ILIKE 'user@example.com';  -- Case-insensitive

-- MySQL (case-insensitive by default with utf8_general_ci collation)
SELECT * FROM users WHERE email = 'USER@EXAMPLE.COM';  -- Matches 'user@example.com'
SELECT * FROM users WHERE email COLLATE utf8_bin = 'user@example.com';  -- Case-sensitive
```

## Recursive CTEs

```sql
-- PostgreSQL (requires RECURSIVE keyword)
WITH RECURSIVE subordinates AS (
    SELECT employee_id, name, manager_id, 1 as level
    FROM employees WHERE manager_id IS NULL
    UNION ALL
    SELECT e.employee_id, e.name, e.manager_id, s.level + 1
    FROM employees e
    INNER JOIN subordinates s ON e.manager_id = s.employee_id
)
SELECT * FROM subordinates;

-- MySQL (8.0+ — same syntax)
WITH RECURSIVE subordinates AS (
    SELECT employee_id, name, manager_id, 1 as level
    FROM employees WHERE manager_id IS NULL
    UNION ALL
    SELECT e.employee_id, e.name, e.manager_id, s.level + 1
    FROM employees e
    INNER JOIN subordinates s ON e.manager_id = s.employee_id
)
SELECT * FROM subordinates;
```

## Window Functions — Frame Specifications

```sql
-- PostgreSQL — Full support including RANGE with intervals
SELECT
    order_date, total,
    SUM(total) OVER (
        ORDER BY order_date
        RANGE BETWEEN INTERVAL '7 days' PRECEDING AND CURRENT ROW
    ) as rolling_7day
FROM orders;

-- MySQL (8.0+) — Limited RANGE support (no intervals)
SELECT
    order_date, total,
    SUM(total) OVER (
        ORDER BY order_date
        ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
    ) as rolling_7rows
FROM orders;
```

## UPSERT (Insert or Update)

```sql
-- PostgreSQL (ON CONFLICT)
INSERT INTO products (product_id, name, price)
VALUES (123, 'Widget', 29.99)
ON CONFLICT (product_id)
DO UPDATE SET name = EXCLUDED.name, price = EXCLUDED.price;

-- MySQL (ON DUPLICATE KEY)
INSERT INTO products (product_id, name, price)
VALUES (123, 'Widget', 29.99)
ON DUPLICATE KEY UPDATE name = VALUES(name), price = VALUES(price);

-- MySQL 8.0.19+ (alias-based)
INSERT INTO products (product_id, name, price)
VALUES (123, 'Widget', 29.99) AS new
ON DUPLICATE KEY UPDATE name = new.name, price = new.price;
```

## Data Type Mapping

| Concept | PostgreSQL | MySQL |
|---------|-----------|-------|
| Integer | INT, BIGINT | INT, BIGINT |
| Decimal | NUMERIC, DECIMAL | DECIMAL |
| String | VARCHAR, TEXT | VARCHAR, TEXT |
| Binary | BYTEA | BLOB, BINARY |
| Boolean | BOOLEAN | BOOLEAN/TINYINT(1) |
| Date | DATE | DATE |
| Timestamp | TIMESTAMP | DATETIME, TIMESTAMP |
| UUID | UUID | CHAR(36), BINARY(16) |
| JSON | JSON, JSONB | JSON |
| Array | ARRAY | JSON (no native arrays) |

## Performance Tips

**PostgreSQL:**
- Use `EXPLAIN ANALYZE` with `BUFFERS`
- Leverage JSONB with GIN indexes
- Use parallel query settings for large scans
- VACUUM and ANALYZE regularly
- Consider table partitioning for 10M+ rows
- Use `ILIKE` or expression indexes for case-insensitive search

**MySQL:**
- Choose InnoDB over MyISAM
- Optimize buffer pool size (70-80% of RAM)
- Use covering indexes aggressively
- Be aware of case-insensitive defaults
- Consider read replicas for scaling
- Use `pt-query-digest` for slow query analysis
