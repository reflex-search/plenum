//! `SQLite` Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for `SQLite` databases.
//!
//! # Features
//! - File-based connections (`/path/to/db.sqlite`)
//! - In-memory connections (`:memory:`)
//! - Schema introspection via `SQLite` system tables and PRAGMAs
//! - Capability-enforced query execution
//!
//! # Implementation Notes
//! - Uses `rusqlite` (synchronous driver, no async needed)
//! - BLOB data is Base64-encoded for JSON safety
//! - Timeouts enforced via `busy_timeout`
//! - Row limits enforced in application code
//! - No explicit schema support (`SQLite` uses catalogs)

use rusqlite::{Connection, OpenFlags, Row};
use std::collections::HashMap; // Used for grouping foreign keys during introspection
use std::time::Instant;

use crate::capability::validate_query;
use crate::engine::{
    Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine, DatabaseType,
    ForeignKeyInfo, IndexInfo, QueryResult, SchemaInfo, TableInfo,
};
use crate::error::{PlenumError, Result};

/// `SQLite` database engine implementation
pub struct SqliteEngine;

impl DatabaseEngine for SqliteEngine {
    async fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo> {
        // Validate config is for SQLite
        if config.engine != DatabaseType::SQLite {
            return Err(PlenumError::invalid_input(format!(
                "Expected SQLite engine, got {}",
                config.engine
            )));
        }

        // Extract file path
        let file_path = config
            .file
            .as_ref()
            .ok_or_else(|| PlenumError::invalid_input("SQLite requires 'file' parameter"))?;

        // Open connection (read-only for validation)
        let path_str = file_path.to_str().ok_or_else(|| {
            PlenumError::invalid_input("SQLite file path contains invalid UTF-8 characters")
        })?;
        let conn = open_connection(path_str, true)?;

        // Get SQLite version
        let version: String =
            conn.query_row("SELECT sqlite_version()", [], |row| row.get(0)).map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query SQLite version: {e}"))
            })?;

        // Get database file name for connected_database
        let db_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_else(|| file_path.to_str().unwrap_or("unknown"))
            .to_string();

        Ok(ConnectionInfo {
            database_version: version.clone(),
            server_info: format!("SQLite {version}"),
            connected_database: db_name,
            user: "N/A".to_string(), // SQLite has no user concept
        })
    }

    async fn introspect(
        config: &ConnectionConfig,
        schema_filter: Option<&str>,
    ) -> Result<SchemaInfo> {
        // Validate config is for SQLite
        if config.engine != DatabaseType::SQLite {
            return Err(PlenumError::invalid_input(format!(
                "Expected SQLite engine, got {}",
                config.engine
            )));
        }

        // Extract file path
        let file_path = config
            .file
            .as_ref()
            .ok_or_else(|| PlenumError::invalid_input("SQLite requires 'file' parameter"))?;

        // Open connection (read-only)
        let path_str = file_path.to_str().ok_or_else(|| {
            PlenumError::invalid_input("SQLite file path contains invalid UTF-8 characters")
        })?;
        let conn = open_connection(path_str, true)?;

        // Note: SQLite doesn't have explicit schemas in the same way as PostgreSQL/MySQL
        // The schema_filter parameter is ignored for SQLite
        if schema_filter.is_some() {
            // Silently ignore - SQLite doesn't have schema filtering
        }

        // Query sqlite_master for all tables (exclude internal tables)
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table'
                 AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .map_err(|e| {
                PlenumError::engine_error("sqlite", format!("Failed to query tables: {e}"))
            })?;

        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| {
                PlenumError::engine_error("sqlite", format!("Failed to fetch table names: {e}"))
            })?
            .collect::<std::result::Result<Vec<String>, _>>()
            .map_err(|e| {
                PlenumError::engine_error("sqlite", format!("Failed to collect table names: {e}"))
            })?;

        // Introspect each table
        let mut tables = Vec::new();
        for table_name in table_names {
            tables.push(introspect_table(&conn, &table_name)?);
        }

        Ok(SchemaInfo { tables })
    }

    async fn execute(
        config: &ConnectionConfig,
        query: &str,
        caps: &Capabilities,
    ) -> Result<QueryResult> {
        // Validate config is for SQLite
        if config.engine != DatabaseType::SQLite {
            return Err(PlenumError::invalid_input(format!(
                "Expected SQLite engine, got {}",
                config.engine
            )));
        }

        // Validate query against capabilities
        validate_query(query, caps, DatabaseType::SQLite)?;

        // Extract file path
        let file_path = config
            .file
            .as_ref()
            .ok_or_else(|| PlenumError::invalid_input("SQLite requires 'file' parameter"))?;

        // Open connection (read-write for queries)
        let path_str = file_path.to_str().ok_or_else(|| {
            PlenumError::invalid_input("SQLite file path contains invalid UTF-8 characters")
        })?;
        let conn = open_connection(path_str, false)?;

        // Set busy timeout if specified
        if let Some(timeout_ms) = caps.timeout_ms {
            conn.busy_timeout(std::time::Duration::from_millis(timeout_ms)).map_err(|e| {
                PlenumError::engine_error("sqlite", format!("Failed to set timeout: {e}"))
            })?;
        }

        // Execute query
        let start = Instant::now();
        let result = execute_query(&conn, query, caps)?;
        let _elapsed = start.elapsed();

        Ok(result)
    }
}

