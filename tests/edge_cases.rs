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
    let caps = Capabilities {
        allow_write: false,
        allow_ddl: false,
        max_rows: Some(100),
        timeout_ms: None,
    };

    let result = SqliteEngine::execute(&config, "SELECT * FROM large_table", &caps).await;
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
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM large_table", &caps).await;
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
    let caps = Capabilities::read_only();

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM unicode_test ORDER BY id", &caps).await;
    assert!(result.is_ok(), "Should handle Unicode characters");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 3);

    // Verify Unicode is preserved
    let row0 = &query_result.rows[0];
    let text0 = row0.get("text").unwrap().as_str().unwrap();
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
    let caps = Capabilities::read_only();

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM special_chars ORDER BY id", &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 3);

    // Verify special characters are preserved
    let row0 = &query_result.rows[0];
    assert!(row0.get("text").unwrap().as_str().unwrap().contains("'single quotes'"));

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
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM blob_test", &caps).await;
    assert!(result.is_ok(), "Should handle BLOB data");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);

    // BLOB should be base64 encoded in JSON
    let row = &query_result.rows[0];
    let blob_value = row.get("data").unwrap();
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
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM numeric_test", &caps).await;
    assert!(result.is_ok(), "Should handle numeric extremes");

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);

    let row = &query_result.rows[0];
    assert!(row.get("max_int").unwrap().is_number());
    assert!(row.get("min_int").unwrap().is_number());
    assert_eq!(row.get("zero").unwrap().as_i64().unwrap(), 0);

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
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM null_test ORDER BY id", &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2);

    // Empty string should be empty string
    let row0 = &query_result.rows[0];
    assert_eq!(row0.get("value").unwrap().as_str().unwrap(), "");

    // NULL should be JSON null
    let row1 = &query_result.rows[1];
    assert_eq!(row1.get("value").unwrap(), &serde_json::Value::Null);

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
    let caps = Capabilities::read_only();

    // Create a very long WHERE clause
    let mut query = "SELECT * FROM test WHERE id IN (".to_string();
    for i in 1..=1000 {
        if i > 1 {
            query.push_str(", ");
        }
        query.push_str(&i.to_string());
    }
    query.push(')');

    let result = SqliteEngine::execute(&config, &query, &caps).await;
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
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM injection_test", &caps).await;
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2);

    // Verify SQL-like patterns are stored as-is (not sanitized by Plenum)
    let row0 = &query_result.rows[0];
    assert_eq!(row0.get("username").unwrap().as_str().unwrap(), "admin' OR '1'='1");

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
        allow_write: false,
        allow_ddl: false,
        max_rows: None,
        timeout_ms: Some(5000), // 5 second timeout
    };

    // Simple query should complete within timeout
    let result = SqliteEngine::execute(&config, "SELECT * FROM timeout_test", &caps).await;
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
    let caps = Capabilities::read_only();

    // Query with excessive whitespace
    let query = "  \n\n  SELECT   *   \n  FROM   test   \n\n  WHERE   id   =   1   \n\n  ";

    let result = SqliteEngine::execute(&config, query, &caps).await;
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
    let caps = Capabilities::read_only();

    // SQLite is case-insensitive for table names
    let result1 = SqliteEngine::execute(&config, "SELECT * FROM TestTable", &caps).await;
    assert!(result1.is_ok());

    let result2 = SqliteEngine::execute(&config, "SELECT * FROM testtable", &caps).await;
    assert!(result2.is_ok());

    cleanup_db(&temp_file);
}
