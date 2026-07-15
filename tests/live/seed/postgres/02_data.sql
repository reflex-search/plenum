-- Plenum live-test seed data — PostgreSQL dialect (REF-274 / REF-275).
-- Deterministic fixed values only; no now()/random() so identical inputs
-- always yield identical query results.

INSERT INTO type_matrix
    (c_smallint, c_integer, c_bigint,
     c_numeric, c_real, c_double,
     c_varchar, c_text,
     c_date, c_time, c_timestamp, c_timestamptz,
     c_bytea, c_bool, c_mood,
     c_tags, c_matrix, c_jsonb, c_json)
VALUES
    -- Boundary-ish values with emoji strings and a fixed-offset timestamptz.
    (32767, 2147483647, 9223372036854775807,
     12345678.9999, 3.5, 2.718281828459045,
     'café résumé 🚀', E'multi-line\ntext with emoji 🎉 and quotes ''"',
     '2024-01-15', '13:45:30', '2024-01-15 13:45:30', '2024-01-15 13:45:30+00',
     '\xdeadbeef', true, 'happy',
     ARRAY['red', 'green 🌿', 'blue'], ARRAY[[1, 2], [3, 4]],
     '{"kind": "demo", "nested": {"n": 1}, "arr": [1, 2, 3]}', '[true, false, null]'),
    -- Negative / small values.
    (-32768, -2147483648, -9223372036854775808,
     -0.0001, -1.25, -6.62607015,
     'plain ascii', 'short',
     '1999-12-31', '00:00:00', '1999-12-31 23:59:59', '2000-01-01 00:00:00+00',
     '\x00', false, 'sad',
     ARRAY[]::text[], NULL,
     '{}', 'null'),
    -- All-NULL row.
    (NULL, NULL, NULL,
     NULL, NULL, NULL,
     NULL, NULL,
     NULL, NULL, NULL, NULL,
     NULL, NULL, NULL,
     NULL, NULL, NULL, NULL);

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
INSERT INTO bulk_rows (n, label)
SELECT n, 'row-' || lpad(n::text, 4, '0')
FROM generate_series(1, 1500) AS n;

INSERT INTO analytics.page_views (path, viewed_at, meta) VALUES
    ('/home',        '2024-03-01 08:00:00+00', '{"ref": "direct"}'),
    ('/docs/plenum', '2024-03-01 08:05:00+00', '{"ref": "search", "q": "agent db cli"}'),
    ('/pricing',     '2024-03-01 08:10:00+00', NULL);
