//! Output Validation Tests
//!
//! This module validates that all Plenum output conforms to the defined JSON schemas.
//! It ensures:
//! - All stdout is valid JSON
//! - Success envelopes match the expected schema
//! - Error envelopes match the expected schema
//! - No logs or non-JSON output appears on stdout
//! - Metadata is consistent across commands
//!
//! Uses `insta` for snapshot testing to detect unintended output changes.

#![cfg(feature = "sqlite")]

use plenum::{
    Capabilities, ConnectionConfig, DatabaseEngine, ErrorEnvelope, Metadata, SuccessEnvelope,
};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

use std::path::PathBuf;

// ============================================================================
// Success Envelope Structure Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_success_envelope_structure() {
    // Create a simple success envelope and validate its JSON structure
    let data = serde_json::json!({"test": "value"});
    let envelope: SuccessEnvelope<serde_json::Value> =
        SuccessEnvelope::new("sqlite", "test", data, Metadata::new(42));

    // Serialize to JSON
    let json_str = serde_json::to_string(&envelope).expect("Should serialize");
    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Should deserialize");

    // Verify required fields
    assert!(json_value.is_object(), "Should be JSON object");
    assert_eq!(json_value["ok"], true, "ok should be true");
    assert_eq!(json_value["engine"], "sqlite", "engine should be sqlite");
    assert_eq!(json_value["command"], "test", "command should be test");
    assert!(json_value["data"].is_object(), "data should be object");
    assert!(json_value["meta"].is_object(), "meta should be object");

    // Verify metadata structure
    assert_eq!(json_value["meta"]["execution_ms"], 42, "execution_ms should be 42");

    // Verify no extra fields (should match schema exactly)
    let top_level_keys: Vec<&str> =
        json_value.as_object().unwrap().keys().map(std::string::String::as_str).collect();
    assert_eq!(top_level_keys.len(), 5, "Should have exactly 5 top-level fields");
    assert!(top_level_keys.contains(&"ok"));
    assert!(top_level_keys.contains(&"engine"));
    assert!(top_level_keys.contains(&"command"));
    assert!(top_level_keys.contains(&"data"));
    assert!(top_level_keys.contains(&"meta"));
}

#[test]
#[cfg(feature = "sqlite")]
fn test_error_envelope_structure() {
    // Create a simple error envelope and validate its JSON structure
    let envelope = ErrorEnvelope::new(
        "sqlite",
        "test",
        plenum::ErrorInfo::new("TEST_ERROR", "Test error message"),
    );

    // Serialize to JSON
    let json_str = serde_json::to_string(&envelope).expect("Should serialize");
    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Should deserialize");

    // Verify required fields
    assert!(json_value.is_object(), "Should be JSON object");
    assert_eq!(json_value["ok"], false, "ok should be false");
    assert_eq!(json_value["engine"], "sqlite", "engine should be sqlite");
    assert_eq!(json_value["command"], "test", "command should be test");
    assert!(json_value["error"].is_object(), "error should be object");

    // Verify error structure
    assert_eq!(json_value["error"]["code"], "TEST_ERROR");
    assert_eq!(json_value["error"]["message"], "Test error message");

    // Verify no extra fields
    let top_level_keys: Vec<&str> =
        json_value.as_object().unwrap().keys().map(std::string::String::as_str).collect();
    assert_eq!(top_level_keys.len(), 4, "Should have exactly 4 top-level fields");
    assert!(top_level_keys.contains(&"ok"));
    assert!(top_level_keys.contains(&"engine"));
    assert!(top_level_keys.contains(&"command"));
    assert!(top_level_keys.contains(&"error"));

    let error_keys: Vec<&str> =
        json_value["error"].as_object().unwrap().keys().map(std::string::String::as_str).collect();
    assert_eq!(error_keys.len(), 2, "Should have exactly 2 error fields");
    assert!(error_keys.contains(&"code"));
    assert!(error_keys.contains(&"message"));
}

