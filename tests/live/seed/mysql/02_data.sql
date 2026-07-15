-- Plenum live-test seed data — MySQL dialect (REF-274 / REF-275).
-- Deterministic fixed values only; no NOW()/RAND() so identical inputs
-- always yield identical query results.

USE plenum_test;
SET NAMES utf8mb4;

INSERT INTO type_matrix
    (c_tinyint, c_smallint, c_mediumint, c_int, c_bigint,
     c_decimal, c_float, c_double,
     c_varchar, c_text,
     c_date, c_time, c_datetime, c_timestamp,
     c_binary, c_varbinary, c_blob,
     c_enum, c_set, c_json)
VALUES
    -- Boundary-ish values with utf8mb4 emoji strings.
    (127, 32767, 8388607, 2147483647, 9223372036854775807,
     12345678.9999, 3.5, 2.718281828459045,
     'café résumé 🚀', 'multi-line\ntext with emoji 🎉 and quotes ''"',
     '2024-01-15', '13:45:30', '2024-01-15 13:45:30', '2024-01-15 13:45:30',
     x'DEADBEEF', x'0123456789ABCDEF', x'00FF00FF',
     'green', 'alpha,gamma', '{"kind": "demo", "nested": {"n": 1}, "arr": [1, 2, 3]}'),
    -- Negative / small values.
    (-128, -32768, -8388608, -2147483648, -9223372036854775808,
     -0.0001, -1.25, -6.62607015,
     'plain ascii', 'short',
     '1999-12-31', '00:00:00', '1999-12-31 23:59:59', '2000-01-01 00:00:00',
     x'00000000', x'FF', x'0102030405',
     'red', 'beta', '[true, false, null]'),
    -- All-NULL row (except the required generated-column source stays NULL too).
    (NULL, NULL, NULL, NULL, NULL,
     NULL, NULL, NULL,
     NULL, NULL,
     NULL, NULL, NULL, NULL,
     NULL, NULL, NULL,
     NULL, NULL, NULL);

INSERT INTO customers (id, name, email) VALUES
    (1, 'Ada Lovelace', 'ada@example.com'),
    (2, 'Grace Hopper 🌟', 'grace@example.com'),
    (3, 'Annie Easley', 'annie@example.com');

INSERT INTO orders (customer_id, order_no, status, placed_at) VALUES
    (1, 1, 'shipped',   '2024-02-01 09:00:00'),
    (1, 2, 'pending',   '2024-02-03 10:30:00'),
    (2, 1, 'cancelled', '2024-02-05 16:45:00');

INSERT INTO order_items (customer_id, order_no, line_no, sku, qty, unit_price) VALUES
    (1, 1, 1, 'SKU-0001', 2, 19.99),
    (1, 1, 2, 'SKU-0002', 1, 5.00),
    (1, 2, 1, 'SKU-0003', 4, 2.50),
    (2, 1, 1, 'SKU-0001', 1, 19.99);

-- 1,500 deterministic rows for max_rows truncation tests.
SET SESSION cte_max_recursion_depth = 5000;
INSERT INTO bulk_rows (n, label)
WITH RECURSIVE seq (n) AS (
    SELECT 1
    UNION ALL
    SELECT n + 1 FROM seq WHERE n < 1500
)
SELECT n, CONCAT('row-', LPAD(n, 4, '0')) FROM seq;