/// Open `SQLite` connection with appropriate flags
fn open_connection(path: &str, read_only: bool) -> Result<Connection> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    };

    Connection::open_with_flags(path, flags)
        .map_err(|e| PlenumError::connection_failed(format!("Failed to open SQLite database: {e}")))
}

/// Introspect a single table and return `TableInfo`
fn introspect_table(conn: &Connection, table_name: &str) -> Result<TableInfo> {
    // Get column information via PRAGMA table_info
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table_name})")).map_err(|e| {
        PlenumError::engine_error(
            "sqlite",
            format!("Failed to prepare table_info for {table_name}: {e}"),
        )
    })?;

    let columns: Vec<ColumnInfo> = stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                name: row.get::<_, String>(1)?,
                data_type: row.get::<_, String>(2)?,
                nullable: row.get::<_, i32>(3)? == 0, // notnull column: 0 = nullable, 1 = not null
                default: row.get::<_, Option<String>>(4)?,
            })
        })
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to query columns for {table_name}: {e}"),
            )
        })?
        .collect::<std::result::Result<Vec<ColumnInfo>, _>>()
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to collect columns for {table_name}: {e}"),
            )
        })?;

    // Detect primary key columns
    let mut pk_stmt = conn.prepare(&format!("PRAGMA table_info({table_name})")).map_err(|e| {
        PlenumError::engine_error(
            "sqlite",
            format!("Failed to prepare pk query for {table_name}: {e}"),
        )
    })?;

    let primary_key_columns: Vec<String> = pk_stmt
        .query_map([], |row| {
            let pk: i32 = row.get(5)?; // pk column: >0 means part of primary key
            let name: String = row.get(1)?;
            Ok((pk, name))
        })
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to query primary keys for {table_name}: {e}"),
            )
        })?
        .filter_map(std::result::Result::ok)
        .filter(|(pk, _)| *pk > 0)
        .map(|(_, name)| name)
        .collect();

    let primary_key = if primary_key_columns.is_empty() { None } else { Some(primary_key_columns) };

    // Get foreign keys via PRAGMA foreign_key_list
    let mut fk_stmt =
        conn.prepare(&format!("PRAGMA foreign_key_list({table_name})")).map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to prepare foreign_key_list for {table_name}: {e}"),
            )
        })?;

    // Group foreign keys by constraint id
    let mut fk_map: HashMap<i32, (String, Vec<String>, Vec<String>)> = HashMap::new();

    fk_stmt
        .query_map([], |row| {
            let id: i32 = row.get(0)?; // Foreign key id
            let table: String = row.get(2)?; // Referenced table
            let from_col: String = row.get(3)?; // Column in this table
            let to_col: String = row.get(4)?; // Column in referenced table
            Ok((id, table, from_col, to_col))
        })
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to query foreign keys for {table_name}: {e}"),
            )
        })?
        .for_each(|r| {
            if let Ok((id, ref_table, from_col, to_col)) = r {
                fk_map.entry(id).or_insert_with(|| (ref_table.clone(), Vec::new(), Vec::new()));
                fk_map.get_mut(&id).unwrap().1.push(from_col);
                fk_map.get_mut(&id).unwrap().2.push(to_col);
            }
        });

    let foreign_keys: Vec<ForeignKeyInfo> = fk_map
        .into_iter()
        .map(|(id, (ref_table, from_cols, to_cols))| ForeignKeyInfo {
            name: format!("fk_{table_name}_{id}"),
            columns: from_cols,
            referenced_table: ref_table,
            referenced_columns: to_cols,
        })
        .collect();

    // Get indexes via PRAGMA index_list
    let mut idx_stmt = conn.prepare(&format!("PRAGMA index_list({table_name})")).map_err(|e| {
        PlenumError::engine_error(
            "sqlite",
            format!("Failed to prepare index_list for {table_name}: {e}"),
        )
    })?;

    let index_list: Vec<(String, bool)> = idx_stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            let unique: i32 = row.get(2)?;
            Ok((name, unique != 0))
        })
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to query indexes for {table_name}: {e}"),
            )
        })?
        .filter_map(std::result::Result::ok)
        .collect();

    let mut indexes = Vec::new();
    for (index_name, unique) in index_list {
        // Skip auto-created indexes for primary keys (SQLite creates these automatically)
        if index_name.starts_with("sqlite_autoindex_") {
            continue;
        }

        // Get columns in this index via PRAGMA index_info
        let mut idx_info_stmt =
            conn.prepare(&format!("PRAGMA index_info({index_name})")).map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to prepare index_info for {index_name}: {e}"),
                )
            })?;

        let index_columns: Vec<String> = idx_info_stmt
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to query index columns for {index_name}: {e}"),
                )
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        indexes.push(IndexInfo { name: index_name, columns: index_columns, unique });
    }

    Ok(TableInfo {
        name: table_name.to_string(),
        schema: None, // SQLite doesn't have explicit schemas
        columns,
        primary_key,
        foreign_keys,
        indexes,
    })
}