// ============================================================================
// JSON Serialization Tests (No Logs/Text Pollution)
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_query_result_serializes_to_pure_json() {
    // Verify that QueryResult serializes to pure JSON with no extra content
    use plenum::engine::QueryResult;

    let row = vec![serde_json::json!(1), serde_json::json!("Alice")];

    let result = QueryResult {
        columns: vec!["id".to_string(), "name".to_string()],
        rows: vec![row],
        rows_affected: None,
        execution_ms: 0,
        rows_truncated: false,
    };

    let json_str = serde_json::to_string(&result).expect("Should serialize");

    // Verify it's pure JSON (no logs, no text, just JSON)
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Should be valid JSON");
    assert!(parsed.is_object());

    // Verify no extra whitespace or content
    assert!(!json_str.contains('\n'), "Should not contain newlines");
    assert!(
        !json_str.starts_with("INFO:") && !json_str.starts_with("ERROR:"),
        "Should not contain log prefixes"
    );
}

#[test]
#[cfg(feature = "sqlite")]
fn test_schema_info_serializes_to_pure_json() {
    // Verify that SchemaInfo serializes to pure JSON with no extra content
    use plenum::engine::SchemaInfo;

    let schema = SchemaInfo { tables: vec![] };

    let json_str = serde_json::to_string(&schema).expect("Should serialize");

    // Verify it's pure JSON
    let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Should be valid JSON");
    assert!(parsed.is_object());
    assert!(parsed["tables"].is_array());

    // Verify no extra content
    assert!(!json_str.contains('\n'), "Should not contain newlines");
}

