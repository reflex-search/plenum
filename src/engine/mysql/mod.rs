//! MySQL Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for MySQL databases (including MariaDB).
//!
//! # Features
//! - Client-server connections via TCP
//! - Schema introspection via information_schema
//! - Capability-enforced query execution
//! - MySQL and MariaDB version detection
//!
//! # Implementation Notes
//! - Uses `mysql_async` (async driver, requires tokio runtime)
//! - Async operations are wrapped in synchronous interface
//! - Handles MySQL implicit commits for DDL operations
//! - ENUM and SET types converted to strings
//! - JSON type support (MySQL 5.7+)
//! - BLOB data is Base64-encoded for JSON safety
//! - Timeouts enforced via tokio::time::timeout
//! - Row limits enforced in application code
//! - Schema filtering supported (MySQL has explicit schemas/databases)

use mysql_async::{prelude::*, Conn, OptsBuilder, Row, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::capability::validate_query;
use crate::engine::{
    Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine, DatabaseType,
    ForeignKeyInfo, IndexInfo, QueryResult, SchemaInfo, TableInfo,
};
use crate::error::{PlenumError, Result};

/// MySQL database engine implementation
pub struct MySqlEngine;

impl DatabaseEngine for MySqlEngine {
    async fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo> {
        // Validate config is for MySQL
        if config.engine != DatabaseType::MySQL {
            return Err(PlenumError::invalid_input(format!(
                "Expected MySQL engine, got {}",
                config.engine
            )));
        }

        // Build connection options
        let opts = build_mysql_opts(config)?;

        // Connect to MySQL
        let mut conn = Conn::new(opts).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {}", e))
        })?;

        // Get MySQL version
        let version_row: Row = conn
            .query_first("SELECT VERSION()")
            .await
            .map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query MySQL version: {}", e))
            })?
            .ok_or_else(|| PlenumError::connection_failed("No version returned".to_string()))?;

        let version_string: String = version_row.get(0).ok_or_else(|| {
            PlenumError::connection_failed("Failed to extract version string".to_string())
        })?;

        // Detect MySQL vs MariaDB
        let (database_version, server_info) = parse_mysql_version(&version_string);

        // Get current database name
        let db_row: Row = conn
            .query_first("SELECT DATABASE()")
            .await
            .map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query current database: {}", e))
            })?
            .ok_or_else(|| PlenumError::connection_failed("No database returned".to_string()))?;

        let connected_database: String = db_row.get(0).ok_or_else(|| {
            PlenumError::connection_failed("Failed to extract database name".to_string())
        })?;

        // Get current user
        let user_row: Row = conn
            .query_first("SELECT CURRENT_USER()")
            .await
            .map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query current user: {}", e))
            })?
            .ok_or_else(|| PlenumError::connection_failed("No user returned".to_string()))?;

        let user: String = user_row.get(0).ok_or_else(|| {
            PlenumError::connection_failed("Failed to extract user".to_string())
        })?;

        // Close connection
        conn.disconnect().await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to disconnect: {}", e))
        })?;

        Ok(ConnectionInfo {
            database_version,
            server_info,
            connected_database,
            user,
        })
    }

    async fn introspect(config: &ConnectionConfig, schema_filter: Option<&str>) -> Result<SchemaInfo> {
        // Validate config is for MySQL
        if config.engine != DatabaseType::MySQL {
            return Err(PlenumError::invalid_input(format!(
                "Expected MySQL engine, got {}",
                config.engine
            )));
        }

        // Build connection options
        let opts = build_mysql_opts(config)?;

        // Connect to MySQL
        let mut conn = Conn::new(opts).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {}", e))
        })?;

        // Get current database if no schema filter provided
        let target_schema = if let Some(schema) = schema_filter {
            schema.to_string()
        } else {
            // Use current database
            let db_row: Row = conn
                .query_first("SELECT DATABASE()")
                .await
                .map_err(|e| {
                    PlenumError::engine_error("mysql", format!("Failed to query current database: {}", e))
                })?
                .ok_or_else(|| PlenumError::engine_error("mysql", "No database selected".to_string()))?;

            db_row.get(0).ok_or_else(|| {
                PlenumError::engine_error("mysql", "Failed to extract database name".to_string())
            })?
        };

        // Introspect all tables in the schema
        let tables = introspect_all_tables(&mut conn, &target_schema).await?;

        // Close connection
        conn.disconnect().await.map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to disconnect: {}", e))
        })?;

        Ok(SchemaInfo { tables })
    }

    async fn execute(config: &ConnectionConfig, query: &str, caps: &Capabilities) -> Result<QueryResult> {
        // Validate config is for MySQL
        if config.engine != DatabaseType::MySQL {
            return Err(PlenumError::invalid_input(format!(
                "Expected MySQL engine, got {}",
                config.engine
            )));
        }

        // Validate query against capabilities
        validate_query(query, caps, DatabaseType::MySQL)?;

        // Build connection options
        let opts = build_mysql_opts(config)?;

        // Connect to MySQL
        let mut conn = Conn::new(opts).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {}", e))
        })?;

        // Execute with optional timeout
        let start = Instant::now();
        let query_result = if let Some(timeout_ms) = caps.timeout_ms {
            let timeout_duration = Duration::from_millis(timeout_ms);
            tokio::time::timeout(timeout_duration, execute_query(&mut conn, query, caps))
                .await
                .map_err(|_| {
                    PlenumError::query_failed(format!("Query exceeded timeout of {}ms", timeout_ms))
                })??
        } else {
            execute_query(&mut conn, query, caps).await?
        };

        let _elapsed = start.elapsed();

        // Close connection
        conn.disconnect().await.map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to disconnect: {}", e))
        })?;

        Ok(query_result)
    }
}

