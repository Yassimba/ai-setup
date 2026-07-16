# SQL Patterns

## Common Table Expressions (CTEs)

```sql
-- Basic CTE for readability
WITH active_users AS (
    SELECT user_id, username, created_at
    FROM users
    WHERE is_active = true
      AND last_login >= CURRENT_DATE - INTERVAL '30 days'
),
user_orders AS (
    SELECT user_id, COUNT(*) as order_count, SUM(total) as total_spent
    FROM orders
    WHERE status = 'completed'
    GROUP BY user_id
)
SELECT
    u.username,
    u.created_at,
    COALESCE(o.order_count, 0) as orders,
    COALESCE(o.total_spent, 0) as lifetime_value
FROM active_users u
LEFT JOIN user_orders o ON u.user_id = o.user_id
WHERE COALESCE(o.order_count, 0) > 0
ORDER BY o.total_spent DESC;

-- CTE with multiple references (avoiding duplicate computation)
WITH monthly_sales AS (
    SELECT
        DATE_TRUNC('month', sale_date) as month,
        product_id,
        SUM(quantity) as total_quantity,
        SUM(amount) as total_amount
    FROM sales
    WHERE sale_date >= '2024-01-01'
    GROUP BY DATE_TRUNC('month', sale_date), product_id
)
SELECT
    current.month,
    current.product_id,
    current.total_amount,
    current.total_amount - COALESCE(previous.total_amount, 0) as growth,
    ROUND(100.0 * (current.total_amount - COALESCE(previous.total_amount, 0))
        / NULLIF(previous.total_amount, 0), 2) as growth_pct
FROM monthly_sales current
LEFT JOIN monthly_sales previous
    ON current.product_id = previous.product_id
    AND current.month = previous.month + INTERVAL '1 month';
```

## Recursive CTEs

```sql
-- Organizational hierarchy traversal
WITH RECURSIVE org_hierarchy AS (
    -- Anchor member: top-level managers
    SELECT
        employee_id, name, manager_id,
        1 as level,
        ARRAY[employee_id] as path,
        name as hierarchy_path
    FROM employees
    WHERE manager_id IS NULL

    UNION ALL

    -- Recursive member
    SELECT
        e.employee_id, e.name, e.manager_id,
        h.level + 1,
        h.path || e.employee_id,
        h.hierarchy_path || ' > ' || e.name
    FROM employees e
    INNER JOIN org_hierarchy h ON e.manager_id = h.employee_id
    WHERE NOT e.employee_id = ANY(h.path)  -- Prevent cycles
)
SELECT
    employee_id,
    REPEAT('  ', level - 1) || name as indented_name,
    level,
    hierarchy_path
FROM org_hierarchy
ORDER BY path;

-- Bill of materials (parts explosion)
WITH RECURSIVE parts_explosion AS (
    SELECT
        part_id, component_id, quantity,
        1 as level, ARRAY[part_id] as path
    FROM bill_of_materials
    WHERE part_id = 'PRODUCT-123'

    UNION ALL

    SELECT
        pe.part_id, bom.component_id,
        pe.quantity * bom.quantity,
        pe.level + 1,
        pe.path || bom.part_id
    FROM parts_explosion pe
    INNER JOIN bill_of_materials bom ON pe.component_id = bom.part_id
    WHERE NOT bom.part_id = ANY(pe.path)
)
SELECT
    component_id,
    SUM(quantity) as total_quantity,
    MAX(level) as max_depth
FROM parts_explosion
GROUP BY component_id;
```

## Advanced JOIN Patterns

