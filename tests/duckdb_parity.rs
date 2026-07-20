//! `DuckDB` offline parity suite — REF-290.
//!
//! Brings the `DuckDB` test coverage to parity with the `SQLite` offline suite
//! (REF-278) and the live-DB coverage matrix (`MySQL` / `PostgreSQL`). The
//! fixture dataset mirrors the logical dataset from the live seed scripts:
//!   `type_matrix`, customers, orders, `order_items`, `bulk_rows`, `v_order_totals`.
//!
//! All tests run offline — no Docker required.  Plain `cargo test` includes
//! them all.  Uses the `DuckDbEngine` API directly; no CLI binary is spawned.
//!
//! Coverage matrix:
//!   connect      — valid path; nonexistent file → normalized error envelope
//!   introspect   — tables, columns + native types, PK, composite FK, indexes,
//!                  views, schemas, comments, row estimates; stable JSON shape
//!   query allowed — SELECT, EXPLAIN, EXPLAIN (FORMAT JSON), SHOW, DESCRIBE,
//!                  SUMMARIZE, PRAGMA allowlist, transaction control
//!   query denied  — INSERT / UPDATE / DELETE / CREATE / DROP / ALTER / COPY /
//!                  ATTACH → `CAPABILITY_VIOLATION` before execution, then
//!                  re-query to prove DB state unchanged
//!   safety       — `max_rows` truncation + `rows_truncated` flag; `timeout_ms`
//!                  (interrupt handle) tested for fast and long queries
//!   envelope     — `QueryResult` / `IntrospectResult` serialize to valid JSON;
//!                  deterministic with `execution_ms` excluded

#![cfg(feature = "duckdb")]

use plenum::engine::duckdb::DuckDbEngine;
use plenum::engine::{IntrospectOperation, IntrospectResult, TableFields};
use plenum::{Capabilities, ConnectionConfig, DatabaseEngine};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

// ============================================================================
// Fixture helpers
// ============================================================================

static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fixture_path(tag: &str) -> PathBuf {
    let id = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("plenum_duckdb_parity_{tag}_{pid}_{id}.duckdb"))
}

