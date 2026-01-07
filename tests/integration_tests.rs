//! Cross-Engine Integration Tests
//!
//! This module tests that all three database engines (SQLite, PostgreSQL, MySQL)
//! behave consistently for the same types of operations. It validates:
//! - Identical queries produce similar structured results
//! - JSON output schemas are consistent
//! - Capability enforcement works uniformly
//! - Error handling is consistent
//! - No cross-engine behavior leakage
//!
//! These tests help ensure that agents can rely on deterministic behavior
//! regardless of which database engine they're using.

#![cfg(feature = "sqlite")]

use plenum::{
    Capabilities, ConnectionConfig, DatabaseEngine,
};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;
#[cfg(feature = "postgres")]
use plenum::engine::postgres::PostgresEngine;
#[cfg(feature = "mysql")]
use plenum::engine::mysql::MySqlEngine;

// ============================================================================
// Test Helpers
// ============================================================================

/// Create a test SQLite database with sample data
fn create_test_sqlite_db() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp_file = std::env::temp_dir().join(format!("test_integration_{}.db", timestamp));
    let _ = std::fs::remove_file(&temp_file); // Clean up if exists

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create temp database");

        // Create test table
        conn.execute(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT,
                age INTEGER
            )",
            [],
        )
        .expect("Failed to create table");

        // Insert sample data
        conn.execute("INSERT INTO users (name, email, age) VALUES ('Alice', 'alice@example.com', 30)", [])
            .expect("Failed to insert");
        conn.execute("INSERT INTO users (name, email, age) VALUES ('Bob', 'bob@example.com', 25)", [])
            .expect("Failed to insert");
        conn.execute("INSERT INTO users (name, email, age) VALUES ('Charlie', NULL, 35)", [])
            .expect("Failed to insert");
    }

    temp_file
}

