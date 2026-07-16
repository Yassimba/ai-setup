# PostgreSQL Administration

## Memory Configuration

### Shared Buffers

```sql
-- Recommended: 25% of system RAM (up to 40% for dedicated DB server)
-- For 16GB RAM server:
ALTER SYSTEM SET shared_buffers = '4GB';

-- Monitor buffer hit ratio (target: >99%)
SELECT
    sum(heap_blks_read) as heap_read,
    sum(heap_blks_hit) as heap_hit,
    round(sum(heap_blks_hit) / nullif(sum(heap_blks_hit) + sum(heap_blks_read), 0) * 100, 2) as cache_hit_ratio
FROM pg_statio_user_tables;
```

### Work Memory

```sql
-- Per-operation memory for sorting/hashing
-- Recommended: (Total RAM * 0.25) / max_connections
-- For 16GB RAM, 100 connections: ~40MB
ALTER SYSTEM SET work_mem = '40MB';

-- Set per-session for large operations
SET work_mem = '256MB';
SELECT ... ORDER BY ... LIMIT 1000;
RESET work_mem;
```

### Maintenance Work Memory

```sql
-- For VACUUM, CREATE INDEX, ALTER TABLE
-- Recommended: 1-2GB for production systems
ALTER SYSTEM SET maintenance_work_mem = '2GB';

-- Autovacuum workers use proportional amount
ALTER SYSTEM SET autovacuum_work_mem = '512MB';
```

### Effective Cache Size

```sql
-- Planner hint for available OS cache
-- Recommended: 50-75% of total RAM
ALTER SYSTEM SET effective_cache_size = '12GB';
```

## Query Planner Settings

### Statistics Target

```sql
-- Default is 100, increase for better estimates on complex queries
ALTER SYSTEM SET default_statistics_target = 200;

-- Per-column statistics for specific columns
ALTER TABLE users ALTER COLUMN email SET STATISTICS 500;

-- Force statistics update
ANALYZE users;

-- Check statistics quality
SELECT schemaname, tablename, attname, n_distinct, correlation
FROM pg_stats WHERE tablename = 'users';
```

### Parallel Query Configuration

```sql
ALTER SYSTEM SET max_parallel_workers_per_gather = 4;
ALTER SYSTEM SET max_parallel_workers = 8;
ALTER SYSTEM SET parallel_setup_cost = 100;
ALTER SYSTEM SET parallel_tuple_cost = 0.01;

-- Minimum rows to consider parallel execution
ALTER SYSTEM SET min_parallel_table_scan_size = '8MB';
ALTER SYSTEM SET min_parallel_index_scan_size = '512kB';
```

### Cost Parameters

```sql
-- Adjust based on hardware
ALTER SYSTEM SET random_page_cost = 1.1;  -- For SSD (default 4.0 is for HDD)
ALTER SYSTEM SET seq_page_cost = 1.0;
ALTER SYSTEM SET effective_io_concurrency = 200;  -- Higher for SSD
```

## Write Performance Optimization

### WAL Configuration

```sql
ALTER SYSTEM SET wal_buffers = '16MB';
ALTER SYSTEM SET wal_writer_delay = '200ms';

-- Checkpoint configuration
ALTER SYSTEM SET checkpoint_completion_target = 0.9;
ALTER SYSTEM SET max_wal_size = '2GB';
ALTER SYSTEM SET min_wal_size = '1GB';

-- Monitor checkpoints
SELECT
    checkpoints_timed,
    checkpoints_req,
    checkpoint_write_time,
    checkpoint_sync_time,
    buffers_checkpoint,
    buffers_clean,
    buffers_backend
FROM pg_stat_bgwriter;
-- Too many requested checkpoints = increase max_wal_size
```

### Commit Delays

```sql
-- Group commits (trade latency for throughput)
ALTER SYSTEM SET commit_delay = 10000;  -- 10ms
ALTER SYSTEM SET commit_siblings = 5;

-- Asynchronous commit (trade durability for speed)
-- Use cautiously — risk losing recent commits on crash
ALTER SYSTEM SET synchronous_commit = 'off';

-- Or per-transaction
BEGIN;
SET LOCAL synchronous_commit = 'off';
INSERT INTO logs (...) VALUES (...);
COMMIT;
```

## VACUUM Fundamentals

PostgreSQL uses MVCC (Multi-Version Concurrency Control):
- Updates/deletes don't remove old rows immediately
- Old rows marked as "dead tuples"
- VACUUM reclaims space from dead tuples
- Without VACUUM: table bloat, degraded performance, transaction ID wraparound

### VACUUM Variants

```sql
-- Standard VACUUM (non-blocking, reclaims space for reuse)
VACUUM users;

-- VACUUM FULL (locks table, rewrites entire table, reclaims disk space)
VACUUM FULL users;
-- Use pg_repack instead for production (non-blocking alternative)

-- VACUUM VERBOSE (shows details)
VACUUM VERBOSE users;

-- VACUUM ANALYZE (vacuum + update statistics)
VACUUM ANALYZE users;
```

### VACUUM Monitoring