/// Build the full parity fixture dataset into a fresh temp file.
///
/// Mirrors the logical dataset from the `MySQL` / `PostgreSQL` seed scripts
/// and the `SQLite` parity fixture (REF-278), using `DuckDB` native types:
///   - `type_matrix`   — one column per interesting `DuckDB` type
///   - `customers`     — simple PK + UNIQUE index on email, emoji in data
///   - `orders`        — composite PK, FK → customers
///   - `order_items`   — composite 3-col PK, composite FK → orders, index on sku
///   - `bulk_rows`     — 1 500 deterministic rows for `max_rows` tests
///   - `v_order_totals` — view over orders + `order_items`
fn build_parity_fixture() -> PathBuf {
    use duckdb::Connection;

    let path = fixture_path("fixture");
    let _ = std::fs::remove_file(&path);

    let conn = Connection::open(&path).expect("create fixture DB");

    // ------------------------------------------------------------------
    // type_matrix — one column per interesting DuckDB type. Mirrors the
    // logical columns from the live seeds (INT, DOUBLE, DECIMAL, TEXT, BLOB,
    // nullable TEXT, DATE / TIME / TIMESTAMP as native types) plus
    // DuckDB-specific HUGEINT, BOOLEAN, and LIST.
    // ------------------------------------------------------------------
    conn.execute_batch(
        "CREATE TABLE type_matrix (
            id         INTEGER PRIMARY KEY,
            c_integer  BIGINT,
            c_double   DOUBLE,
            c_decimal  DECIMAL(18,4),
            c_hugeint  HUGEINT,
            c_bool     BOOLEAN,
            c_text     VARCHAR,
            c_blob     BLOB,
            c_null_col VARCHAR,
            c_date     DATE,
            c_time     TIME,
            c_datetime TIMESTAMP,
            c_list     INTEGER[]
        );

        -- Row 1: boundary / positive values, emoji string, BLOB bytes
        INSERT INTO type_matrix VALUES (
            1,
            9223372036854775807,
            2.123456789012345,
            12345678.9999,
            170141183460469231731687303715884105727,
            TRUE,
            'café résumé 🚀',
            '\\xDE\\xAD\\xBE\\xEF'::BLOB,
            NULL,
            DATE '2024-01-15',
            TIME '13:45:30',
            TIMESTAMP '2024-01-15 13:45:30',
            [1, 2, 3]
        );

        -- Row 2: negative / small values
        INSERT INTO type_matrix VALUES (
            2,
            -9223372036854775807,
            -6.62607015,
            -0.0001,
            -170141183460469231731687303715884105727,
            FALSE,
            'plain ascii',
            '\\x00\\x01\\x02\\x03'::BLOB,
            NULL,
            DATE '1999-12-31',
            TIME '00:00:00',
            TIMESTAMP '1999-12-31 23:59:59',
            []
        );

        -- Row 3: all-NULL except id
        INSERT INTO type_matrix (id) VALUES (3);",
    )
    .expect("create + seed type_matrix");

    // ------------------------------------------------------------------
    // customers — PK on id, UNIQUE index on email, emoji in data
    // ------------------------------------------------------------------
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
            (3, 'Annie Easley',    'annie@example.com');",
    )
    .expect("create + seed customers");

    // ------------------------------------------------------------------
    // orders — composite PK (customer_id, order_no), FK → customers
    // ------------------------------------------------------------------
    conn.execute_batch(
        "CREATE TABLE orders (
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
            (2, 1, 'cancelled', TIMESTAMP '2024-02-05 16:45:00');",
    )
    .expect("create + seed orders");

    // ------------------------------------------------------------------
    // order_items — 3-col composite PK, composite FK → orders, index on sku
    // ------------------------------------------------------------------
    conn.execute_batch(
        "CREATE TABLE order_items (
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
            (2, 1, 1, 'SKU-0001', 1, 19.99);",
    )
    .expect("create + seed order_items");

    // ------------------------------------------------------------------
    // bulk_rows — 1 500 deterministic rows for max_rows truncation tests
    // ------------------------------------------------------------------
    conn.execute_batch(
        "CREATE TABLE bulk_rows AS
         SELECT CAST(range + 1 AS INTEGER) AS n,
                printf('row-%04d', range + 1) AS label
         FROM range(1500);",
    )
    .expect("create + seed bulk_rows");

    // ------------------------------------------------------------------
    // v_order_totals — view over orders + order_items
    // ------------------------------------------------------------------
    conn.execute_batch(
        "CREATE VIEW v_order_totals AS
         SELECT o.customer_id,
                o.order_no,
                o.status,
                SUM(i.qty * i.unit_price) AS total
         FROM orders o
         JOIN order_items i
           ON i.customer_id = o.customer_id
          AND i.order_no    = o.order_no
         GROUP BY o.customer_id, o.order_no, o.status;",
    )
    .expect("create v_order_totals view");

    path
}

fn cleanup(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

/// Return the value of column `name` from `row`, panicking with a useful
/// message if the column is absent.
fn get_col<'a>(cols: &[String], row: &'a [serde_json::Value], name: &str) -> &'a serde_json::Value {
    let idx = cols
        .iter()
        .position(|c| c == name)
        .unwrap_or_else(|| panic!("column '{name}' not found in {cols:?}"));
    &row[idx]
}

// ============================================================================
// Connect
// ============================================================================

#[tokio::test]
async fn parity_connect_valid_path() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::validate_connection(&config).await;
    assert!(result.is_ok(), "validate_connection failed: {:?}", result.err());
    let info = result.unwrap();
    assert!(!info.database_version.is_empty(), "database_version must not be empty");
    assert!(info.server_info.contains("DuckDB"), "server_info must mention DuckDB");
    assert!(!info.connected_database.is_empty(), "connected_database must not be empty");
    cleanup(&path);
}

#[tokio::test]
async fn parity_connect_nonexistent_file() {
    let path = PathBuf::from("/nonexistent/plenum_parity_test.duckdb");
    let config = ConnectionConfig::duckdb(path);
    let result = DuckDbEngine::validate_connection(&config).await;
    assert!(result.is_err(), "expected connection failure for nonexistent path");
    let err = result.unwrap_err();
    assert_eq!(
        err.error_code(),
        "CONNECTION_FAILED",
        "nonexistent file must produce CONNECTION_FAILED, got: {}",
        err.error_code()
    );
}

// ============================================================================
// Introspect — tables
// ============================================================================