/// Build MySQL connection options from ConnectionConfig
fn build_mysql_opts(config: &ConnectionConfig) -> Result<OptsBuilder> {
    let host = config
        .host
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'host' parameter"))?;

    let port = config
        .port
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'port' parameter"))?;

    let user = config
        .user
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'user' parameter"))?;

    let password = config
        .password
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'password' parameter"))?;

    let database = config
        .database
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'database' parameter"))?;

    let opts = OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some(user))
        .pass(Some(password))
        .db_name(Some(database));

    Ok(opts)
}

/// Parse MySQL version string to detect MySQL vs MariaDB
fn parse_mysql_version(version_string: &str) -> (String, String) {
    // Example MySQL: "8.0.35"
    // Example MariaDB: "10.11.2-MariaDB"

    if version_string.to_uppercase().contains("MARIADB") {
        // MariaDB
        let version = version_string
            .split('-')
            .next()
            .unwrap_or("unknown")
            .to_string();
        (version.clone(), format!("MariaDB {}", version))
    } else {
        // MySQL
        let version = version_string
            .split_whitespace()
            .next()
            .unwrap_or(version_string)
            .to_string();
        (version.clone(), format!("MySQL {}", version))
    }
}

/// Introspect all tables in the database
async fn introspect_all_tables(conn: &mut Conn, schema: &str) -> Result<Vec<TableInfo>> {
    // Query information_schema.tables for table list
    let query = "SELECT table_name
                 FROM information_schema.tables
                 WHERE table_schema = ?
                 AND table_type = 'BASE TABLE'
                 ORDER BY table_name";

    let rows: Vec<Row> = conn.exec(query, (schema,)).await.map_err(|e| {
        PlenumError::engine_error("mysql", format!("Failed to query tables: {}", e))
    })?;

    let mut tables = Vec::new();
    for row in rows {
        let table_name: String = row.get(0).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract table name".to_string())
        })?;
        tables.push(introspect_table(conn, schema, &table_name).await?);
    }

    Ok(tables)
}

/// Introspect a single table
async fn introspect_table(conn: &mut Conn, schema: &str, table_name: &str) -> Result<TableInfo> {
    // Get columns
    let columns = introspect_columns(conn, schema, table_name).await?;

    // Get primary key
    let primary_key = introspect_primary_key(conn, schema, table_name).await?;

    // Get foreign keys
    let foreign_keys = introspect_foreign_keys(conn, schema, table_name).await?;

    // Get indexes
    let indexes = introspect_indexes(conn, schema, table_name).await?;

    Ok(TableInfo {
        name: table_name.to_string(),
        schema: Some(schema.to_string()),
        columns,
        primary_key,
        foreign_keys,
        indexes,
    })
}

/// Introspect table columns
async fn introspect_columns(conn: &mut Conn, schema: &str, table_name: &str) -> Result<Vec<ColumnInfo>> {
    let query = "SELECT column_name, data_type, is_nullable, column_default
                 FROM information_schema.columns
                 WHERE table_schema = ? AND table_name = ?
                 ORDER BY ordinal_position";

    let rows: Vec<Row> = conn.exec(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query columns for {}.{}: {}", schema, table_name, e),
        )
    })?;

    let mut columns = Vec::new();
    for row in rows {
        let column_name: String = row.get(0).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract column name".to_string())
        })?;
        let data_type: String = row.get(1).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract data type".to_string())
        })?;
        let is_nullable: String = row.get(2).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract nullable status".to_string())
        })?;
        let default: Option<String> = row.get(3);

        columns.push(ColumnInfo {
            name: column_name,
            data_type,
            nullable: is_nullable == "YES",
            default,
        });
    }

    Ok(columns)
}

