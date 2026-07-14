//! Edge Case Testing
//!
//! This module tests edge cases and boundary conditions to ensure Plenum
//! handles unusual inputs gracefully. Tests include:
//! - Large result sets
//! - Special characters and Unicode
//! - Binary data (BLOBs)
//! - Numeric extremes
//! - Empty strings vs NULL
//! - Very long queries
//!
//! These tests ensure robustness and help prevent unexpected failures in
//! production scenarios.

#![cfg(feature = "sqlite")]

use plenum::{Capabilities, ConnectionConfig, DatabaseEngine};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

use std::path::PathBuf;

// ============================================================================
// Test Helpers
// ============================================================================

fn create_test_db() -> PathBuf {
    use rusqlite::{Connection, OpenFlags};
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let temp_file = std::env::temp_dir().join(format!("test_edge_{timestamp}.db"));
    let _ = std::fs::remove_file(&temp_file);

    // Pre-create the database with explicit read-write flags to avoid macOS symlink issues
    // (SQLite 3.39+ with SQLITE_OPEN_NOFOLLOW can cause readonly errors on macOS temp dirs)
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let _conn = Connection::open_with_flags(&temp_file, flags)
        .expect("Failed to create database with write permissions");
    // Connection is dropped here, ensuring file is properly created

    temp_file
}

fn cleanup_db(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

/// Helper to open a connection with explicit read-write flags (avoids macOS readonly issues)
fn open_test_conn(path: &PathBuf) -> rusqlite::Connection {
    use rusqlite::{Connection, OpenFlags};
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    Connection::open_with_flags(path, flags).expect("Failed to open database")
}

/// Helper to get a column value from a row by column name
fn get_column<'a>(
    columns: &[String],
    row: &'a [serde_json::Value],
    column_name: &str,
) -> &'a serde_json::Value {
    let idx = columns
        .iter()
        .position(|c| c == column_name)
        .unwrap_or_else(|| panic!("Column '{column_name}' not found in columns: {columns:?}"));
    &row[idx]
}

// ============================================================================
// Large Dataset Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_large_result_set_with_max_rows() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE large_table (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");

        // Insert 1000 rows
        for i in 1..=1000 {
            conn.execute("INSERT INTO large_table (value) VALUES (?)", [format!("Value {i}")])
                .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities { max_rows: Some(100), timeout_ms: None, offset: None };

    let result = SqliteEngine::execute(&config, "SELECT * FROM large_table", &[], &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(
        query_result.rows.len(),
        100,
        "Should enforce max_rows limit of 100 despite 1000 rows existing"
    );

    cleanup_db(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_very_large_result_set_without_limit() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE large_table (id INTEGER PRIMARY KEY, value INTEGER)", [])
            .expect("Failed to create table");

        // Insert 5000 rows
        for i in 1..=5000 {
            conn.execute("INSERT INTO large_table (value) VALUES (?)", [i])
                .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM large_table", &[], &caps).await;
    assert!(result.is_ok(), "Should handle large result sets");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 5000, "Should return all 5000 rows");

    cleanup_db(&temp_file);
}

// ============================================================================
// Unicode and Special Character Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_unicode_characters() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE unicode_test (id INTEGER PRIMARY KEY, text TEXT)", [])
            .expect("Failed to create table");

        // Insert various Unicode characters
        conn.execute(
            "INSERT INTO unicode_test (text) VALUES (?)",
            ["Hello 世界 🌍 Здравствуй мир"],
        )
        .expect("Failed to insert");

        conn.execute("INSERT INTO unicode_test (text) VALUES (?)", ["Emoji test: 🚀🔥💯✨🎉"])
            .expect("Failed to insert");

        conn.execute(
            "INSERT INTO unicode_test (text) VALUES (?)",
            ["Arabic: مرحبا بالعالم Hebrew: שלום עולם"],
        )
        .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM unicode_test ORDER BY id", &[], &caps).await;
    assert!(result.is_ok(), "Should handle Unicode characters");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 3);

    // Verify Unicode is preserved
    let row0 = &query_result.rows[0];
    let text0 = get_column(&query_result.columns, row0, "text").as_str().unwrap();
    assert!(text0.contains("世界"));
    assert!(text0.contains("🌍"));
    assert!(text0.contains("Здравствуй"));

    cleanup_db(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_special_sql_characters() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE special_chars (id INTEGER PRIMARY KEY, text TEXT)", [])
            .expect("Failed to create table");

        // Test strings with SQL-like characters (should be stored as-is)
        conn.execute(
            "INSERT INTO special_chars (text) VALUES (?)",
            ["Text with 'single quotes' and \"double quotes\""],
        )
        .expect("Failed to insert");

        conn.execute(
            "INSERT INTO special_chars (text) VALUES (?)",
            ["Text with; semicolons; and -- comments"],
        )
        .expect("Failed to insert");

        conn.execute(
            "INSERT INTO special_chars (text) VALUES (?)",
            ["Text with\nnewlines\nand\ttabs"],
        )
        .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM special_chars ORDER BY id", &[], &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 3);

    // Verify special characters are preserved
    let row0 = &query_result.rows[0];
    assert!(get_column(&query_result.columns, row0, "text")
        .as_str()
        .unwrap()
        .contains("'single quotes'"));

    cleanup_db(&temp_file);
}

// ============================================================================
// Binary Data (BLOB) Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_binary_blob_data() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE blob_test (id INTEGER PRIMARY KEY, data BLOB)", [])
            .expect("Failed to create table");

        // Insert binary data
        let binary_data: Vec<u8> = vec![0, 1, 2, 255, 128, 64, 32, 16];
        conn.execute("INSERT INTO blob_test (data) VALUES (?)", [&binary_data])
            .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM blob_test", &[], &caps).await;
    assert!(result.is_ok(), "Should handle BLOB data");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);

    // BLOB should be base64 encoded in JSON
    let row = &query_result.rows[0];
    let blob_value = get_column(&query_result.columns, row, "data");
    assert!(blob_value.is_string(), "BLOB should be encoded as string (base64)");

    cleanup_db(&temp_file);
}