#[tokio::test]
async fn parity_introspect_list_tables() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result =
        DuckDbEngine::introspect(&config, &IntrospectOperation::ListTables, None, None).await;
    assert!(result.is_ok(), "ListTables failed: {:?}", result.err());
    let IntrospectResult::TableList { tables } = result.unwrap() else {
        panic!("Expected TableList variant");
    };
    for expected in &["type_matrix", "customers", "orders", "order_items", "bulk_rows"] {
        assert!(
            tables.iter().any(|t| t == expected),
            "table '{expected}' missing from ListTables result; got: {tables:?}"
        );
    }
    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_type_matrix_columns_and_types() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::TableDetails {
            name: "type_matrix".to_string(),
            fields: TableFields::all(),
        },
        None,
        None,
    )
    .await;
    assert!(result.is_ok(), "TableDetails(type_matrix) failed: {:?}", result.err());
    let IntrospectResult::TableDetails { table } = result.unwrap() else {
        panic!("Expected TableDetails variant");
    };

    assert_eq!(table.name, "type_matrix");
    assert_eq!(table.schema.as_deref(), Some("main"));

    let col_type: std::collections::HashMap<&str, &str> =
        table.columns.iter().map(|c| (c.name.as_str(), c.data_type.as_str())).collect();

    // Verify native DuckDB types are reported verbatim
    assert_eq!(col_type.get("c_integer").copied(), Some("BIGINT"));
    assert_eq!(col_type.get("c_double").copied(), Some("DOUBLE"));
    assert_eq!(col_type.get("c_decimal").copied(), Some("DECIMAL(18,4)"));
    assert_eq!(col_type.get("c_hugeint").copied(), Some("HUGEINT"));
    assert_eq!(col_type.get("c_bool").copied(), Some("BOOLEAN"));
    assert_eq!(col_type.get("c_text").copied(), Some("VARCHAR"));
    assert_eq!(col_type.get("c_blob").copied(), Some("BLOB"));
    assert_eq!(col_type.get("c_date").copied(), Some("DATE"));
    assert_eq!(col_type.get("c_datetime").copied(), Some("TIMESTAMP"));
    assert_eq!(col_type.get("c_list").copied(), Some("INTEGER[]"));

    // PK must be reported
    assert_eq!(
        table.primary_key.as_deref(),
        Some(["id".to_string()].as_slice()),
        "type_matrix PK must be [id]"
    );

    // c_null_col has no NOT NULL constraint → nullable
    let null_col =
        table.columns.iter().find(|c| c.name == "c_null_col").expect("c_null_col column");
    assert!(null_col.nullable, "c_null_col must be nullable");

    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_customers_pk_and_unique_index() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::TableDetails {
            name: "customers".to_string(),
            fields: TableFields::all(),
        },
        None,
        None,
    )
    .await
    .expect("TableDetails(customers) failed");
    let IntrospectResult::TableDetails { table } = result else { panic!("Expected TableDetails") };

    assert_eq!(
        table.primary_key.as_deref(),
        Some(["id".to_string()].as_slice()),
        "customers PK must be [id]"
    );

    // email must have a UNIQUE index
    let email_idx = table.indexes.iter().find(|i| i.columns.contains(&"email".to_string()));
    assert!(email_idx.is_some(), "expected a UNIQUE index on email; indexes: {:?}", table.indexes);
    assert!(email_idx.unwrap().unique, "email index must be unique");

    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_orders_composite_pk_and_fk() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::TableDetails {
            name: "orders".to_string(),
            fields: TableFields::all(),
        },
        None,
        None,
    )
    .await
    .expect("TableDetails(orders) failed");
    let IntrospectResult::TableDetails { table } = result else { panic!("Expected TableDetails") };

    let pk = table.primary_key.as_ref().expect("orders must have a PK");
    assert!(
        pk.contains(&"customer_id".to_string()) && pk.contains(&"order_no".to_string()),
        "composite PK must include customer_id and order_no; got {pk:?}"
    );

    let fk = table
        .foreign_keys
        .iter()
        .find(|fk| fk.referenced_table == "customers")
        .expect("expected FK referencing customers");
    assert!(
        fk.columns.contains(&"customer_id".to_string()),
        "FK must include customer_id; got {:?}",
        fk.columns
    );

    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_order_items_composite_fk_and_index() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::TableDetails {
            name: "order_items".to_string(),
            fields: TableFields::all(),
        },
        None,
        None,
    )
    .await
    .expect("TableDetails(order_items) failed");
    let IntrospectResult::TableDetails { table } = result else { panic!("Expected TableDetails") };

    // 3-column composite PK
    let pk = table.primary_key.as_ref().expect("order_items must have a PK");
    assert_eq!(pk.len(), 3, "composite PK must have 3 columns; got {pk:?}");
    for col in &["customer_id", "order_no", "line_no"] {
        assert!(pk.contains(&(*col).to_string()), "composite PK must contain {col}");
    }

    // Composite FK to orders
    let fk = table
        .foreign_keys
        .iter()
        .find(|fk| fk.referenced_table == "orders")
        .expect("expected composite FK referencing orders");
    assert_eq!(fk.columns.len(), 2, "composite FK must have 2 columns; got {:?}", fk.columns);

    // Index on sku
    assert!(
        table.indexes.iter().any(|i| i.columns.contains(&"sku".to_string())),
        "expected an index on sku; indexes: {:?}",
        table.indexes
    );

    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_list_views() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result =
        DuckDbEngine::introspect(&config, &IntrospectOperation::ListViews, None, None).await;
    assert!(result.is_ok(), "ListViews failed: {:?}", result.err());
    let IntrospectResult::ViewList { views } = result.unwrap() else {
        panic!("Expected ViewList variant");
    };
    assert!(
        views.contains(&"v_order_totals".to_string()),
        "v_order_totals missing from view list; got: {views:?}"
    );
    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_view_details() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::ViewDetails { name: "v_order_totals".to_string() },
        None,
        None,
    )
    .await;
    assert!(result.is_ok(), "ViewDetails failed: {:?}", result.err());
    let IntrospectResult::ViewDetails { view } = result.unwrap() else {
        panic!("Expected ViewDetails variant");
    };
    assert_eq!(view.name, "v_order_totals");
    assert!(view.definition.is_some(), "view definition must be present");
    assert!(
        view.definition.as_ref().unwrap().contains("order_items"),
        "view definition must reference order_items"
    );
    assert!(!view.columns.is_empty(), "view must report its columns");
    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_list_indexes_for_table() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(
        &config,
        &IntrospectOperation::ListIndexes { table: Some("order_items".to_string()) },
        None,
        None,
    )
    .await;
    assert!(result.is_ok(), "ListIndexes(order_items) failed: {:?}", result.err());
    let IntrospectResult::IndexList { indexes } = result.unwrap() else {
        panic!("Expected IndexList variant");
    };
    assert!(
        indexes.iter().any(|i| i.columns.contains(&"sku".to_string())),
        "expected sku index in ListIndexes result; got: {indexes:?}"
    );
    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_list_schemas_and_databases() {
    // DuckDB supports schemas (unlike SQLite) — ListSchemas and ListDatabases
    // are first-class operations.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());

    let result = DuckDbEngine::introspect(&config, &IntrospectOperation::ListSchemas, None, None)
        .await
        .expect("ListSchemas failed");
    let IntrospectResult::SchemaList { schemas } = result else { panic!("Expected SchemaList") };
    assert!(schemas.contains(&"main".to_string()), "main schema must be listed; got {schemas:?}");

    let result = DuckDbEngine::introspect(&config, &IntrospectOperation::ListDatabases, None, None)
        .await
        .expect("ListDatabases failed");
    let IntrospectResult::DatabaseList { databases } = result else {
        panic!("Expected DatabaseList")
    };
    assert!(!databases.is_empty(), "at least the connected database must be listed");

    cleanup(&path);
}