/// Introspect primary key
async fn introspect_primary_key(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
) -> Result<Option<Vec<String>>> {
    let query = "SELECT column_name
                 FROM information_schema.key_column_usage
                 WHERE table_schema = ?
                 AND table_name = ?
                 AND constraint_name = 'PRIMARY'
                 ORDER BY ordinal_position";

    let rows: Vec<Row> = conn.exec(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query primary key for {}.{}: {}", schema, table_name, e),
        )
    })?;

    if rows.is_empty() {
        return Ok(None);
    }

    let pk_columns: Vec<String> = rows
        .into_iter()
        .filter_map(|row| row.get(0))
        .collect();

    Ok(Some(pk_columns))
}

/// Introspect foreign keys
async fn introspect_foreign_keys(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
) -> Result<Vec<ForeignKeyInfo>> {
    let query = "SELECT
                    kcu.constraint_name,
                    kcu.column_name,
                    kcu.referenced_table_name,
                    kcu.referenced_column_name
                 FROM information_schema.key_column_usage kcu
                 WHERE kcu.table_schema = ?
                 AND kcu.table_name = ?
                 AND kcu.referenced_table_name IS NOT NULL
                 ORDER BY kcu.constraint_name, kcu.ordinal_position";

    let rows: Vec<Row> = conn.exec(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query foreign keys for {}.{}: {}", schema, table_name, e),
        )
    })?;

    // Group by constraint name
    let mut fk_map: HashMap<String, (Vec<String>, String, Vec<String>)> = HashMap::new();

    for row in rows {
        let constraint_name: String = row.get(0).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract constraint name".to_string())
        })?;
        let column_name: String = row.get(1).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract column name".to_string())
        })?;
        let referenced_table: String = row.get(2).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract referenced table".to_string())
        })?;
        let referenced_column: String = row.get(3).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract referenced column".to_string())
        })?;

        fk_map
            .entry(constraint_name.clone())
            .or_insert_with(|| (Vec::new(), referenced_table.clone(), Vec::new()));

        let entry = fk_map.get_mut(&constraint_name).unwrap();
        entry.0.push(column_name);
        entry.2.push(referenced_column);
    }

    let foreign_keys: Vec<ForeignKeyInfo> = fk_map
        .into_iter()
        .map(|(name, (columns, referenced_table, referenced_columns))| ForeignKeyInfo {
            name,
            columns,
            referenced_table,
            referenced_columns,
        })
        .collect();

    Ok(foreign_keys)
}

/// Introspect indexes
async fn introspect_indexes(conn: &mut Conn, schema: &str, table_name: &str) -> Result<Vec<IndexInfo>> {
    let query = "SELECT DISTINCT
                    index_name,
                    non_unique
                 FROM information_schema.statistics
                 WHERE table_schema = ? AND table_name = ?
                 AND index_name != 'PRIMARY'
                 ORDER BY index_name";

    let rows: Vec<Row> = conn.exec(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query indexes for {}.{}: {}", schema, table_name, e),
        )
    })?;

    let mut indexes = Vec::new();
    for row in rows {
        let index_name: String = row.get(0).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract index name".to_string())
        })?;
        let non_unique: i64 = row.get(1).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract non_unique flag".to_string())
        })?;

        // Get columns for this index
        let columns = get_index_columns(conn, schema, table_name, &index_name).await?;

        indexes.push(IndexInfo {
            name: index_name,
            columns,
            unique: non_unique == 0,
        });
    }

    Ok(indexes)
}

/// Get columns for a specific index
async fn get_index_columns(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
    index_name: &str,
) -> Result<Vec<String>> {
    let query = "SELECT column_name
                 FROM information_schema.statistics
                 WHERE table_schema = ? AND table_name = ? AND index_name = ?
                 ORDER BY seq_in_index";

    let rows: Vec<Row> = conn
        .exec(query, (schema, table_name, index_name))
        .await
        .map_err(|e| {
            PlenumError::engine_error(
                "mysql",
                format!(
                    "Failed to query columns for index {}.{}.{}: {}",
                    schema, table_name, index_name, e
                ),
            )
        })?;

    let columns: Vec<String> = rows.into_iter().filter_map(|row| row.get(0)).collect();

    Ok(columns)
}