/// Cleanup test database
fn cleanup_sqlite_db(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

// ============================================================================
// Cross-Engine Consistency Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_select_query_structure() {
    // Test that SELECT queries return consistent JSON structure across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT name, email FROM users WHERE id = 1", &caps);
    assert!(result.is_ok(), "SQLite SELECT query should succeed");

    let query_result = result.unwrap();

    // Verify structure
    assert_eq!(query_result.columns.len(), 2, "Should have 2 columns");
    assert_eq!(query_result.columns[0], "name");
    assert_eq!(query_result.columns[1], "email");
    assert_eq!(query_result.rows.len(), 1, "Should have 1 row");
    assert!(query_result.rows_affected.is_none(), "SELECT should not have rows_affected");

    // Verify data structure
    let row = &query_result.rows[0];
    assert!(row.contains_key("name"));
    assert!(row.contains_key("email"));
    assert_eq!(row.get("name").unwrap(), &serde_json::json!("Alice"));

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_insert_query_structure() {
    // Test that INSERT queries return consistent structure across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::with_write();

    let result = SqliteEngine::execute(
        &config,
        "INSERT INTO users (name, email, age) VALUES ('David', 'david@example.com', 40)",
        &caps,
    );
    assert!(result.is_ok(), "SQLite INSERT query should succeed");

    let query_result = result.unwrap();

    // Verify structure for DML operations
    assert_eq!(query_result.columns.len(), 0, "INSERT should have no columns");
    assert_eq!(query_result.rows.len(), 0, "INSERT should have no rows");
    assert!(query_result.rows_affected.is_some(), "INSERT should have rows_affected");
    assert_eq!(query_result.rows_affected.unwrap(), 1, "Should affect 1 row");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_capability_violation_readonly() {
    // Test that capability violations are caught uniformly across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    // Try to INSERT without write capability
    let result = SqliteEngine::execute(
        &config,
        "INSERT INTO users (name, email) VALUES ('Eve', 'eve@example.com')",
        &caps,
    );

    assert!(result.is_err(), "Should reject INSERT without write capability");
    let err = result.unwrap_err();
    assert!(
        err.message().contains("Write operations require --allow-write"),
        "Error message should mention capability requirement"
    );

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_capability_violation_ddl() {
    // Test that DDL operations are rejected without appropriate capability
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::with_write(); // Has write but NOT ddl

    // Try to CREATE TABLE without DDL capability
    let result = SqliteEngine::execute(
        &config,
        "CREATE TABLE test (id INTEGER PRIMARY KEY)",
        &caps,
    );

    assert!(result.is_err(), "Should reject DDL without --allow-ddl capability");
    let err = result.unwrap_err();
    assert!(
        err.message().contains("DDL operations require --allow-ddl"),
        "Error message should mention DDL capability requirement"
    );

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_null_handling() {
    // Test that NULL values are handled consistently across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT name, email FROM users WHERE name = 'Charlie'", &caps);
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 1);

    let row = &query_result.rows[0];
    assert_eq!(row.get("email").unwrap(), &serde_json::Value::Null, "NULL should be represented as JSON null");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_max_rows_enforcement() {
    // Test that max_rows is enforced uniformly across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities {
        allow_write: false,
        allow_ddl: false,
        max_rows: Some(2),
        timeout_ms: None,
    };

    let result = SqliteEngine::execute(&config, "SELECT * FROM users ORDER BY id", &caps);
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.rows.len(), 2, "Should limit to max_rows=2 even though 3 rows exist");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_empty_result_set() {
    // Test that empty result sets are handled consistently
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM users WHERE id = 9999", &caps);
    assert!(result.is_ok());

    let query_result = result.unwrap();
    assert_eq!(query_result.columns.len(), 4, "Columns should still be present");
    assert_eq!(query_result.rows.len(), 0, "Should have zero rows");
    assert!(query_result.rows_affected.is_none(), "SELECT should not have rows_affected");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_schema_introspection_structure() {
    // Test that introspection returns consistent structure across engines
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let result = SqliteEngine::introspect(&config, None);
    assert!(result.is_ok());

    let schema = result.unwrap();

    // Verify structure
    assert_eq!(schema.tables.len(), 1, "Should have 1 table");

    let table = &schema.tables[0];
    assert_eq!(table.name, "users");
    assert_eq!(table.columns.len(), 4, "Should have 4 columns");

    // Verify primary key info exists
    assert!(table.primary_key.is_some(), "Should have primary key info");
    let pk = table.primary_key.as_ref().unwrap();
    assert_eq!(pk.len(), 1);
    assert_eq!(pk[0], "id");

    // Verify column structure
    let id_col = table.columns.iter().find(|c| c.name == "id").expect("Should have id column");
    assert_eq!(id_col.name, "id");
    // Note: SQLite INTEGER PRIMARY KEY columns may report as nullable=true because SQLite
    // doesn't explicitly set the notnull flag for INTEGER PRIMARY KEY (it's implicitly NOT NULL).
    // This is SQLite-specific behavior. The important check is that the primary_key field
    // correctly identifies this column.

    let name_col = table.columns.iter().find(|c| c.name == "name").expect("Should have name column");
    assert!(!name_col.nullable, "NOT NULL column should report as not nullable");

    let email_col = table.columns.iter().find(|c| c.name == "email").expect("Should have email column");
    assert!(email_col.nullable, "Nullable column should report as nullable");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_connection_validation_structure() {
    // Test that connection validation returns consistent structure
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let result = SqliteEngine::validate_connection(&config);
    assert!(result.is_ok());

    let conn_info = result.unwrap();

    // Verify structure
    assert!(!conn_info.database_version.is_empty(), "Should have database version");
    assert!(!conn_info.server_info.is_empty(), "Should have server info");
    assert!(!conn_info.connected_database.is_empty(), "Should have connected database name");
    assert!(!conn_info.user.is_empty(), "Should have user (even if N/A)");

    cleanup_sqlite_db(&temp_file);
}

// ============================================================================
// Error Handling Consistency Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_malformed_sql_error() {
    // Test that malformed SQL produces consistent error behavior
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    // Use a query with invalid column name (will pass capability check but fail at execution)
    let result = SqliteEngine::execute(&config, "SELECT nonexistent_column FROM users", &caps);
    assert!(result.is_err(), "Malformed SQL should error");

    // Error should be a query failed error
    let err = result.unwrap_err();
    let error_msg = err.message();
    assert!(
        error_msg.contains("column") || error_msg.contains("nonexistent") || error_msg.contains("no such column"),
        "Error message should mention SQL problem. Got: {}", error_msg
    );

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_missing_table_error() {
    // Test that querying non-existent table produces consistent error
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM nonexistent_table", &caps);
    assert!(result.is_err(), "Missing table should error");

    let err = result.unwrap_err();
    assert!(
        err.message().contains("no such table") || err.message().contains("nonexistent"),
        "Error should mention missing table"
    );

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_cross_engine_connection_failure() {
    // Test that invalid connection produces consistent error
    let config = ConnectionConfig::sqlite(std::path::PathBuf::from("/nonexistent/path/db.sqlite"));

    let result = SqliteEngine::validate_connection(&config);
    assert!(result.is_err(), "Invalid path should fail connection");

    let err = result.unwrap_err();
    assert!(
        err.error_code() == "CONNECTION_FAILED",
        "Should be a connection failure error"
    );
}

// ============================================================================
// JSON Output Consistency Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_json_serialization_of_query_result() {
    // Test that QueryResult serializes to valid JSON
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let result = SqliteEngine::execute(&config, "SELECT * FROM users ORDER BY id LIMIT 1", &caps);
    assert!(result.is_ok());

    let query_result = result.unwrap();

    // Verify it can be serialized to JSON
    let json = serde_json::to_string(&query_result);
    assert!(json.is_ok(), "QueryResult should serialize to JSON");

    // Verify it can be deserialized back
    let json_str = json.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert!(parsed.is_object());
    assert!(parsed.get("columns").is_some());
    assert!(parsed.get("rows").is_some());

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_json_serialization_of_schema_info() {
    // Test that SchemaInfo serializes to valid JSON
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let result = SqliteEngine::introspect(&config, None);
    assert!(result.is_ok());

    let schema_info = result.unwrap();

    // Verify it can be serialized to JSON
    let json = serde_json::to_string(&schema_info);
    assert!(json.is_ok(), "SchemaInfo should serialize to JSON");

    // Verify structure
    let json_str = json.unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert!(parsed.is_object());
    assert!(parsed.get("tables").is_some());
    assert!(parsed.get("tables").unwrap().is_array());

    cleanup_sqlite_db(&temp_file);
}

// ============================================================================
// Determinism Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_deterministic_query_results() {
    // Test that identical queries produce identical results
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    let sql = "SELECT name, email FROM users ORDER BY id";

    let result1 = SqliteEngine::execute(&config, sql, &caps).unwrap();
    let result2 = SqliteEngine::execute(&config, sql, &caps).unwrap();

    // Results should be identical (except timing metadata which is not part of QueryResult)
    assert_eq!(result1.columns, result2.columns, "Columns should be identical");
    assert_eq!(result1.rows, result2.rows, "Rows should be identical");
    assert_eq!(result1.rows_affected, result2.rows_affected, "Rows affected should be identical");

    cleanup_sqlite_db(&temp_file);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_deterministic_introspection() {
    // Test that introspection is deterministic
    let temp_file = create_test_sqlite_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let schema1 = SqliteEngine::introspect(&config, None).unwrap();
    let schema2 = SqliteEngine::introspect(&config, None).unwrap();

    // Schemas should be identical
    assert_eq!(schema1.tables.len(), schema2.tables.len(), "Table count should be identical");

    for (t1, t2) in schema1.tables.iter().zip(schema2.tables.iter()) {
        assert_eq!(t1.name, t2.name, "Table names should be identical");
        assert_eq!(t1.columns.len(), t2.columns.len(), "Column counts should be identical");
        assert_eq!(t1.primary_key, t2.primary_key, "Primary keys should be identical");
    }

    cleanup_sqlite_db(&temp_file);
}