#[tokio::test]
async fn parity_introspect_stable_json_shape() {
    // Successive introspections must produce identical JSON (determinism).
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());

    let r1 = DuckDbEngine::introspect(&config, &IntrospectOperation::ListTables, None, None)
        .await
        .unwrap();
    let r2 = DuckDbEngine::introspect(&config, &IntrospectOperation::ListTables, None, None)
        .await
        .unwrap();

    let j1 = serde_json::to_string(&r1).expect("serialize r1");
    let j2 = serde_json::to_string(&r2).expect("serialize r2");
    assert_eq!(j1, j2, "successive introspect results must be identical");

    cleanup(&path);
}

// ============================================================================
// Query — allowed operations
// ============================================================================

#[tokio::test]
async fn parity_query_select_type_matrix_numeric_null_blob() {
    // Verify numeric types, emoji string, NULLs, BLOBs, HUGEINT, DECIMAL, and
    // LIST values survive the round-trip.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result = DuckDbEngine::execute(
        &config,
        "SELECT c_integer, c_double, c_decimal, c_hugeint, c_bool, c_text, c_blob, \
                c_null_col, c_list \
         FROM type_matrix ORDER BY id",
        &[],
        &caps,
    )
    .await;
    assert!(result.is_ok(), "SELECT type_matrix failed: {:?}", result.err());
    let qr = result.unwrap();
    assert_eq!(qr.rows.len(), 3, "expected 3 rows");
    assert!(qr.rows_affected.is_none(), "SELECT must not set rows_affected");

    // Row 1: non-null values
    let r1 = &qr.rows[0];
    assert!(get_col(&qr.columns, r1, "c_integer").is_number(), "c_integer must be a number");
    assert!(get_col(&qr.columns, r1, "c_double").is_number(), "c_double must be a number");
    // DECIMAL and HUGEINT are stringified to preserve precision
    assert_eq!(get_col(&qr.columns, r1, "c_decimal"), &serde_json::json!("12345678.9999"));
    assert_eq!(
        get_col(&qr.columns, r1, "c_hugeint"),
        &serde_json::json!("170141183460469231731687303715884105727")
    );
    assert_eq!(get_col(&qr.columns, r1, "c_bool"), &serde_json::json!(true));
    let text = get_col(&qr.columns, r1, "c_text").as_str().expect("c_text must be string");
    assert!(text.contains('🚀'), "emoji must survive round-trip; got: {text:?}");
    // BLOB must be base64-encoded
    assert_eq!(get_col(&qr.columns, r1, "c_blob"), &serde_json::json!("3q2+7w=="));
    assert!(get_col(&qr.columns, r1, "c_null_col").is_null(), "c_null_col must be JSON null");
    assert_eq!(get_col(&qr.columns, r1, "c_list"), &serde_json::json!([1, 2, 3]));

    // Row 3: all-NULL
    let r3 = &qr.rows[2];
    for col in ["c_integer", "c_double", "c_decimal", "c_hugeint", "c_text", "c_blob", "c_list"] {
        assert!(get_col(&qr.columns, r3, col).is_null(), "row-3 {col} must be null");
    }

    cleanup(&path);
}