/// Execute query and return `QueryResult`
fn execute_query(conn: &Connection, query: &str, caps: &Capabilities) -> Result<QueryResult> {
    // Prepare statement
    let mut stmt = conn
        .prepare(query)
        .map_err(|e| PlenumError::query_failed(format!("Failed to prepare query: {e}")))?;

    // Get column names
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| (*s).to_string()).collect();

    // Execute query and collect rows
    let mut rows_data = Vec::new();
    let mut rows_affected: Option<u64> = None;

    // Check if this is a SELECT query (has columns)
    if column_names.is_empty() {
        // Non-SELECT query (INSERT, UPDATE, DELETE, DDL)
        stmt.execute([])
            .map_err(|e| PlenumError::query_failed(format!("Failed to execute query: {e}")))?;

        // Get rows affected (only for DML statements)
        rows_affected = Some(conn.changes());
    } else {
        // SELECT query - collect result set
        let rows = stmt
            .query([])
            .map_err(|e| PlenumError::query_failed(format!("Failed to execute query: {e}")))?;

        let mapped_rows = rows.mapped(|row| row_to_json(&column_names, row)).collect::<Vec<_>>();

        for row_result in mapped_rows {
            let row = row_result
                .map_err(|e| PlenumError::query_failed(format!("Failed to fetch row: {e}")))?;
            rows_data.push(row);

            // Enforce max_rows limit
            if let Some(max_rows) = caps.max_rows {
                if rows_data.len() >= max_rows {
                    break;
                }
            }
        }
    }

    Ok(QueryResult { columns: column_names, rows: rows_data, rows_affected })
}

/// Convert a `SQLite` row to a JSON-safe `Vec`
fn row_to_json(
    column_names: &[String],
    row: &Row,
) -> std::result::Result<Vec<serde_json::Value>, rusqlite::Error> {
    let mut values = Vec::with_capacity(column_names.len());

    for idx in 0..column_names.len() {
        let value = sqlite_value_to_json(row, idx)?;
        values.push(value);
    }

    Ok(values)
}