/// Execute query and return QueryResult
async fn execute_query(conn: &mut Conn, query: &str, caps: &Capabilities) -> Result<QueryResult> {
    // Execute query and determine if it returns rows
    // MySQL async doesn't have a prepare-then-check pattern like tokio-postgres
    // We'll use a heuristic: if query starts with SELECT (after preprocessing), expect rows

    let query_upper = query.trim().to_uppercase();
    let is_select = query_upper.starts_with("SELECT")
        || query_upper.starts_with("SHOW")
        || query_upper.starts_with("DESCRIBE")
        || query_upper.starts_with("DESC")
        || (query_upper.starts_with("WITH") && query_upper.contains("SELECT"));

    if is_select {
        // Query returns rows
        let rows: Vec<Row> = conn.query(query).await.map_err(|e| {
            PlenumError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        // Get column names from first row (if any)
        let column_names: Vec<String> = if let Some(first_row) = rows.first() {
            first_row
                .columns_ref()
                .iter()
                .map(|col| col.name_str().to_string())
                .collect()
        } else {
            // No rows, try to get column names from a LIMIT 0 query
            // For empty result sets, we may not have column info
            Vec::new()
        };

        // Convert rows to JSON
        let mut rows_data = Vec::new();
        for row in rows {
            rows_data.push(row_to_json(&row)?);

            // Enforce max_rows limit
            if let Some(max_rows) = caps.max_rows {
                if rows_data.len() >= max_rows {
                    break;
                }
            }
        }

        Ok(QueryResult {
            columns: column_names,
            rows: rows_data,
            rows_affected: None,
        })
    } else {
        // Query does not return rows (INSERT, UPDATE, DELETE, DDL)
        let result = conn.query_iter(query).await.map_err(|e| {
            PlenumError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        let rows_affected = result.affected_rows();

        // Drop the result to close it
        drop(result);

        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: Some(rows_affected),
        })
    }
}

/// Convert a MySQL row to a JSON-safe HashMap
fn row_to_json(row: &Row) -> Result<HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();

    for (idx, column) in row.columns_ref().iter().enumerate() {
        let col_name = column.name_str().to_string();
        let value = mysql_value_to_json(row, idx)?;
        map.insert(col_name, value);
    }

    Ok(map)
}

/// Convert MySQL value to JSON value
fn mysql_value_to_json(row: &Row, idx: usize) -> Result<serde_json::Value> {
    let value = row.as_ref(idx).ok_or_else(|| {
        PlenumError::query_failed(format!("Failed to get value at index {}", idx))
    })?;

    let json_value = match value {
        Value::NULL => serde_json::Value::Null,

        Value::Bytes(bytes) => {
            // Try to convert to UTF-8 string first
            if let Ok(s) = std::str::from_utf8(bytes) {
                serde_json::Value::String(s.to_string())
            } else {
                // Binary data - encode as Base64
                use base64::Engine;
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                serde_json::Value::String(encoded)
            }
        }

        Value::Int(i) => serde_json::Value::Number((*i).into()),

        Value::UInt(u) => serde_json::json!(*u), // Use json! macro for u64

        Value::Float(f) => serde_json::Number::from_f64(*f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null), // Handle NaN/Infinity as null

        Value::Double(d) => serde_json::Number::from_f64(*d)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null), // Handle NaN/Infinity as null

        Value::Date(year, month, day, hour, minute, second, micro) => {
            // Format as ISO 8601 datetime string
            let datetime_str = format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}",
                year, month, day, hour, minute, second, micro
            );
            serde_json::Value::String(datetime_str)
        }

        Value::Time(is_negative, days, hours, minutes, seconds, microseconds) => {
            // Format as time duration string
            let sign = if *is_negative { "-" } else { "" };
            let total_hours = days * 24 + (*hours as u32);
            let time_str = format!(
                "{}{}:{:02}:{:02}.{:06}",
                sign,
                total_hours,
                minutes,
                seconds,
                microseconds
            );
            serde_json::Value::String(time_str)
        }
    };

    Ok(json_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mysql_version() {
        let (version, info) = parse_mysql_version("8.0.35");
        assert_eq!(version, "8.0.35");
        assert_eq!(info, "MySQL 8.0.35");

        let (version, info) = parse_mysql_version("10.11.2-MariaDB");
        assert_eq!(version, "10.11.2");
        assert_eq!(info, "MariaDB 10.11.2");
    }

    // Note: Integration tests require a running MySQL instance
    // They are marked with #[ignore] and should be run with:
    // cargo test --features mysql -- --ignored

    #[test]
    #[ignore] // Requires running MySQL instance
    fn test_validate_connection() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let result = MySqlEngine::validate_connection(&config);
        assert!(
            result.is_ok(),
            "Connection validation failed: {:?}",
            result.err()
        );

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("MySQL") || info.server_info.contains("MariaDB"));
    }

    #[test]
    fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.engine = DatabaseType::Postgres;

        let result = MySqlEngine::validate_connection(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Expected MySQL engine"));
    }

    #[test]
    fn test_validate_connection_missing_host() {
        let config = ConnectionConfig {
            engine: DatabaseType::MySQL,
            host: None,
            port: Some(3306),
            user: Some("root".to_string()),
            password: Some("password".to_string()),
            database: Some("test".to_string()),
            file: None,
        };

        let result = MySqlEngine::validate_connection(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("MySQL requires 'host' parameter"));
    }

    // Additional integration tests would follow the pattern from postgres/mod.rs
    // Testing introspection, query execution, capability enforcement, etc.
}