#[tokio::test]
async fn parity_query_select_customers_emoji_survives() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let qr =
        DuckDbEngine::execute(&config, "SELECT id, name FROM customers ORDER BY id", &[], &caps)
            .await
            .expect("SELECT customers failed");
    assert_eq!(qr.rows.len(), 3);
    let name2 = get_col(&qr.columns, &qr.rows[1], "name").as_str().unwrap();
    assert!(name2.contains('🌟'), "emoji in customer name must survive; got: {name2:?}");
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_select_view() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result = DuckDbEngine::execute(
        &config,
        "SELECT customer_id, order_no, total \
         FROM v_order_totals ORDER BY customer_id, order_no",
        &[],
        &caps,
    )
    .await;
    assert!(result.is_ok(), "SELECT from view failed: {:?}", result.err());
    assert!(!result.unwrap().rows.is_empty(), "v_order_totals must return rows");
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_explain_select_allowed() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result =
        DuckDbEngine::execute(&config, "EXPLAIN SELECT * FROM customers", &[], &caps).await;
    assert!(result.is_ok(), "EXPLAIN SELECT must be allowed: {:?}", result.err());
    assert!(!result.unwrap().rows.is_empty(), "EXPLAIN must return plan rows");
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_structured_explain() {
    // DuckDB structured explain uses EXPLAIN (FORMAT JSON) and normalizes to
    // the engine-stable ExplainPlanNode tree.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities {
        explain_format: Some(plenum::ExplainFormat::Structured),
        ..Capabilities::default()
    };
    let result =
        DuckDbEngine::execute(&config, "EXPLAIN SELECT * FROM customers WHERE id = 1", &[], &caps)
            .await;
    assert!(result.is_ok(), "structured EXPLAIN failed: {:?}", result.err());
    let qr = result.unwrap();
    let plan = qr.plan.expect("structured explain must populate plan");
    assert_eq!(plan.node_type, "QUERY PLAN");
    assert!(!plan.children.is_empty(), "plan must have child nodes");
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_show_describe_summarize_allowed() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();

    for sql in ["SHOW TABLES", "DESCRIBE customers", "SUMMARIZE customers"] {
        let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;
        assert!(result.is_ok(), "'{sql}' must be allowed: {:?}", result.err());
        assert!(!result.unwrap().rows.is_empty(), "'{sql}' must return rows");
    }
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_pragma_table_info_allowed() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result = DuckDbEngine::execute(&config, "PRAGMA table_info('customers')", &[], &caps).await;
    assert!(result.is_ok(), "PRAGMA table_info must be allowed: {:?}", result.err());
    let qr = result.unwrap();
    assert!(!qr.rows.is_empty(), "PRAGMA table_info must return column rows");
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_pragma_database_list_allowed() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result = DuckDbEngine::execute(&config, "PRAGMA database_list", &[], &caps).await;
    assert!(result.is_ok(), "PRAGMA database_list must be allowed: {:?}", result.err());
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_pragma_setter_rejected() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let result = DuckDbEngine::execute(&config, "PRAGMA memory_limit='1GB'", &[], &caps).await;
    assert!(result.is_err(), "setter PRAGMA must be rejected");
    assert_eq!(result.unwrap_err().error_code(), "CAPABILITY_VIOLATION");
    cleanup(&path);
}