/// Convert `SQLite` value to JSON value
fn sqlite_value_to_json(
    row: &Row,
    idx: usize,
) -> std::result::Result<serde_json::Value, rusqlite::Error> {
    use rusqlite::types::ValueRef;

    let value_ref = row.get_ref(idx)?;

    Ok(match value_ref {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::Number(i.into()),
        ValueRef::Real(f) => serde_json::Number::from_f64(f)
            .map_or(serde_json::Value::Null, serde_json::Value::Number), // Handle NaN/Infinity as null
        ValueRef::Text(s) => {
            let text = std::str::from_utf8(s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            serde_json::Value::String(text.to_string())
        }
        ValueRef::Blob(b) => {
            // Encode BLOB as Base64 for JSON safety
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(b);
            serde_json::Value::String(encoded)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DatabaseType;

    #[tokio::test]
    async fn test_validate_connection_memory() {
        let config = ConnectionConfig::sqlite(":memory:".into());
        let result = SqliteEngine::validate_connection(&config).await;
        assert!(result.is_ok());

        let info = result.unwrap();
        assert!(info.database_version.starts_with("3.")); // SQLite version 3.x
        assert!(info.server_info.contains("SQLite"));
        assert_eq!(info.connected_database, ":memory:");
        assert_eq!(info.user, "N/A");
    }

    #[tokio::test]
    async fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::sqlite(":memory:".into());
        config.engine = DatabaseType::Postgres;

        let result = SqliteEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Expected SQLite engine"));
    }

    #[tokio::test]
    async fn test_validate_connection_missing_file() {
        let config = ConnectionConfig {
            engine: DatabaseType::SQLite,
            file: None,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
        };

        let result = SqliteEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("SQLite requires 'file' parameter"));
    }

    #[tokio::test]
    async fn test_introspect_schema() {
        // Create a temporary database file
        let temp_file = std::env::temp_dir().join("test_introspect.db");
        let _ = std::fs::remove_file(&temp_file); // Clean up if exists

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    email TEXT
                )",
                [],
            )
            .expect("Failed to create table");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let result = SqliteEngine::introspect(&config, None).await;
        assert!(result.is_ok());

        let schema = result.unwrap();
        assert_eq!(schema.tables.len(), 1);

        let table = &schema.tables[0];
        assert_eq!(table.name, "users");
        assert_eq!(table.columns.len(), 3);

        // Check primary key
        assert!(table.primary_key.is_some());
        let pk = table.primary_key.as_ref().unwrap();
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0], "id");

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_select_query() {
        // Create temp database
        let temp_file = std::env::temp_dir().join("test_execute_select.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create table");
            conn.execute("INSERT INTO users (name) VALUES ('Alice')", [])
                .expect("Failed to insert");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result = SqliteEngine::execute(&config, "SELECT * FROM users", &caps).await;
        assert!(result.is_ok());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns.len(), 2);
        assert_eq!(query_result.rows.len(), 1);
        assert_eq!(query_result.rows_affected, None);

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_insert_rejected() {
        let temp_file = std::env::temp_dir().join("test_execute_insert_rejected.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create table");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result =
            SqliteEngine::execute(&config, "INSERT INTO users (name) VALUES ('Bob')", &caps).await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message().to_string();
        assert!(error_message.contains("Plenum is read-only"));
        assert!(error_message.contains("Please run this query manually"));

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_update_rejected() {
        let temp_file = std::env::temp_dir().join("test_execute_update_rejected.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create table");
            conn.execute("INSERT INTO users (name) VALUES ('Alice')", [])
                .expect("Failed to insert");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result =
            SqliteEngine::execute(&config, "UPDATE users SET name = 'Bob' WHERE id = 1", &caps)
                .await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message().to_string();
        assert!(error_message.contains("Plenum is read-only"));
        assert!(error_message.contains("Please run this query manually"));

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_ddl_rejected() {
        let temp_file = std::env::temp_dir().join("test_execute_ddl_rejected.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let _ = Connection::open(&temp_file).expect("Failed to create temp database");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result =
            SqliteEngine::execute(&config, "CREATE TABLE users (id INTEGER PRIMARY KEY)", &caps)
                .await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message().to_string();
        assert!(error_message.contains("Plenum is read-only"));
        assert!(error_message.contains("Please run this query manually"));

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_delete_rejected() {
        let temp_file = std::env::temp_dir().join("test_execute_delete_rejected.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create table");
            conn.execute("INSERT INTO users (name) VALUES ('Alice')", [])
                .expect("Failed to insert");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result = SqliteEngine::execute(&config, "DELETE FROM users WHERE id = 1", &caps).await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message().to_string();
        assert!(error_message.contains("Plenum is read-only"));
        assert!(error_message.contains("Please run this query manually"));

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_max_rows_limit() {
        let temp_file = std::env::temp_dir().join("test_max_rows.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create table");

            for i in 1..=10 {
                conn.execute("INSERT INTO users (name) VALUES (?)", [format!("User {i}")])
                    .expect("Failed to insert");
            }
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities { max_rows: Some(5), ..Capabilities::default() };
        let result = SqliteEngine::execute(&config, "SELECT * FROM users", &caps).await;

        assert!(result.is_ok());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows.len(), 5); // Limited to 5 rows

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_all_data_types() {
        let temp_file = std::env::temp_dir().join("test_data_types.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute(
                "CREATE TABLE test_types (
                    int_col INTEGER,
                    real_col REAL,
                    text_col TEXT,
                    blob_col BLOB,
                    null_col TEXT
                )",
                [],
            )
            .expect("Failed to create table");

            conn.execute(
                "INSERT INTO test_types VALUES (?, ?, ?, ?, ?)",
                rusqlite::params![
                    42,
                    std::f64::consts::PI,
                    "hello",
                    vec![1u8, 2u8, 3u8],
                    Option::<String>::None
                ],
            )
            .expect("Failed to insert");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let caps = Capabilities::default();
        let result = SqliteEngine::execute(&config, "SELECT * FROM test_types", &caps).await;

        assert!(result.is_ok());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows.len(), 1);

        let row = &query_result.rows[0];

        // Check INTEGER (index 0)
        assert_eq!(row[0], serde_json::json!(42));

        // Check REAL (index 1)
        let real_val = &row[1];
        assert!(real_val.is_number());

        // Check TEXT (index 2)
        assert_eq!(row[2], serde_json::json!("hello"));

        // Check BLOB (index 3, should be base64 encoded)
        let blob_val = &row[3];
        assert!(blob_val.is_string());

        // Check NULL (index 4)
        assert_eq!(row[4], serde_json::Value::Null);

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_foreign_keys() {
        let temp_file = std::env::temp_dir().join("test_fk.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("PRAGMA foreign_keys = ON", []).expect("Failed to enable FKs");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                .expect("Failed to create users table");
            conn.execute(
                "CREATE TABLE posts (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER,
                    FOREIGN KEY (user_id) REFERENCES users(id)
                )",
                [],
            )
            .expect("Failed to create posts table");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let result = SqliteEngine::introspect(&config, None).await;
        assert!(result.is_ok());

        let schema = result.unwrap();
        let posts_table =
            schema.tables.iter().find(|t| t.name == "posts").expect("posts table not found");

        assert!(!posts_table.foreign_keys.is_empty());
        let fk = &posts_table.foreign_keys[0];
        assert_eq!(fk.referenced_table, "users");
        assert_eq!(fk.columns, vec!["user_id"]);

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_indexes() {
        let temp_file = std::env::temp_dir().join("test_indexes.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT)", [])
                .expect("Failed to create table");
            conn.execute("CREATE INDEX idx_email ON users(email)", [])
                .expect("Failed to create index");
            conn.execute("CREATE UNIQUE INDEX idx_email_unique ON users(email)", [])
                .expect("Failed to create unique index");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let result = SqliteEngine::introspect(&config, None).await;
        assert!(result.is_ok());

        let schema = result.unwrap();
        let users_table =
            schema.tables.iter().find(|t| t.name == "users").expect("users table not found");

        // Should have at least 2 indexes (excluding auto-created primary key index)
        assert!(users_table.indexes.len() >= 2);

        // Check for unique index
        let unique_idx = users_table
            .indexes
            .iter()
            .find(|i| i.name == "idx_email_unique")
            .expect("unique index not found");
        assert!(unique_idx.unique);

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }
}
