-- Plenum live-test seed schema — PostgreSQL dialect (REF-274 / REF-275).
-- Runs via /docker-entrypoint-initdb.d/ on a fresh container, connected to
-- POSTGRES_DB (plenum_test) as POSTGRES_USER (plenum).
--
-- This file is PostgreSQL-specific by design. Do NOT share SQL with the
-- MySQL seed (tests/live/seed/mysql/) — engine isolation is a project rule.
--
-- The timeout_ms test path needs no seed object: the built-in pg_sleep()
-- (e.g. `SELECT pg_sleep(5)`) provides the slow query.

CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');

-- One column per interesting type family: integers, fixed/floating point,
-- strings, date/time (with and without tz), binary, booleans, and the
-- PostgreSQL-specific enum / array / JSONB features.
CREATE TABLE type_matrix (
    id            integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    c_smallint    smallint,
    c_integer     integer,
    c_bigint      bigint,
    c_numeric     numeric(12, 4),
    c_real        real,
    c_double      double precision,
    c_varchar     varchar(120),
    c_text        text,
    c_date        date,
    c_time        time,
    c_timestamp   timestamp,
    c_timestamptz timestamptz,
    c_bytea       bytea,
    c_bool        boolean,
    c_mood        mood,
    c_tags        text[],
    c_matrix      integer[][],
    c_jsonb       jsonb,
    c_json        json
);

CREATE TABLE customers (
    id    integer      NOT NULL PRIMARY KEY,
    name  varchar(80)  NOT NULL,
    email varchar(120) NOT NULL,
    CONSTRAINT uq_customers_email UNIQUE (email)
);

-- Composite primary key so order_items can carry a composite foreign key.
CREATE TABLE orders (
    customer_id integer   NOT NULL,
    order_no    integer   NOT NULL,
    status      text      NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'shipped', 'cancelled')),
    placed_at   timestamp NOT NULL,
    PRIMARY KEY (customer_id, order_no),
    CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
);

CREATE TABLE order_items (
    customer_id integer        NOT NULL,
    order_no    integer        NOT NULL,
    line_no     integer        NOT NULL,
    sku         varchar(40)    NOT NULL,
    qty         integer        NOT NULL,
    unit_price  numeric(10, 2) NOT NULL,
    PRIMARY KEY (customer_id, order_no, line_no),
    CONSTRAINT fk_order_items_order
        FOREIGN KEY (customer_id, order_no)
        REFERENCES orders (customer_id, order_no)
);

CREATE INDEX idx_order_items_sku ON order_items (sku);

CREATE VIEW v_order_totals AS
SELECT o.customer_id,
       o.order_no,
       o.status,
       SUM(i.qty * i.unit_price) AS total
FROM orders o
JOIN order_items i
  ON i.customer_id = o.customer_id AND i.order_no = o.order_no
GROUP BY o.customer_id, o.order_no, o.status;

-- Deterministic >1,000-row table for max_rows truncation tests.
CREATE TABLE bulk_rows (
    n     integer     NOT NULL PRIMARY KEY,
    label varchar(32) NOT NULL
);

CREATE INDEX idx_bulk_rows_label ON bulk_rows (label);

-- Second schema so multi-schema introspection has something to find.
CREATE SCHEMA analytics;

CREATE TABLE analytics.page_views (
    id        integer GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    path      text        NOT NULL,
    viewed_at timestamptz NOT NULL,
    meta      jsonb
);