```sql
-- Check when tables were last vacuumed
SELECT
  schemaname, relname,
  last_vacuum, last_autovacuum,
  n_dead_tup, n_live_tup,
  round(100.0 * n_dead_tup / NULLIF(n_live_tup + n_dead_tup, 0), 2) as dead_pct
FROM pg_stat_user_tables
ORDER BY n_dead_tup DESC;

-- Check vacuum progress (PG 9.6+)
SELECT
  pid, datname, relid::regclass, phase,
  heap_blks_total, heap_blks_scanned, heap_blks_vacuumed,
  round(100.0 * heap_blks_scanned / NULLIF(heap_blks_total, 0), 2) as pct_complete
FROM pg_stat_progress_vacuum;
```

## Autovacuum Configuration

```sql
-- Global settings (postgresql.conf)
autovacuum = on
autovacuum_max_workers = 3
autovacuum_naptime = 60s

-- Vacuum thresholds
autovacuum_vacuum_threshold = 50
autovacuum_vacuum_scale_factor = 0.2
-- Triggers when: dead_tuples > threshold + (scale_factor * total_tuples)
-- Default: 50 + (0.2 * 1000000) = 200,050 dead tuples for 1M row table

-- Analyze thresholds
autovacuum_analyze_threshold = 50
autovacuum_analyze_scale_factor = 0.1

-- Performance settings
autovacuum_vacuum_cost_delay = 2ms
autovacuum_vacuum_cost_limit = 200
```

### Per-Table Autovacuum Tuning

```sql
-- High-churn table: vacuum more aggressively
ALTER TABLE orders SET (
  autovacuum_vacuum_scale_factor = 0.05,
  autovacuum_vacuum_threshold = 1000,
  autovacuum_analyze_scale_factor = 0.02
);

-- Large, stable table: vacuum less often
ALTER TABLE archive_logs SET (
  autovacuum_vacuum_scale_factor = 0.5,
  autovacuum_vacuum_threshold = 5000
);

-- Very high-churn table: disable cost delays
ALTER TABLE sessions SET (
  autovacuum_vacuum_cost_delay = 0
);

-- View table settings
SELECT relname, reloptions FROM pg_class WHERE relname = 'orders';
```

## ANALYZE (Statistics)

```sql
-- Update statistics for query planner
ANALYZE users;
ANALYZE;  -- All tables

-- Check statistics freshness
SELECT
  schemaname, relname,
  last_analyze, last_autoanalyze,
  n_mod_since_analyze
FROM pg_stat_user_tables
ORDER BY n_mod_since_analyze DESC;

-- Increase statistics target for high-cardinality columns
ALTER TABLE users ALTER COLUMN email SET STATISTICS 1000;
-- Default is 100, range is 0-10000

-- View column statistics
SELECT tablename, attname, n_distinct, correlation, null_frac
FROM pg_stats WHERE tablename = 'users';
```

## Bloat Detection and Removal

### Detect Table Bloat

```sql
SELECT
  schemaname, tablename,
  pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as total_size,
  pg_size_pretty(pg_relation_size(schemaname||'.'||tablename)) as table_size,
  n_dead_tup,
  round(100.0 * n_dead_tup / NULLIF(n_live_tup + n_dead_tup, 0), 2) as dead_pct
FROM pg_stat_user_tables
WHERE pg_total_relation_size(schemaname||'.'||tablename) > 10485760  -- > 10MB
ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;
```

### Detect Index Bloat

```sql
SELECT
  schemaname, tablename, indexname,
  idx_scan,
  pg_size_pretty(pg_relation_size(indexrelid)) as index_size
FROM pg_stat_user_indexes
WHERE idx_scan = 0 AND indexrelname NOT LIKE '%pkey'
ORDER BY pg_relation_size(indexrelid) DESC;
```

### Remove Bloat

```sql
-- Option 1: VACUUM FULL (locks table)
VACUUM FULL users;

-- Option 2: pg_repack (online, no locks)
-- Command line: pg_repack -d mydb -t users

-- Option 3: REINDEX (for index bloat)
REINDEX INDEX CONCURRENTLY idx_users_email;  -- Non-blocking (PG 12+)

-- Option 4: CLUSTER (rewrite table in index order, locks table)
CLUSTER users USING users_pkey;
```

## Transaction ID Wraparound

```sql
-- Check distance to wraparound (should be < 1 billion)
SELECT
  datname,
  age(datfrozenxid) as xid_age,
  2147483647 - age(datfrozenxid) as xids_remaining
FROM pg_database
ORDER BY age(datfrozenxid) DESC;

-- Per-table wraparound status
SELECT
  schemaname, relname,
  age(relfrozenxid) as xid_age,
  pg_size_pretty(pg_total_relation_size(schemaname||'.'||relname)) as size
FROM pg_stat_user_tables
ORDER BY age(relfrozenxid) DESC
LIMIT 20;

-- Prevent wraparound: VACUUM FREEZE
VACUUM FREEZE;
VACUUM FREEZE users;
```

## Connection Pooling

### PostgreSQL Connection Settings