```sql
-- Self-join for finding gaps in sequences
SELECT
    a.order_id as current_id,
    MIN(b.order_id) as next_id,
    MIN(b.order_id) - a.order_id - 1 as gap_size
FROM orders a
LEFT JOIN orders b ON b.order_id > a.order_id
GROUP BY a.order_id
HAVING MIN(b.order_id) - a.order_id > 1;

-- LATERAL join for correlated subqueries (PostgreSQL)
SELECT
    c.customer_id, c.name,
    recent.order_date, recent.total
FROM customers c
CROSS JOIN LATERAL (
    SELECT order_date, total
    FROM orders o
    WHERE o.customer_id = c.customer_id
    ORDER BY order_date DESC
    LIMIT 3
) recent;

-- Anti-join pattern (records in A not in B)
SELECT u.user_id, u.email
FROM users u
LEFT JOIN orders o ON u.user_id = o.user_id
WHERE o.order_id IS NULL;

-- Using EXISTS (more efficient for large sets)
SELECT u.user_id, u.email
FROM users u
WHERE NOT EXISTS (
    SELECT 1 FROM orders o WHERE o.user_id = u.user_id
);
```

## Subquery Optimization

```sql
-- Bad: Scalar subquery in SELECT (N+1)
SELECT
    p.product_id, p.name,
    (SELECT COUNT(*) FROM reviews r WHERE r.product_id = p.product_id) as review_count,
    (SELECT AVG(rating) FROM reviews r WHERE r.product_id = p.product_id) as avg_rating
FROM products p;

-- Good: Single JOIN with aggregation
SELECT
    p.product_id, p.name,
    COALESCE(r.review_count, 0) as review_count,
    r.avg_rating
FROM products p
LEFT JOIN (
    SELECT product_id, COUNT(*) as review_count, AVG(rating) as avg_rating
    FROM reviews
    GROUP BY product_id
) r ON p.product_id = r.product_id;

-- Better: Use window functions for correlated filtering
SELECT order_id, customer_id, total
FROM (
    SELECT
        order_id, customer_id, total,
        AVG(total) OVER (PARTITION BY customer_id) as avg_customer_total
    FROM orders
) x
WHERE total > avg_customer_total;
```

## PIVOT/UNPIVOT Operations

```sql
-- PostgreSQL CROSSTAB (requires tablefunc extension)
CREATE EXTENSION IF NOT EXISTS tablefunc;

SELECT * FROM crosstab(
    'SELECT customer_id, product_category, SUM(amount)
     FROM sales GROUP BY customer_id, product_category
     ORDER BY customer_id, product_category',
    'SELECT DISTINCT product_category FROM sales ORDER BY 1'
) AS ct(customer_id INT, electronics NUMERIC, clothing NUMERIC, food NUMERIC);

-- Manual PIVOT with CASE (works on all platforms)
SELECT
    customer_id,
    SUM(CASE WHEN product_category = 'electronics' THEN amount ELSE 0 END) as electronics,
    SUM(CASE WHEN product_category = 'clothing' THEN amount ELSE 0 END) as clothing,
    SUM(CASE WHEN product_category = 'food' THEN amount ELSE 0 END) as food
FROM sales
GROUP BY customer_id;

-- UNPIVOT pattern (columns to rows)
SELECT customer_id, 'electronics' as category, electronics as amount
FROM customer_sales WHERE electronics > 0
UNION ALL
SELECT customer_id, 'clothing', clothing
FROM customer_sales WHERE clothing > 0
UNION ALL
SELECT customer_id, 'food', food
FROM customer_sales WHERE food > 0;
```

## Set Operations

```sql
-- UNION for combining distinct results
SELECT product_id FROM active_products
UNION
SELECT product_id FROM featured_products;

-- UNION ALL for better performance (includes duplicates)
SELECT user_id, 'signup' as event FROM signups WHERE date = CURRENT_DATE
UNION ALL
SELECT user_id, 'purchase' as event FROM purchases WHERE date = CURRENT_DATE;

-- INTERSECT for common records
SELECT email FROM newsletter_subscribers
INTERSECT
SELECT email FROM premium_members;

-- EXCEPT for difference (A - B)
SELECT email FROM all_users
EXCEPT
SELECT email FROM unsubscribed_users;
```

## Window Functions — Ranking