/// Verify that a transaction control statement is NOT rejected as a
/// `CAPABILITY_VIOLATION` by the Plenum capability checker.  `DuckDB`-level
/// errors (e.g. "no transaction is active" on ROLLBACK without BEGIN) are
/// acceptable here; only `CAPABILITY_VIOLATION` is forbidden.
async fn assert_not_capability_violation(config: &ConnectionConfig, sql: &str) {
    let result = DuckDbEngine::execute(config, sql, &[], &Capabilities::default()).await;
    if let Err(ref err) = result {
        assert_ne!(
            err.error_code(),
            "CAPABILITY_VIOLATION",
            "'{sql}' must not be rejected as CAPABILITY_VIOLATION; got error_code={}",
            err.error_code()
        );
    }
}

#[tokio::test]
async fn parity_query_transaction_begin_not_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::execute(&config, "BEGIN", &[], &Capabilities::default()).await;
    assert!(
        result.is_ok(),
        "BEGIN must succeed on a read-only DuckDB connection: {:?}",
        result.err()
    );
    cleanup(&path);
}

#[tokio::test]
async fn parity_query_transaction_control_not_capability_violations() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_not_capability_violation(&config, "COMMIT").await;
    assert_not_capability_violation(&config, "ROLLBACK").await;
    cleanup(&path);
}

// ============================================================================
// Query — denied (CAPABILITY_VIOLATION)
// ============================================================================

/// Assert that `denied_sql` is rejected with `CAPABILITY_VIOLATION` and that
/// a follow-up `verify_sql` returns exactly `expected_rows`, proving the DB
/// was not mutated.
async fn assert_capability_violation_and_state_unchanged(
    config: &ConnectionConfig,
    denied_sql: &str,
    verify_sql: &str,
    expected_rows: usize,
) {
    let caps = Capabilities::default();

    let err_result = DuckDbEngine::execute(config, denied_sql, &[], &caps).await;
    assert!(err_result.is_err(), "expected an error for: {denied_sql}");
    let err = err_result.unwrap_err();
    assert_eq!(
        err.error_code(),
        "CAPABILITY_VIOLATION",
        "'{denied_sql}' must produce CAPABILITY_VIOLATION; got: {}",
        err.error_code()
    );

    // Re-query to prove state is unchanged
    let verify = DuckDbEngine::execute(config, verify_sql, &[], &caps)
        .await
        .unwrap_or_else(|e| panic!("verify SELECT failed after denied write: {e:?}"));
    assert_eq!(
        verify.rows.len(),
        expected_rows,
        "DB state must be unchanged after denied '{denied_sql}': \
         expected {expected_rows} rows from '{verify_sql}'"
    );
}

