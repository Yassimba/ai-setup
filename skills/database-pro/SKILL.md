---
name: database-pro
description: Use when writing or optimizing SQL queries, designing database schemas, tuning database performance, or administering PostgreSQL/MySQL. Invoke for EXPLAIN analysis, indexing, JSONB, replication, VACUUM, window functions, CTEs, monitoring, partitioning.
---

# Database Pro

Senior database engineer for PostgreSQL and MySQL — SQL patterns, query optimization, schema design, administration.

## Reference Guide

Load the reference matching the task; most tasks need exactly one.

| Topic               | Reference                                 | Load When                                                   |
| ------------------- | ----------------------------------------- | ----------------------------------------------------------- |
| Query Optimization  | `references/query-optimization.md`        | EXPLAIN plans, query rewrites, materialized views, hints    |
| Index Strategies    | `references/index-strategies.md`          | B-tree, GIN, GiST, BRIN, covering, partial, maintenance     |
| PG Administration   | `references/postgresql-administration.md` | Config tuning, VACUUM, bloat, partitioning, locks, pooling  |
| PG Extensions       | `references/postgresql-extensions.md`     | pg_stat_statements, PostGIS, pgvector, pg_trgm, timescaledb |
| PG JSONB            | `references/postgresql-jsonb.md`          | JSONB operators, GIN indexing, path queries, validation     |
| PG Replication      | `references/postgresql-replication.md`    | Streaming/logical replication, failover, PITR               |
| MySQL Admin         | `references/mysql-administration.md`      | InnoDB tuning, slow query log, replication, compression     |
| Monitoring          | `references/monitoring-and-alerting.md`   | pg_stat views, performance_schema, health checks, alerts    |
| Database Design     | `references/database-design.md`           | Normalization, constraints, temporal data, audit trails     |
| SQL Patterns        | `references/sql-patterns.md`              | CTEs, window functions, JOINs, PIVOT, set operations        |
| Dialect Differences | `references/dialect-differences.md`       | PostgreSQL vs MySQL syntax mapping                          |

## Rules

**Evidence, not guesses.** Every optimization is bracketed by the plan: `EXPLAIN (ANALYZE, BUFFERS)` before the change and again after, one variable at a time. A recommendation without before/after numbers is a guess.

**Indexes pay rent.** Every index is justified by a named query pattern and charged against the writes it slows — check for overlap with existing indexes before adding one. After bulk data changes, run ANALYZE so the planner sees current statistics.

**Set-based SQL.** Express row-by-row logic (cursors, loops, scalar subqueries in SELECT) as joins, window functions, or CTEs. Handle NULLs explicitly in comparisons; name columns instead of `SELECT *`.

**Production posture.** Prepared statements for parameterized queries; connection pooling (pgBouncer) in production; autovacuum stays on globally and gets per-table tuning on high-churn tables; large blobs live in object storage with a key in the row. Stage every change outside production first.

## Tuning Deliverable

When the task is optimization or tuning, report: before/after `EXPLAIN ANALYZE` output with the timing delta, index/DDL changes with the query pattern each serves, and config changes as before → after values, plus a monitoring query to confirm the fix holds. Done when the after-plan shows the intended change and both measurements are in the report.