```sql
-- ROW_NUMBER: Sequential numbering within partition
SELECT
    customer_id, order_date, total,
    ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY order_date DESC) as row_num
FROM orders;

-- Get most recent order per customer (Top-1-per-group)
SELECT * FROM (
    SELECT
        customer_id, order_id, order_date, total,
        ROW_NUMBER() OVER (PARTITION BY customer_id ORDER BY order_date DESC) as rn
    FROM orders
) ranked
WHERE rn = 1;

-- RANK vs DENSE_RANK vs ROW_NUMBER
SELECT
    student_id, score,
    RANK() OVER (ORDER BY score DESC) as rank,
    DENSE_RANK() OVER (ORDER BY score DESC) as dense_rank,
    ROW_NUMBER() OVER (ORDER BY score DESC) as row_num
FROM exam_results;
-- score=100: rank=1, dense_rank=1, row_num=1
-- score=100: rank=1, dense_rank=1, row_num=2
-- score=95:  rank=3, dense_rank=2, row_num=3

-- NTILE: Divide into N buckets
SELECT
    customer_id, total_spent,
    NTILE(4) OVER (ORDER BY total_spent DESC) as quartile
FROM customer_lifetime_value;
```

## Window Functions — Aggregates

```sql
-- Running totals and moving averages
SELECT
    order_date,
    daily_revenue,
    SUM(daily_revenue) OVER (ORDER BY order_date) as cumulative_revenue,
    AVG(daily_revenue) OVER (
        ORDER BY order_date
        ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
    ) as rolling_7day_avg
FROM daily_sales;

-- Moving average with RANGE (date-based)
SELECT
    sale_date, amount,
    AVG(amount) OVER (
        ORDER BY sale_date
        RANGE BETWEEN INTERVAL '7 days' PRECEDING AND CURRENT ROW
    ) as avg_last_7_days
FROM sales;

-- Partition-specific aggregates
SELECT
    product_id, sale_date, quantity,
    SUM(quantity) OVER (PARTITION BY product_id ORDER BY sale_date) as cumulative_qty,
    AVG(quantity) OVER (PARTITION BY product_id) as avg_qty_for_product,
    quantity::FLOAT / SUM(quantity) OVER (PARTITION BY product_id) as pct_of_total
FROM product_sales;
```

## Window Functions — LAG/LEAD

```sql
-- Compare with previous/next row
SELECT
    order_date, total,
    LAG(total) OVER (ORDER BY order_date) as previous_day_total,
    LEAD(total) OVER (ORDER BY order_date) as next_day_total,
    total - LAG(total) OVER (ORDER BY order_date) as day_over_day_change
FROM daily_orders;

-- Find gaps in time series
SELECT event_date, prev_date, days_since_last
FROM (
    SELECT
        event_date,
        LAG(event_date) OVER (ORDER BY event_date) as prev_date,
        event_date - LAG(event_date) OVER (ORDER BY event_date) as days_since_last
    FROM events
) x
WHERE days_since_last > 7;

-- Session analysis with time gaps
SELECT
    user_id, action_time,
    LAG(action_time) OVER (PARTITION BY user_id ORDER BY action_time) as prev_action,
    EXTRACT(EPOCH FROM (
        action_time - LAG(action_time) OVER (PARTITION BY user_id ORDER BY action_time)
    )) / 60 as minutes_since_last_action,
    CASE
        WHEN EXTRACT(EPOCH FROM (
            action_time - LAG(action_time) OVER (PARTITION BY user_id ORDER BY action_time)
        )) / 60 > 30 THEN 1
        ELSE 0
    END as new_session
FROM user_actions;
```

## Window Functions — FIRST_VALUE/LAST_VALUE

```sql
SELECT
    product_id, price_date, price,
    FIRST_VALUE(price) OVER (
        PARTITION BY product_id ORDER BY price_date
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ) as initial_price,
    LAST_VALUE(price) OVER (
        PARTITION BY product_id ORDER BY price_date
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ) as current_price,
    price - FIRST_VALUE(price) OVER (
        PARTITION BY product_id ORDER BY price_date
        ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING
    ) as price_change_from_start
FROM product_price_history;
```

## Frame Specifications