#[tokio::test]
async fn parity_denied_insert_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "INSERT INTO customers (id, name, email) VALUES (99, 'Hacker', 'h@example.com')",
        "SELECT id FROM customers ORDER BY id",
        3,
    )
    .await;
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_update_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "UPDATE customers SET name = 'Hacker' WHERE id = 1",
        "SELECT name FROM customers WHERE id = 1",
        1,
    )
    .await;
    // Double-check the actual value was not changed
    let qr = DuckDbEngine::execute(
        &config,
        "SELECT name FROM customers WHERE id = 1",
        &[],
        &Capabilities::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        qr.rows[0][0].as_str(),
        Some("Ada Lovelace"),
        "UPDATE must not have mutated the row"
    );
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_delete_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "DELETE FROM customers WHERE id = 1",
        "SELECT id FROM customers ORDER BY id",
        3,
    )
    .await;
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_create_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "CREATE TABLE hacker (id INTEGER PRIMARY KEY)",
        "SELECT table_name FROM duckdb_tables() WHERE table_name = 'hacker'",
        0,
    )
    .await;
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_drop_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "DROP TABLE customers",
        "SELECT id FROM customers ORDER BY id",
        3,
    )
    .await;
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_alter_capability_violation() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    assert_capability_violation_and_state_unchanged(
        &config,
        "ALTER TABLE customers ADD COLUMN phone VARCHAR",
        "SELECT id FROM customers ORDER BY id",
        3,
    )
    .await;
    cleanup(&path);
}

#[tokio::test]
async fn parity_denied_copy_and_attach_capability_violation() {
    // DuckDB-specific write/escape surfaces: COPY writes to the filesystem,
    // ATTACH mutates catalog state, INSTALL/LOAD pull in extensions.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();

    for sql in [
        "COPY customers TO '/tmp/plenum_parity_leak.csv'",
        "ATTACH ':memory:' AS other",
        "DETACH other",
        "INSTALL httpfs",
        "LOAD httpfs",
        "EXPORT DATABASE '/tmp/plenum_parity_leak'",
    ] {
        let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;
        assert!(result.is_err(), "'{sql}' must be rejected");
        assert_eq!(
            result.unwrap_err().error_code(),
            "CAPABILITY_VIOLATION",
            "'{sql}' must produce CAPABILITY_VIOLATION"
        );
    }
    cleanup(&path);
}

// ============================================================================
// Safety — max_rows truncation and timeout_ms
// ============================================================================

#[tokio::test]
async fn parity_safety_max_rows_truncates_bulk_table() {
    // bulk_rows has 1 500 rows; max_rows=100 must truncate and set the flag.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities { max_rows: Some(100), ..Capabilities::default() };
    let qr =
        DuckDbEngine::execute(&config, "SELECT n, label FROM bulk_rows ORDER BY n", &[], &caps)
            .await
            .expect("SELECT bulk_rows failed");
    assert_eq!(qr.rows.len(), 100, "max_rows=100 must limit result to 100 rows");
    assert!(qr.rows_truncated, "rows_truncated must be true when max_rows fires");
    // Verify ordering: first row is n=1
    assert_eq!(qr.rows[0][0], serde_json::json!(1), "first row must be n=1");
    cleanup(&path);
}

#[tokio::test]
async fn parity_safety_rows_returned_matches_max_rows() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities { max_rows: Some(50), ..Capabilities::default() };
    let qr = DuckDbEngine::execute(&config, "SELECT n FROM bulk_rows ORDER BY n", &[], &caps)
        .await
        .expect("SELECT bulk_rows failed");
    assert_eq!(qr.rows.len(), 50, "rows returned must equal max_rows when truncated");
    assert!(qr.rows_truncated, "rows_truncated must be set");
    cleanup(&path);
}

#[tokio::test]
async fn parity_safety_no_truncation_when_under_max_rows() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities { max_rows: Some(5000), ..Capabilities::default() };
    let qr = DuckDbEngine::execute(&config, "SELECT n FROM bulk_rows ORDER BY n", &[], &caps)
        .await
        .expect("SELECT bulk_rows failed");
    assert_eq!(qr.rows.len(), 1500, "all 1500 rows must be returned when max_rows=5000");
    assert!(!qr.rows_truncated, "rows_truncated must be false when not truncated");
    cleanup(&path);
}

#[tokio::test]
async fn parity_safety_offset_pagination() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities { max_rows: Some(10), offset: Some(100), ..Capabilities::default() };
    let qr = DuckDbEngine::execute(&config, "SELECT n FROM bulk_rows ORDER BY n", &[], &caps)
        .await
        .expect("SELECT bulk_rows failed");
    assert_eq!(qr.rows.len(), 10, "offset+max_rows must return one page");
    assert_eq!(qr.rows[0][0], serde_json::json!(101), "first row after offset=100 must be n=101");
    cleanup(&path);
}