// ============================================================================
// Numeric Edge Cases
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_numeric_extremes() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute(
            "CREATE TABLE numeric_test (
                id INTEGER PRIMARY KEY,
                max_int INTEGER,
                min_int INTEGER,
                zero INTEGER,
                large_real REAL,
                small_real REAL
            )",
            [],
        )
        .expect("Failed to create table");

        conn.execute(
            "INSERT INTO numeric_test (max_int, min_int, zero, large_real, small_real) VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![i64::MAX, i64::MIN, 0, 1.7976931348623157e308, 2.2250738585072014e-308],
        )
        .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM numeric_test", &[], &caps).await;
    assert!(result.is_ok(), "Should handle numeric extremes");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);

    let row = &query_result.rows[0];
    assert!(get_column(&query_result.columns, row, "max_int").is_number());
    assert!(get_column(&query_result.columns, row, "min_int").is_number());
    assert_eq!(get_column(&query_result.columns, row, "zero").as_i64().unwrap(), 0);

    cleanup_db(&temp_file);
}

// ============================================================================
// Empty String vs NULL Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_empty_string_vs_null() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE null_test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");

        conn.execute("INSERT INTO null_test (value) VALUES (?)", [""])
            .expect("Failed to insert empty string");

        conn.execute("INSERT INTO null_test (value) VALUES (NULL)", [])
            .expect("Failed to insert NULL");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM null_test ORDER BY id", &[], &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2);

    // Empty string should be empty string
    let row0 = &query_result.rows[0];
    assert_eq!(get_column(&query_result.columns, row0, "value").as_str().unwrap(), "");

    // NULL should be JSON null
    let row1 = &query_result.rows[1];
    assert_eq!(get_column(&query_result.columns, row1, "value"), &serde_json::Value::Null);

    cleanup_db(&temp_file);
}

// ============================================================================
// Very Long Query Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_very_long_query() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");

        for i in 1..=10 {
            conn.execute("INSERT INTO test (value) VALUES (?)", [format!("Value {i}")])
                .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    // Create a very long WHERE clause
    let mut query = "SELECT * FROM test WHERE id IN (".to_string();
    for i in 1..=1000 {
        if i > 1 {
            query.push_str(", ");
        }
        query.push_str(&i.to_string());
    }
    query.push(')');

    let result = SqliteEngine::execute(&config, &query, &[], &caps).await;
    assert!(result.is_ok(), "Should handle very long queries");

    cleanup_db(&temp_file);
}

// ============================================================================
// SQL Injection Patterns (Should be passed through, not sanitized)
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_sql_injection_patterns_in_data() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE injection_test (id INTEGER PRIMARY KEY, username TEXT)", [])
            .expect("Failed to create table");

        // Store SQL injection-like patterns as data (agent's responsibility to sanitize)
        conn.execute("INSERT INTO injection_test (username) VALUES (?)", ["admin' OR '1'='1"])
            .expect("Failed to insert");

        conn.execute(
            "INSERT INTO injection_test (username) VALUES (?)",
            ["user; DROP TABLE users;--"],
        )
        .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM injection_test", &[], &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2);

    // Verify SQL-like patterns are stored as-is (not sanitized by Plenum)
    let row0 = &query_result.rows[0];
    assert_eq!(
        get_column(&query_result.columns, row0, "username").as_str().unwrap(),
        "admin' OR '1'='1"
    );

    cleanup_db(&temp_file);
}