```sql
-- ROWS vs RANGE difference
SELECT
    order_date, amount,
    -- ROWS: Physical row offset
    SUM(amount) OVER (
        ORDER BY order_date
        ROWS BETWEEN 2 PRECEDING AND 2 FOLLOWING
    ) as sum_5_rows,
    -- RANGE: Logical value range
    SUM(amount) OVER (
        ORDER BY order_date
        RANGE BETWEEN INTERVAL '2 days' PRECEDING AND INTERVAL '2 days' FOLLOWING
    ) as sum_5_day_range
FROM orders;

-- Common frame patterns
SELECT
    sale_date, revenue,
    SUM(revenue) OVER (ORDER BY sale_date ROWS UNBOUNDED PRECEDING) as running_total,
    AVG(revenue) OVER (ORDER BY sale_date ROWS BETWEEN 2 PRECEDING AND CURRENT ROW) as ma_3,
    SUM(revenue) OVER (PARTITION BY EXTRACT(YEAR FROM sale_date)) as yearly_total,
    AVG(revenue) OVER (ORDER BY sale_date ROWS BETWEEN 3 PRECEDING AND 3 FOLLOWING) as centered_ma_7
FROM sales;
```

## Advanced Analytics

```sql
-- Percentile calculations
SELECT
    employee_id, salary,
    PERCENT_RANK() OVER (ORDER BY salary) as pct_rank,
    CUME_DIST() OVER (ORDER BY salary) as cumulative_dist,
    PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY salary) OVER () as median_salary,
    PERCENTILE_DISC(0.9) WITHIN GROUP (ORDER BY salary) OVER () as p90_salary
FROM employees;

-- Cohort retention analysis
WITH user_cohorts AS (
    SELECT
        user_id,
        DATE_TRUNC('month', signup_date) as cohort_month,
        DATE_TRUNC('month', activity_date) as activity_month
    FROM user_activity
),
cohort_sizes AS (
    SELECT cohort_month, COUNT(DISTINCT user_id) as cohort_size
    FROM user_cohorts
    GROUP BY cohort_month
)
SELECT
    uc.cohort_month,
    uc.activity_month,
    EXTRACT(MONTH FROM AGE(uc.activity_month, uc.cohort_month)) as months_since_signup,
    COUNT(DISTINCT uc.user_id) as active_users,
    cs.cohort_size,
    ROUND(100.0 * COUNT(DISTINCT uc.user_id) / cs.cohort_size, 2) as retention_pct
FROM user_cohorts uc
JOIN cohort_sizes cs ON uc.cohort_month = cs.cohort_month
GROUP BY uc.cohort_month, uc.activity_month, cs.cohort_size
ORDER BY uc.cohort_month, months_since_signup;

-- Time-series gap filling
SELECT
    date_series.date,
    COALESCE(s.revenue, 0) as revenue,
    AVG(s.revenue) OVER (
        ORDER BY date_series.date
        ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
    ) as ma_7day
FROM generate_series(
    '2024-01-01'::DATE,
    '2024-12-31'::DATE,
    '1 day'::INTERVAL
) AS date_series(date)
LEFT JOIN sales s ON date_series.date = s.sale_date;
```

## Conditional Aggregation with FILTER

```sql
-- PostgreSQL FILTER clause
SELECT
    product_id, sale_date, quantity,
    SUM(quantity) FILTER (WHERE quantity > 10) OVER (
        PARTITION BY product_id ORDER BY sale_date
    ) as cumulative_large_orders,
    COUNT(*) FILTER (WHERE quantity > 100) OVER (
        PARTITION BY product_id
    ) as total_bulk_orders
FROM sales;
```

## Common Patterns Summary

1. **Top N per Group**: `ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...) WHERE rn <= N`
2. **Running Totals**: `SUM() OVER (ORDER BY date)`
3. **Moving Averages**: `AVG() OVER (ROWS BETWEEN N PRECEDING AND CURRENT ROW)`
4. **Session Analysis**: `LAG()` to detect time gaps
5. **Deduplication**: `ROW_NUMBER() OVER (PARTITION BY key ORDER BY priority) WHERE rn = 1`
6. **Percentiles**: `PERCENT_RANK()` or `PERCENTILE_CONT()`
7. **Year-over-Year**: `LAG(value, 12) OVER (ORDER BY month)`
8. **Cohort Analysis**: `PARTITION BY cohort_date`, aggregate over activity periods