// ============================================================================
// Metadata Consistency Tests
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_metadata_includes_execution_time() {
    let meta = Metadata::new(123);

    let json_str = serde_json::to_string(&meta).expect("Should serialize");
    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Should deserialize");

    assert_eq!(json_value["execution_ms"], 123);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_metadata_includes_rows_returned_for_queries() {
    let meta = Metadata::with_rows(456, 10);

    let json_str = serde_json::to_string(&meta).expect("Should serialize");
    let json_value: serde_json::Value =
        serde_json::from_str(&json_str).expect("Should deserialize");

    assert_eq!(json_value["execution_ms"], 456);
    assert_eq!(json_value["rows_returned"], 10);
}

// ============================================================================
// Error Code Consistency Tests
// ============================================================================

#[test]
fn test_all_error_codes_are_consistent() {
    use plenum::PlenumError;

    // Verify all error codes match the schema's enum
    let valid_codes = [
        "CAPABILITY_VIOLATION",
        "CONNECTION_FAILED",
        "QUERY_FAILED",
        "INVALID_INPUT",
        "ENGINE_ERROR",
        "CONFIG_ERROR",
    ];

    // Test each error type
    assert!(valid_codes.contains(&PlenumError::capability_violation("test").error_code()));
    assert!(valid_codes.contains(&PlenumError::connection_failed("test").error_code()));
    assert!(valid_codes.contains(&PlenumError::query_failed("test").error_code()));
    assert!(valid_codes.contains(&PlenumError::invalid_input("test").error_code()));
    assert!(valid_codes.contains(&PlenumError::engine_error("test", "test").error_code()));
    assert!(valid_codes.contains(&PlenumError::config_error("test").error_code()));
}

// ============================================================================
// Snapshot Tests (using insta)
// ============================================================================

#[test]
#[cfg(feature = "sqlite")]
fn test_success_envelope_snapshot() {
    // Snapshot test for success envelope JSON structure
    let data = serde_json::json!({
        "result": "success",
        "value": 42
    });

    let envelope: SuccessEnvelope<serde_json::Value> =
        SuccessEnvelope::new("sqlite", "test", data, Metadata::with_rows(100, 5));

    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
#[cfg(feature = "sqlite")]
fn test_error_envelope_snapshot() {
    // Snapshot test for error envelope JSON structure
    let envelope = ErrorEnvelope::new(
        "postgres",
        "query",
        plenum::ErrorInfo::new("QUERY_FAILED", "Column 'invalid' does not exist"),
    );

    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

// ----------------------------------------------------------------------------
// Per-variant error envelope snapshots
//
// Locks down the JSON contract emitted for each `PlenumError` variant. If a
// variant's `error_code()` or `Display` representation changes, the snapshot
// breaks and forces an explicit `cargo insta review` decision rather than
// silently shifting the agent-facing wire format.
// ----------------------------------------------------------------------------

#[test]
fn test_error_envelope_snapshot_capability_violation() {
    let err = plenum::PlenumError::capability_violation("DDL operations are not permitted");
    let envelope = ErrorEnvelope::from_error("postgres", "query", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
fn test_error_envelope_snapshot_connection_failed() {
    let err = plenum::PlenumError::connection_failed("could not connect to server");
    let envelope = ErrorEnvelope::from_error("postgres", "connect", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
fn test_error_envelope_snapshot_query_failed() {
    let err = plenum::PlenumError::query_failed("relation \"users\" does not exist");
    let envelope = ErrorEnvelope::from_error("postgres", "query", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
fn test_error_envelope_snapshot_invalid_input() {
    let err = plenum::PlenumError::invalid_input("Query cannot be empty");
    let envelope = ErrorEnvelope::from_error("", "query", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
fn test_error_envelope_snapshot_engine_error() {
    let err = plenum::PlenumError::engine_error("mysql", "lost connection to server");
    let envelope = ErrorEnvelope::from_error("mysql", "query", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

#[test]
fn test_error_envelope_snapshot_config_error() {
    let err = plenum::PlenumError::config_error("connection 'prod' not found");
    let envelope = ErrorEnvelope::from_error("", "connect", &err);
    let json_str = serde_json::to_string_pretty(&envelope).expect("Should serialize");
    insta::assert_snapshot!(json_str);
}

// Note: Removed query_result_snapshot test due to non-deterministic HashMap ordering
// QueryResult structure is validated by:
// - test_json_serialization_of_query_result
// - test_cross_engine_select_query_structure (in integration_tests.rs)
// - test_real_query_output_is_valid_json

// ============================================================================
// Real-World Output Tests
// ============================================================================

/// Helper to create a test `SQLite` database
fn create_test_db() -> PathBuf {
    use rusqlite::{Connection, OpenFlags};
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let temp_file = std::env::temp_dir().join(format!("test_output_{timestamp}.db"));
    let _ = std::fs::remove_file(&temp_file);

    {
        // Use explicit read-write flags to avoid macOS symlink readonly issues
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
        let conn =
            Connection::open_with_flags(&temp_file, flags).expect("Failed to create temp database");

        conn.execute(
            "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL)",
            [],
        )
        .expect("Failed to create table");

        conn.execute("INSERT INTO products (name, price) VALUES ('Widget', 9.99)", [])
            .expect("Failed to insert");
        conn.execute("INSERT INTO products (name, price) VALUES ('Gadget', 19.99)", [])
            .expect("Failed to insert");
    }

    temp_file
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_real_query_output_is_valid_json() {
    let temp_file = create_test_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result = SqliteEngine::execute(&config, "SELECT * FROM products ORDER BY id", &[], &caps).await;
    assert!(result.is_ok());

    // Wrap in success envelope (like the CLI does)
    let envelope =
        SuccessEnvelope::new("sqlite", "query", result.unwrap(), Metadata::with_rows(42, 2));

    // Verify it serializes to valid JSON
    let json_str = serde_json::to_string(&envelope).expect("Should serialize");
    let _parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Should be valid JSON");

    // Verify no logs or extra content
    assert!(!json_str.contains("INFO:") && !json_str.contains("ERROR:"), "Should not contain logs");
    assert!(!json_str.contains("DEBUG:"), "Should not contain debug output");

    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_real_introspect_output_is_valid_json() {
    use plenum::engine::IntrospectOperation;

    let temp_file = create_test_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let result = SqliteEngine::introspect(&config, &IntrospectOperation::ListTables, None, None).await;
    assert!(result.is_ok());

    // Wrap in success envelope
    let envelope = SuccessEnvelope::new("sqlite", "introspect", result.unwrap(), Metadata::new(50));

    // Verify it serializes to valid JSON
    let json_str = serde_json::to_string(&envelope).expect("Should serialize");
    let _parsed: serde_json::Value = serde_json::from_str(&json_str).expect("Should be valid JSON");

    // Verify structure matches schema
    let json_value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(json_value["ok"], true);
    assert_eq!(json_value["engine"], "sqlite");
    assert_eq!(json_value["command"], "introspect");
    assert!(json_value["data"]["tables"].is_array());

    let _ = std::fs::remove_file(&temp_file);
}

// ============================================================================
// Truncation Signalling + Pagination Tests (REF-262)
// ============================================================================

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_truncated_result_signals_rows_truncated_true() {
    // DB has 2 products; max_rows=1 means result is truncated
    let temp_file = create_test_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities { max_rows: Some(1), timeout_ms: None, offset: None };

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM products ORDER BY id", &[], &caps).await;
    assert!(result.is_ok(), "Query should succeed");

    let query_result = result.unwrap();
    assert!(query_result.rows_truncated, "rows_truncated must be true when max_rows caps result");
    assert_eq!(query_result.rows.len(), 1, "Should return exactly max_rows rows");

    let meta = Metadata::with_query(0, query_result.rows.len(), query_result.rows_truncated, 0);
    let json = serde_json::to_string(&meta).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["rows_truncated"], true);
    assert_eq!(v["has_more"], true);
    assert_eq!(v["next_offset"], 1);

    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_uncapped_result_signals_rows_truncated_false() {
    let temp_file = create_test_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::default();

    let result =
        SqliteEngine::execute(&config, "SELECT * FROM products ORDER BY id", &[], &caps).await;
    assert!(result.is_ok(), "Query should succeed");

    let query_result = result.unwrap();
    assert!(!query_result.rows_truncated, "rows_truncated must be false when result is not capped");

    let meta =
        Metadata::with_query(0, query_result.rows.len(), query_result.rows_truncated, 0);
    let json = serde_json::to_string(&meta).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["rows_truncated"], false);
    assert_eq!(v["has_more"], false);
    assert!(v.get("next_offset").is_none() || v["next_offset"].is_null());

    let _ = std::fs::remove_file(&temp_file);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_pagination_with_offset_returns_disjoint_pages() {
    // DB has 2 products; page size 1 → page 1 has row 1, page 2 has row 2
    let temp_file = create_test_db();
    let config = ConnectionConfig::sqlite(temp_file.clone());

    let caps_p1 = Capabilities { max_rows: Some(1), timeout_ms: None, offset: None };
    let r1 = SqliteEngine::execute(
        &config,
        "SELECT id FROM products ORDER BY id",
        &[],
        &caps_p1,
    )
    .await
    .unwrap();
    assert!(r1.rows_truncated, "page 1 should be truncated (2 rows total, only 1 returned)");
    assert_eq!(r1.rows.len(), 1);

    let caps_p2 = Capabilities { max_rows: Some(1), timeout_ms: None, offset: Some(1) };
    let r2 = SqliteEngine::execute(
        &config,
        "SELECT id FROM products ORDER BY id",
        &[],
        &caps_p2,
    )
    .await
    .unwrap();
    assert!(!r2.rows_truncated, "page 2 should not be truncated (last page)");
    assert_eq!(r2.rows.len(), 1);
    assert_ne!(r1.rows[0][0], r2.rows[0][0], "pages must return different rows");

    let _ = std::fs::remove_file(&temp_file);
}

#[test]
fn test_metadata_with_query_truncated_next_offset() {
    let meta = Metadata::with_query(42, 5, true, 10);
    let json = serde_json::to_string(&meta).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["rows_truncated"], true);
    assert_eq!(v["has_more"], true);
    assert_eq!(v["next_offset"], 15);
    assert_eq!(v["rows_returned"], 5);
    assert_eq!(v["execution_ms"], 42);
}

#[test]
fn test_metadata_with_query_not_truncated_omits_next_offset() {
    let meta = Metadata::with_query(10, 3, false, 0);
    let json = serde_json::to_string(&meta).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["rows_truncated"], false);
    assert_eq!(v["has_more"], false);
    assert!(v.get("next_offset").is_none() || v["next_offset"].is_null());
    assert_eq!(v["rows_returned"], 3);
}

#[test]
fn test_non_query_metadata_omits_truncation_fields() {
    let meta = Metadata::new(50);
    let json = serde_json::to_string(&meta).unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(v.get("rows_truncated").is_none() || v["rows_truncated"].is_null());
    assert!(v.get("has_more").is_none() || v["has_more"].is_null());
    assert!(v.get("next_offset").is_none() || v["next_offset"].is_null());
    assert!(v.get("rows_returned").is_none() || v["rows_returned"].is_null());
}