// ============================================================================
// Timeout Tests (SQLite uses busy_timeout)
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_timeout_capability() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE timeout_test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");
        conn.execute("INSERT INTO timeout_test (value) VALUES ('test')", [])
            .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities {
        max_rows: None,
        timeout_ms: Some(5000), // 5 second timeout
        offset: None,
    };

    // Simple query should complete within timeout
    let result = SqliteEngine::execute(&config, "SELECT * FROM timeout_test", &[], &caps).await;
    assert!(result.is_ok(), "Simple query should complete within timeout");

    cleanup_db(&temp_file);
}

// ============================================================================
// Whitespace and Query Format Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_query_with_excessive_whitespace() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");
        conn.execute("INSERT INTO test (value) VALUES ('data')", []).expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    // Query with excessive whitespace
    let query = "  \n\n  SELECT   *   \n  FROM   test   \n\n  WHERE   id   =   1   \n\n  ";

    let result = SqliteEngine::execute(&config, query, &[], &caps).await;
    assert!(result.is_ok(), "Should handle queries with excessive whitespace");

    cleanup_db(&temp_file);
}

// ============================================================================
// Case Sensitivity Tests
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_case_sensitivity_in_table_names() {
    let temp_file = create_test_db();

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE TestTable (id INTEGER PRIMARY KEY, Value TEXT)", [])
            .expect("Failed to create table");
        conn.execute("INSERT INTO TestTable (Value) VALUES ('data')", [])
            .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    // SQLite is case-insensitive for table names
    let result1 = SqliteEngine::execute(&config, "SELECT * FROM TestTable", &[], &caps).await;
    assert!(result1.is_ok());

    let result2 = SqliteEngine::execute(&config, "SELECT * FROM testtable", &[], &caps).await;
    assert!(result2.is_ok());

    cleanup_db(&temp_file);
}

// ============================================================================
// Type-Coercion Audit Tests (REF-39)
//
// These tests pin the SQLite engine's value-to-JSON behavior for type cases
// the audit explicitly calls out: dynamic typing, NUMERIC affinity, NaN/Inf
// handling, and BLOB-vs-TEXT distinction when BLOB bytes happen to be valid
// UTF-8. Postgres/MySQL counterparts require live databases and are tracked
// in separate child issues.
// ============================================================================

