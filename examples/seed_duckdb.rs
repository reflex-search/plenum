//! Dev-only seeder for the `DuckDB` evidence demo (`make duckdb-evidence`).
//!
//! Creates a seeded `.duckdb` file mirroring the logical dataset used by the
//! `MySQL` / `PostgreSQL` live seeds and the `duckdb_parity` fixture, so the
//! release `plenum` binary can be exercised end-to-end against it.
//!
//! This is an example (never shipped in release artifacts) because plenum
//! itself is strictly read-only and cannot seed a database.
//!
//! Usage: `cargo run --example seed_duckdb -- <path.duckdb>`

use duckdb::Connection;

fn main() {
    let path = std::env::args().nth(1).expect("usage: seed_duckdb <path.duckdb>");
    assert!(
        path.ends_with(".duckdb"),
        "refusing to touch a path that does not end in .duckdb: {path}"
    );

    // Deterministic: always rebuild from scratch.
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }

    let conn = Connection::open(&path).expect("create demo DB");

    conn.execute_batch(
        "CREATE TABLE customers (
            id    INTEGER NOT NULL,
            name  VARCHAR NOT NULL,
            email VARCHAR NOT NULL,
            PRIMARY KEY (id)
        );
        CREATE UNIQUE INDEX uq_customers_email ON customers(email);
        INSERT INTO customers (id, name, email) VALUES
            (1, 'Ada Lovelace',    'ada@example.com'),
            (2, 'Grace Hopper 🌟', 'grace@example.com'),
            (3, 'Annie Easley',    'annie@example.com');

        CREATE TABLE orders (
            customer_id INTEGER   NOT NULL,
            order_no    INTEGER   NOT NULL,
            status      VARCHAR   NOT NULL DEFAULT 'pending',
            placed_at   TIMESTAMP NOT NULL,
            PRIMARY KEY (customer_id, order_no),
            FOREIGN KEY (customer_id) REFERENCES customers(id)
        );
        INSERT INTO orders (customer_id, order_no, status, placed_at) VALUES
            (1, 1, 'shipped',   TIMESTAMP '2024-02-01 09:00:00'),
            (1, 2, 'pending',   TIMESTAMP '2024-02-03 10:30:00'),
            (2, 1, 'cancelled', TIMESTAMP '2024-02-05 16:45:00');

        CREATE TABLE order_items (
            customer_id INTEGER       NOT NULL,
            order_no    INTEGER       NOT NULL,
            line_no     INTEGER       NOT NULL,
            sku         VARCHAR       NOT NULL,
            qty         INTEGER       NOT NULL,
            unit_price  DECIMAL(10,2) NOT NULL,
            PRIMARY KEY (customer_id, order_no, line_no),
            FOREIGN KEY (customer_id, order_no)
                REFERENCES orders(customer_id, order_no)
        );
        CREATE INDEX idx_order_items_sku ON order_items(sku);
        INSERT INTO order_items
            (customer_id, order_no, line_no, sku, qty, unit_price)
        VALUES
            (1, 1, 1, 'SKU-0001', 2, 19.99),
            (1, 1, 2, 'SKU-0002', 1, 5.00),
            (1, 2, 1, 'SKU-0003', 4, 2.50),
            (2, 1, 1, 'SKU-0001', 1, 19.99);

        CREATE TABLE bulk_rows AS
        SELECT CAST(range + 1 AS INTEGER) AS n,
               printf('row-%04d', range + 1) AS label
        FROM range(1500);

        CREATE VIEW v_order_totals AS
        SELECT o.customer_id,
               o.order_no,
               o.status,
               SUM(i.qty * i.unit_price) AS total
        FROM orders o
        JOIN order_items i
          ON i.customer_id = o.customer_id AND i.order_no = o.order_no
        GROUP BY o.customer_id, o.order_no, o.status;",
    )
    .expect("seed demo DB");

    eprintln!("seeded {path}");
}