#[tokio::test]
async fn parity_safety_timeout_ms_completes_fast_query() {
    // A simple query must finish well within a 5 s window.
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities { timeout_ms: Some(5000), ..Capabilities::default() };
    let result = DuckDbEngine::execute(&config, "SELECT count(*) FROM customers", &[], &caps).await;
    assert!(result.is_ok(), "fast query must complete within 5 s timeout: {:?}", result.err());
    cleanup(&path);
}

#[tokio::test]
async fn parity_safety_timeout_ms_interrupts_long_query() {
    // A large cross-join aggregate takes many seconds. With timeout_ms=50 the
    // interrupt thread fires mid-execution, causing DuckDB to return an
    // INTERRUPT error → QUERY_TIMEOUT. Uses :memory: — no fixture file needed.
    let config = ConnectionConfig::duckdb(":memory:".into());
    let caps = Capabilities { timeout_ms: Some(50), ..Capabilities::default() };
    let sql = "SELECT max(a.range * b.range + a.range) \
               FROM range(200000) a, range(200000) b";
    let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;
    assert!(result.is_err(), "long-running query must be interrupted by timeout");
    assert_eq!(
        result.unwrap_err().error_code(),
        "QUERY_TIMEOUT",
        "interrupted query must surface as QUERY_TIMEOUT"
    );
}

// ============================================================================
// Envelope — JSON shape and determinism
// ============================================================================

#[tokio::test]
async fn parity_envelope_query_result_json_shape() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let qr = DuckDbEngine::execute(
        &config,
        "SELECT id, name FROM customers ORDER BY id LIMIT 1",
        &[],
        &caps,
    )
    .await
    .expect("SELECT customers failed");

    let json = serde_json::to_value(&qr).expect("QueryResult must serialize to JSON");
    assert!(
        json.get("columns").is_some_and(serde_json::Value::is_array),
        "must have 'columns' array"
    );
    assert!(json.get("rows").is_some_and(serde_json::Value::is_array), "must have 'rows' array");
    assert!(json.get("execution_ms").is_some(), "must have 'execution_ms'");
    assert!(json.get("rows_affected").is_none(), "SELECT must not have 'rows_affected'");
    cleanup(&path);
}

#[tokio::test]
async fn parity_envelope_introspect_result_json_shape() {
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let result = DuckDbEngine::introspect(&config, &IntrospectOperation::ListTables, None, None)
        .await
        .expect("ListTables failed");

    let json = serde_json::to_value(&result).expect("IntrospectResult must serialize to JSON");
    assert!(json.get("type").is_some(), "IntrospectResult must have 'type' tag");
    assert!(
        json.get("tables").is_some_and(serde_json::Value::is_array),
        "TableList must have 'tables' array"
    );
    cleanup(&path);
}

#[tokio::test]
async fn parity_envelope_deterministic_excluding_execution_ms() {
    // Identical queries must produce identical row data (execution_ms is timing, excluded).
    let path = build_parity_fixture();
    let config = ConnectionConfig::duckdb(path.clone());
    let caps = Capabilities::default();
    let sql = "SELECT id, name, email FROM customers ORDER BY id";

    let r1 = DuckDbEngine::execute(&config, sql, &[], &caps).await.expect("execute 1");
    let r2 = DuckDbEngine::execute(&config, sql, &[], &caps).await.expect("execute 2");

    assert_eq!(r1.columns, r2.columns, "columns must be identical");
    assert_eq!(r1.rows, r2.rows, "rows must be identical");
    assert_eq!(r1.rows_truncated, r2.rows_truncated, "rows_truncated must be identical");
    // execution_ms is timing and intentionally excluded from the comparison
    cleanup(&path);
}

#[tokio::test]
async fn parity_envelope_error_has_code_and_message() {
    // Errors must expose a non-empty error_code and message.
    let config = ConnectionConfig::duckdb(PathBuf::from("/does/not/exist.duckdb"));
    let err = DuckDbEngine::validate_connection(&config).await.unwrap_err();
    assert!(!err.error_code().is_empty(), "error must have a non-empty error_code");
    assert!(!err.message().is_empty(), "error must have a non-empty message");
}
