-- Plenum live-test seed schema — MySQL dialect (REF-274 / REF-275).
-- Runs via /docker-entrypoint-initdb.d/ on a fresh container; the database
-- plenum_test is created by the entrypoint from MYSQL_DATABASE.
--
-- This file is MySQL-specific by design. Do NOT share SQL with the
-- PostgreSQL seed (tests/live/seed/postgres/) — engine isolation is a
-- project rule.
--
-- The timeout_ms test path needs no seed object: MySQL's built-in SLEEP()
-- (e.g. `SELECT SLEEP(5)`) provides the slow query.

USE plenum_test;
SET NAMES utf8mb4;

-- One column per interesting type family: integer widths, fixed/floating
-- point, utf8mb4 strings, date/time/timestamp, binary, and the
-- MySQL-specific ENUM / SET / JSON / generated-column features.
CREATE TABLE type_matrix (
    id           INT           NOT NULL AUTO_INCREMENT,
    c_tinyint    TINYINT,
    c_smallint   SMALLINT,
    c_mediumint  MEDIUMINT,
    c_int        INT,
    c_bigint     BIGINT,
    c_decimal    DECIMAL(12, 4),
    c_float      FLOAT,
    c_double     DOUBLE,
    c_varchar    VARCHAR(120),
    c_text       TEXT,
    c_date       DATE,
    c_time       TIME,
    c_datetime   DATETIME,
    c_timestamp  TIMESTAMP     NULL DEFAULT NULL,
    c_binary     BINARY(4),
    c_varbinary  VARBINARY(16),
    c_blob       BLOB,
    c_enum       ENUM('red', 'green', 'blue'),
    c_set        SET('alpha', 'beta', 'gamma'),
    c_json       JSON,
    c_generated  BIGINT        AS (c_int * 2) STORED,
    PRIMARY KEY (id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE customers (
    id    INT          NOT NULL,
    name  VARCHAR(80)  NOT NULL,
    email VARCHAR(120) NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY uq_customers_email (email)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

-- Composite primary key so order_items can carry a composite foreign key.
CREATE TABLE orders (
    customer_id INT      NOT NULL,
    order_no    INT      NOT NULL,
    status      ENUM('pending', 'shipped', 'cancelled') NOT NULL DEFAULT 'pending',
    placed_at   DATETIME NOT NULL,
    PRIMARY KEY (customer_id, order_no),
    CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

CREATE TABLE order_items (
    customer_id INT            NOT NULL,
    order_no    INT            NOT NULL,
    line_no     INT            NOT NULL,
    sku         VARCHAR(40)    NOT NULL,
    qty         INT            NOT NULL,
    unit_price  DECIMAL(10, 2) NOT NULL,
    PRIMARY KEY (customer_id, order_no, line_no),
    KEY idx_order_items_sku (sku),
    CONSTRAINT fk_order_items_order
        FOREIGN KEY (customer_id, order_no)
        REFERENCES orders (customer_id, order_no)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;

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
    n     INT         NOT NULL,
    label VARCHAR(32) NOT NULL,
    PRIMARY KEY (n),
    KEY idx_bulk_rows_label (label)
) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;