```sql
ALTER SYSTEM SET max_connections = 200;
ALTER SYSTEM SET superuser_reserved_connections = 3;
ALTER SYSTEM SET idle_in_transaction_session_timeout = '5min';
ALTER SYSTEM SET statement_timeout = '30s';

-- Monitor connections
SELECT
    state, count(*),
    max(now() - state_change) as max_idle_time
FROM pg_stat_activity
WHERE state IS NOT NULL
GROUP BY state;
```

### PgBouncer Configuration

```ini
# pgbouncer.ini
[databases]
mydb = host=primary-host port=5432 dbname=mydb

[pgbouncer]
listen_addr = *
listen_port = 6432
auth_type = scram-sha-256
auth_file = /etc/pgbouncer/userlist.txt
pool_mode = transaction
max_client_conn = 1000
default_pool_size = 25
reserve_pool_size = 5
```

## Lock Management

```sql
-- Check current locks
SELECT
    locktype, relation::regclass, mode, granted,
    pid, pg_blocking_pids(pid) as blocked_by
FROM pg_locks
WHERE NOT granted
ORDER BY relation;

-- Find blocking queries
SELECT
    blocked_locks.pid AS blocked_pid,
    blocked_activity.usename AS blocked_user,
    blocking_locks.pid AS blocking_pid,
    blocking_activity.usename AS blocking_user,
    blocked_activity.query AS blocked_statement,
    blocking_activity.query AS blocking_statement
FROM pg_catalog.pg_locks blocked_locks
JOIN pg_catalog.pg_stat_activity blocked_activity ON blocked_activity.pid = blocked_locks.pid
JOIN pg_catalog.pg_locks blocking_locks ON blocking_locks.locktype = blocked_locks.locktype
    AND blocking_locks.relation = blocked_locks.relation
    AND blocking_locks.pid != blocked_locks.pid
JOIN pg_catalog.pg_stat_activity blocking_activity ON blocking_activity.pid = blocking_locks.pid
WHERE NOT blocked_locks.granted;

-- Deadlock configuration
ALTER SYSTEM SET deadlock_timeout = '1s';
ALTER SYSTEM SET log_lock_waits = on;
```

## Partitioning

### Range Partitioning

```sql
CREATE TABLE events (
    id BIGSERIAL,
    event_type VARCHAR(50),
    created_at TIMESTAMP NOT NULL,
    data JSONB
) PARTITION BY RANGE (created_at);

CREATE TABLE events_2024_01 PARTITION OF events
    FOR VALUES FROM ('2024-01-01') TO ('2024-02-01');
CREATE TABLE events_2024_02 PARTITION OF events
    FOR VALUES FROM ('2024-02-01') TO ('2024-03-01');

-- Query uses partition pruning
EXPLAIN (ANALYZE)
SELECT * FROM events
WHERE created_at >= '2024-01-15' AND created_at < '2024-01-20';
```

### List Partitioning

```sql
CREATE TABLE products (
    product_id SERIAL,
    category VARCHAR(50) NOT NULL,
    name VARCHAR(200)
) PARTITION BY LIST (category);

CREATE TABLE products_electronics PARTITION OF products
    FOR VALUES IN ('electronics', 'computers', 'phones');
CREATE TABLE products_clothing PARTITION OF products
    FOR VALUES IN ('clothing', 'shoes', 'accessories');
```

### Hash Partitioning

```sql
CREATE TABLE users (
    user_id SERIAL,
    email VARCHAR(255)
) PARTITION BY HASH (user_id);

CREATE TABLE users_p0 PARTITION OF users
    FOR VALUES WITH (MODULUS 4, REMAINDER 0);
CREATE TABLE users_p1 PARTITION OF users
    FOR VALUES WITH (MODULUS 4, REMAINDER 1);
```

## Configuration File Example

```ini
# postgresql.conf — Production optimized for 16GB RAM server

# Memory
shared_buffers = 4GB
effective_cache_size = 12GB
work_mem = 40MB
maintenance_work_mem = 2GB

# WAL
wal_buffers = 16MB
checkpoint_completion_target = 0.9
max_wal_size = 2GB

# Query Planner
default_statistics_target = 200
random_page_cost = 1.1  # SSD
effective_io_concurrency = 200  # SSD

# Parallel Queries
max_parallel_workers_per_gather = 4
max_parallel_workers = 8

# Connections
max_connections = 200

# Logging
log_min_duration_statement = 1000  # Log queries > 1s
log_line_prefix = '%t [%p]: [%l-1] user=%u,db=%d,app=%a,client=%h '
log_checkpoints = on
log_lock_waits = on
```

## Maintenance Checklist

**Daily:**
- Monitor autovacuum activity
- Check for long-running queries
- Verify replication lag (if applicable)
- Check cache hit ratio

**Weekly:**
- Review slow queries from pg_stat_statements
- Check for table/index bloat
- Review unused indexes
- Monitor disk space usage

**Monthly:**
- Review autovacuum settings
- Reindex heavily updated indexes
- Update statistics on large tables
- Review database growth trends

**Quarterly:**
- Test backup restoration
- Review and optimize slow queries
- Capacity planning
- PostgreSQL version updates