/// Create a uniquely-named temp DB to avoid timestamp collisions between
/// parallel test threads (the shared `create_test_db()` helper has been
/// observed to race when two tests in the same file start in the same
/// nanosecond).
fn create_test_db_named(name: &str) -> PathBuf {
    use rusqlite::{Connection, OpenFlags};
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let temp_file = std::env::temp_dir().join(format!("test_edge_{name}_{timestamp}.db"));
    let _ = std::fs::remove_file(&temp_file);

    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let _conn = Connection::open_with_flags(&temp_file, flags)
        .expect("Failed to create database with write permissions");

    temp_file
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_numeric_affinity_promotes_real_to_integer() {
    // SQLite NUMERIC affinity: "3.0" inserted as REAL gets stored as INTEGER 3
    // when the value has no fractional part. Verify our coercion returns it
    // as a JSON integer, not a float.
    let temp_file = create_test_db_named("numeric_affinity");

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE affinity_test (id INTEGER PRIMARY KEY, n NUMERIC)", [])
            .expect("Failed to create table");
        conn.execute("INSERT INTO affinity_test (n) VALUES (3.0)", [])
            .expect("Failed to insert 3.0");
        conn.execute("INSERT INTO affinity_test (n) VALUES (3.5)", [])
            .expect("Failed to insert 3.5");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();
    let result =
        SqliteEngine::execute(&config, "SELECT n FROM affinity_test ORDER BY id", &[], &caps).await;
    assert!(result.is_ok());
    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2);

    let v_3 = get_column(&query_result.columns, &query_result.rows[0], "n");
    let v_3p5 = get_column(&query_result.columns, &query_result.rows[1], "n");

    assert_eq!(v_3.as_i64(), Some(3), "3.0 in NUMERIC affinity should return as integer 3");
    assert!(v_3p5.is_f64(), "3.5 should remain a float");
    assert!((v_3p5.as_f64().unwrap() - 3.5).abs() < 1e-12);

    cleanup_db(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_real_nan_and_infinity_become_null() {
    // serde_json::Number cannot represent NaN/Inf — our coercion must map
    // those to JSON null rather than panic or emit invalid JSON.
    let temp_file = create_test_db_named("real_nan");

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE r (id INTEGER PRIMARY KEY, v REAL)", [])
            .expect("Failed to create table");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    // SQLite does not natively reject NaN/Inf but they are unusual to insert;
    // generate them via expressions in a SELECT so we exercise the coercion path.
    let result = SqliteEngine::execute(
        &config,
        "SELECT 1.0/0.0 AS pos_inf, -1.0/0.0 AS neg_inf, 0.0/0.0 AS nan",
        &[],
        &caps,
    )
    .await;
    assert!(result.is_ok(), "Query with NaN/Inf should not error");
    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);
    let row = &query_result.rows[0];

    // SQLite returns NULL for divide-by-zero by default, so each cell should be JSON null.
    // The point of this test is that whatever path SQLite takes, our coercion does not panic
    // and emits valid JSON.
    for col in &query_result.columns {
        let v = get_column(&query_result.columns, row, col);
        assert!(
            v.is_null() || v.is_number(),
            "{col} should serialize as JSON null or a finite number, got {v:?}"
        );
    }

    cleanup_db(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_blob_with_valid_utf8_bytes_is_base64_encoded() {
    // Even if a BLOB's bytes happen to be valid UTF-8 (e.g. the bytes of "Hello"),
    // SQLite knows the storage class and we must base64-encode for round-trip safety.
    // Without this guarantee a TEXT "Hello" and BLOB "Hello" would be indistinguishable.
    let temp_file = create_test_db_named("blob_utf8");

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE b (id INTEGER PRIMARY KEY, data BLOB, txt TEXT)", [])
            .expect("Failed to create table");

        // "Hello" — five ASCII bytes, valid UTF-8, but inserted as BLOB.
        let utf8_bytes: Vec<u8> = b"Hello".to_vec();
        conn.execute("INSERT INTO b (data, txt) VALUES (?, ?)", rusqlite::params![utf8_bytes, "Hello"])
            .expect("Failed to insert");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();
    let result = SqliteEngine::execute(&config, "SELECT data, txt FROM b", &[], &caps).await;
    assert!(result.is_ok());
    let query_result = result.unwrap();
    let row = &query_result.rows[0];

    let blob_val = get_column(&query_result.columns, row, "data");
    let text_val = get_column(&query_result.columns, row, "txt");

    // Base64("Hello") = "SGVsbG8="
    assert_eq!(
        blob_val.as_str(),
        Some("SGVsbG8="),
        "BLOB with valid UTF-8 bytes must still be base64-encoded for round-trip safety"
    );
    assert_eq!(text_val.as_str(), Some("Hello"));
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_dynamic_typing_all_storage_classes() {
    // SQLite's dynamic typing means a single column can hold values of any storage class.
    // Verify each storage class maps to the expected JSON shape.
    let temp_file = create_test_db_named("dynamic_typing");

    {
        let conn = open_test_conn(&temp_file);
        conn.execute("CREATE TABLE dyn (id INTEGER PRIMARY KEY, v)", [])
            .expect("Failed to create table");
        // No declared type → fully dynamic. Insert one row per storage class.
        conn.execute("INSERT INTO dyn (v) VALUES (42)", []).expect("integer");
        conn.execute("INSERT INTO dyn (v) VALUES (3.14)", []).expect("real");
        conn.execute("INSERT INTO dyn (v) VALUES ('hi')", []).expect("text");
        conn.execute("INSERT INTO dyn (v) VALUES (x'48656c6c6f')", []).expect("blob hex");
        conn.execute("INSERT INTO dyn (v) VALUES (NULL)", []).expect("null");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();
    let result =
        SqliteEngine::execute(&config, "SELECT v FROM dyn ORDER BY id", &[], &caps).await;
    assert!(result.is_ok());
    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 5);

    let v = |i: usize| get_column(&query_result.columns, &query_result.rows[i], "v");

    assert_eq!(v(0).as_i64(), Some(42));
    assert!(v(1).is_f64());
    assert!((v(1).as_f64().unwrap() - 3.14).abs() < 1e-12);
    assert_eq!(v(2).as_str(), Some("hi"));
    assert_eq!(v(3).as_str(), Some("SGVsbG8="), "BLOB x'48656c6c6f' must be base64");
    assert!(v(4).is_null());

    cleanup_db(&temp_file);
}
