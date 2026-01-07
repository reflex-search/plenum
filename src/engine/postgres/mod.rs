//! PostgreSQL Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for PostgreSQL databases.
//!
//! # Features
//! - Client-server connections via TCP
//! - Schema introspection via information_schema
//! - Capability-enforced query execution
//! - Rich type system support (arrays, JSON/JSONB, timestamps, etc.)
//!
//! # Implementation Notes
//! - Uses `tokio-postgres` (async driver, requires tokio runtime)
//! - Async operations are wrapped in synchronous interface
//! - Arrays converted to JSON arrays
//! - JSON/JSONB preserved as nested JSON
//! - BYTEA data is Base64-encoded for JSON safety
//! - Timeouts enforced via tokio::time::timeout
//! - Row limits enforced in application code
//! - Schema filtering supported (PostgreSQL has explicit schemas)

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio_postgres::{Client, Config, NoTls, Row};

use crate::capability::validate_query;
use crate::engine::{
    Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine, DatabaseType,
    ForeignKeyInfo, IndexInfo, QueryResult, SchemaInfo, TableInfo,
};
use crate::error::{PlenumError, Result};

/// PostgreSQL database engine implementation
pub struct PostgresEngine;

impl DatabaseEngine for PostgresEngine {
    fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo> {
        // Validate config is for PostgreSQL
        if config.engine != DatabaseType::Postgres {
            return Err(PlenumError::invalid_input(format!(
                "Expected PostgreSQL engine, got {}",
                config.engine
            )));
        }

        // Build connection config
        let pg_config = build_pg_config(config)?;

        // Connect (async operation wrapped in blocking call)
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PlenumError::connection_failed(format!("Failed to create tokio runtime: {}", e)))?;

        let conn_info = runtime.block_on(async {
            // Connect to PostgreSQL
            let (client, connection) = pg_config.connect(NoTls).await.map_err(|e| {
                PlenumError::connection_failed(format!("Failed to connect to PostgreSQL: {}", e))
            })?;

            // Spawn connection handler
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("PostgreSQL connection error: {}", e);
                }
            });

            // Get PostgreSQL version
            let version_row = client
                .query_one("SELECT version()", &[])
                .await
                .map_err(|e| {
                    PlenumError::connection_failed(format!("Failed to query PostgreSQL version: {}", e))
                })?;

            let version_string: String = version_row.get(0);

            // Extract version number (e.g., "PostgreSQL 15.3 on x86_64..." -> "15.3")
            let database_version = version_string
                .split_whitespace()
                .nth(1)
                .unwrap_or("unknown")
                .to_string();

            // Get current database name
            let db_row = client
                .query_one("SELECT current_database()", &[])
                .await
                .map_err(|e| {
                    PlenumError::connection_failed(format!("Failed to query current database: {}", e))
                })?;

            let connected_database: String = db_row.get(0);

            // Get current user
            let user_row = client
                .query_one("SELECT current_user", &[])
                .await
                .map_err(|e| {
                    PlenumError::connection_failed(format!("Failed to query current user: {}", e))
                })?;

            let user: String = user_row.get(0);

            Ok(ConnectionInfo {
                database_version: database_version.clone(),
                server_info: version_string,
                connected_database,
                user,
            })
        })?;

        Ok(conn_info)
    }

    fn introspect(config: &ConnectionConfig, schema_filter: Option<&str>) -> Result<SchemaInfo> {
        // Validate config is for PostgreSQL
        if config.engine != DatabaseType::Postgres {
            return Err(PlenumError::invalid_input(format!(
                "Expected PostgreSQL engine, got {}",
                config.engine
            )));
        }

        // Build connection config
        let pg_config = build_pg_config(config)?;

        // Connect and introspect (async operation wrapped in blocking call)
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PlenumError::engine_error("postgres", format!("Failed to create tokio runtime: {}", e)))?;

        let schema_info = runtime.block_on(async {
            // Connect to PostgreSQL
            let (client, connection) = pg_config.connect(NoTls).await.map_err(|e| {
                PlenumError::connection_failed(format!("Failed to connect to PostgreSQL: {}", e))
            })?;

            // Spawn connection handler
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("PostgreSQL connection error: {}", e);
                }
            });

            // Introspect all tables
            let tables = introspect_all_tables(&client, schema_filter).await?;

            Ok(SchemaInfo { tables })
        })?;

        Ok(schema_info)
    }

    fn execute(config: &ConnectionConfig, query: &str, caps: &Capabilities) -> Result<QueryResult> {
        // Validate config is for PostgreSQL
        if config.engine != DatabaseType::Postgres {
            return Err(PlenumError::invalid_input(format!(
                "Expected PostgreSQL engine, got {}",
                config.engine
            )));
        }

        // Validate query against capabilities
        validate_query(query, caps, DatabaseType::Postgres)?;

        // Build connection config
        let pg_config = build_pg_config(config)?;

        // Execute query (async operation wrapped in blocking call)
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| PlenumError::engine_error("postgres", format!("Failed to create tokio runtime: {}", e)))?;

        let result = runtime.block_on(async {
            // Connect to PostgreSQL
            let (client, connection) = pg_config.connect(NoTls).await.map_err(|e| {
                PlenumError::connection_failed(format!("Failed to connect to PostgreSQL: {}", e))
            })?;

            // Spawn connection handler
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("PostgreSQL connection error: {}", e);
                }
            });

            // Execute with optional timeout
            let start = Instant::now();
            let query_result = if let Some(timeout_ms) = caps.timeout_ms {
                let timeout_duration = Duration::from_millis(timeout_ms);
                tokio::time::timeout(timeout_duration, execute_query(&client, query, caps))
                    .await
                    .map_err(|_| {
                        PlenumError::query_failed(format!("Query exceeded timeout of {}ms", timeout_ms))
                    })??
            } else {
                execute_query(&client, query, caps).await?
            };

            let _elapsed = start.elapsed();

            Ok(query_result)
        })?;

        Ok(result)
    }
}

/// Build PostgreSQL connection config from ConnectionConfig
fn build_pg_config(config: &ConnectionConfig) -> Result<Config> {
    let host = config
        .host
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("PostgreSQL requires 'host' parameter"))?;

    let port = config
        .port
        .ok_or_else(|| PlenumError::invalid_input("PostgreSQL requires 'port' parameter"))?;

    let user = config
        .user
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("PostgreSQL requires 'user' parameter"))?;

    let password = config
        .password
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("PostgreSQL requires 'password' parameter"))?;

    let database = config
        .database
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("PostgreSQL requires 'database' parameter"))?;

    let mut pg_config = Config::new();
    pg_config
        .host(host)
        .port(port)
        .user(user)
        .password(password)
        .dbname(database);

    Ok(pg_config)
}

/// Introspect all tables in the database
async fn introspect_all_tables(
    client: &Client,
    schema_filter: Option<&str>,
) -> Result<Vec<TableInfo>> {
    // Query information_schema.tables
    // Exclude system schemas (pg_catalog, information_schema, pg_toast)
    let query = if let Some(schema) = schema_filter {
        format!(
            "SELECT table_schema, table_name
             FROM information_schema.tables
             WHERE table_schema = '{}'
             AND table_type = 'BASE TABLE'
             ORDER BY table_schema, table_name",
            schema
        )
    } else {
        "SELECT table_schema, table_name
         FROM information_schema.tables
         WHERE table_schema NOT IN ('pg_catalog', 'information_schema', 'pg_toast')
         AND table_type = 'BASE TABLE'
         ORDER BY table_schema, table_name"
            .to_string()
    };

    let rows = client.query(&query, &[]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to query tables: {}", e))
    })?;

    let mut tables = Vec::new();
    for row in rows {
        let schema: String = row.get(0);
        let table_name: String = row.get(1);
        tables.push(introspect_table(client, &schema, &table_name).await?);
    }

    Ok(tables)
}

/// Introspect a single table
async fn introspect_table(client: &Client, schema: &str, table_name: &str) -> Result<TableInfo> {
    // Get columns
    let columns = introspect_columns(client, schema, table_name).await?;

    // Get primary key
    let primary_key = introspect_primary_key(client, schema, table_name).await?;

    // Get foreign keys
    let foreign_keys = introspect_foreign_keys(client, schema, table_name).await?;

    // Get indexes
    let indexes = introspect_indexes(client, schema, table_name).await?;

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
async fn introspect_columns(client: &Client, schema: &str, table_name: &str) -> Result<Vec<ColumnInfo>> {
    let query = "
        SELECT column_name, data_type, is_nullable, column_default
        FROM information_schema.columns
        WHERE table_schema = $1 AND table_name = $2
        ORDER BY ordinal_position";

    let rows = client
        .query(query, &[&schema, &table_name])
        .await
        .map_err(|e| {
            PlenumError::engine_error(
                "postgres",
                format!("Failed to query columns for {}.{}: {}", schema, table_name, e),
            )
        })?;

    let mut columns = Vec::new();
    for row in rows {
        let column_name: String = row.get(0);
        let data_type: String = row.get(1);
        let is_nullable: String = row.get(2);
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
    client: &Client,
    schema: &str,
    table_name: &str,
) -> Result<Option<Vec<String>>> {
    let query = "
        SELECT kcu.column_name
        FROM information_schema.table_constraints tc
        JOIN information_schema.key_column_usage kcu
          ON tc.constraint_name = kcu.constraint_name
          AND tc.table_schema = kcu.table_schema
        WHERE tc.constraint_type = 'PRIMARY KEY'
          AND tc.table_schema = $1
          AND tc.table_name = $2
        ORDER BY kcu.ordinal_position";

    let rows = client
        .query(query, &[&schema, &table_name])
        .await
        .map_err(|e| {
            PlenumError::engine_error(
                "postgres",
                format!("Failed to query primary key for {}.{}: {}", schema, table_name, e),
            )
        })?;

    if rows.is_empty() {
        return Ok(None);
    }

    let pk_columns: Vec<String> = rows.iter().map(|row| row.get(0)).collect();
    Ok(Some(pk_columns))
}

/// Introspect foreign keys
async fn introspect_foreign_keys(
    client: &Client,
    schema: &str,
    table_name: &str,
) -> Result<Vec<ForeignKeyInfo>> {
    let query = "
        SELECT
            tc.constraint_name,
            kcu.column_name,
            ccu.table_name AS foreign_table_name,
            ccu.column_name AS foreign_column_name
        FROM information_schema.table_constraints AS tc
        JOIN information_schema.key_column_usage AS kcu
          ON tc.constraint_name = kcu.constraint_name
          AND tc.table_schema = kcu.table_schema
        JOIN information_schema.constraint_column_usage AS ccu
          ON ccu.constraint_name = tc.constraint_name
          AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND tc.table_schema = $1
          AND tc.table_name = $2
        ORDER BY tc.constraint_name, kcu.ordinal_position";

    let rows = client
        .query(query, &[&schema, &table_name])
        .await
        .map_err(|e| {
            PlenumError::engine_error(
                "postgres",
                format!("Failed to query foreign keys for {}.{}: {}", schema, table_name, e),
            )
        })?;

    // Group by constraint name
    let mut fk_map: HashMap<String, (Vec<String>, String, Vec<String>)> = HashMap::new();

    for row in rows {
        let constraint_name: String = row.get(0);
        let column_name: String = row.get(1);
        let foreign_table: String = row.get(2);
        let foreign_column: String = row.get(3);

        fk_map
            .entry(constraint_name.clone())
            .or_insert_with(|| (Vec::new(), foreign_table.clone(), Vec::new()));

        let entry = fk_map.get_mut(&constraint_name).unwrap();
        entry.0.push(column_name);
        entry.2.push(foreign_column);
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
async fn introspect_indexes(client: &Client, schema: &str, table_name: &str) -> Result<Vec<IndexInfo>> {
    // Query pg_indexes for index information
    let query = "
        SELECT
            indexname,
            indexdef
        FROM pg_indexes
        WHERE schemaname = $1 AND tablename = $2
        ORDER BY indexname";

    let rows = client
        .query(query, &[&schema, &table_name])
        .await
        .map_err(|e| {
            PlenumError::engine_error(
                "postgres",
                format!("Failed to query indexes for {}.{}: {}", schema, table_name, e),
            )
        })?;

    let mut indexes = Vec::new();
    for row in rows {
        let index_name: String = row.get(0);
        let index_def: String = row.get(1);

        // Skip primary key indexes (they're already captured in primary_key field)
        if index_name.ends_with("_pkey") {
            continue;
        }

        // Determine if index is unique
        let unique = index_def.contains("UNIQUE INDEX");

        // Extract column names from index definition
        // Example: "CREATE INDEX idx_users_email ON public.users USING btree (email)"
        let columns = extract_index_columns(&index_def);

        indexes.push(IndexInfo {
            name: index_name,
            columns,
            unique,
        });
    }

    Ok(indexes)
}

/// Extract column names from PostgreSQL index definition
fn extract_index_columns(index_def: &str) -> Vec<String> {
    // Find the column list between parentheses
    if let Some(start) = index_def.rfind('(') {
        if let Some(end) = index_def.rfind(')') {
            let column_str = &index_def[start + 1..end];
            return column_str
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
        }
    }
    Vec::new()
}

/// Execute query and return QueryResult
async fn execute_query(client: &Client, query: &str, caps: &Capabilities) -> Result<QueryResult> {
    // Execute query
    let stmt = client.prepare(query).await.map_err(|e| {
        PlenumError::query_failed(format!("Failed to prepare query: {}", e))
    })?;

    // Check if this is a SELECT query (returns rows)
    let is_select = !stmt.columns().is_empty();

    if is_select {
        // SELECT query - execute and collect rows
        let rows = client.query(&stmt, &[]).await.map_err(|e| {
            PlenumError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        // Get column names
        let column_names: Vec<String> = stmt.columns().iter().map(|c| c.name().to_string()).collect();

        // Convert rows to JSON
        let mut rows_data = Vec::new();
        for row in rows {
            rows_data.push(row_to_json(&column_names, &row)?);

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
        // Non-SELECT query (INSERT, UPDATE, DELETE, DDL)
        let rows_affected = client.execute(&stmt, &[]).await.map_err(|e| {
            PlenumError::query_failed(format!("Failed to execute query: {}", e))
        })?;

        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: Some(rows_affected),
        })
    }
}

/// Convert a PostgreSQL row to a JSON-safe HashMap
fn row_to_json(column_names: &[String], row: &Row) -> Result<HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();

    for (idx, col_name) in column_names.iter().enumerate() {
        let value = postgres_value_to_json(row, idx)?;
        map.insert(col_name.clone(), value);
    }

    Ok(map)
}

/// Convert PostgreSQL value to JSON value
fn postgres_value_to_json(row: &Row, idx: usize) -> Result<serde_json::Value> {
    use tokio_postgres::types::Type;

    let column = &row.columns()[idx];
    let col_type = column.type_();

    // Handle NULL first
    if let Ok(None) = row.try_get::<_, Option<String>>(idx) {
        return Ok(serde_json::Value::Null);
    }

    // Map PostgreSQL types to JSON
    let value = match *col_type {
        // Boolean
        Type::BOOL => {
            let v: bool = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get boolean value: {}", e))
            })?;
            serde_json::Value::Bool(v)
        }

        // Integers
        Type::INT2 => {
            let v: i16 = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get i16 value: {}", e))
            })?;
            serde_json::Value::Number(v.into())
        }
        Type::INT4 => {
            let v: i32 = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get i32 value: {}", e))
            })?;
            serde_json::Value::Number(v.into())
        }
        Type::INT8 => {
            let v: i64 = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get i64 value: {}", e))
            })?;
            serde_json::Value::Number(v.into())
        }

        // Floats
        Type::FLOAT4 => {
            let v: f32 = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get f32 value: {}", e))
            })?;
            serde_json::Number::from_f64(v as f64)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null) // Handle NaN/Infinity as null
        }
        Type::FLOAT8 => {
            let v: f64 = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get f64 value: {}", e))
            })?;
            serde_json::Number::from_f64(v)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null) // Handle NaN/Infinity as null
        }

        // Text types (VARCHAR, TEXT, CHAR, etc.)
        Type::VARCHAR | Type::TEXT | Type::BPCHAR | Type::NAME => {
            let v: String = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get string value: {}", e))
            })?;
            serde_json::Value::String(v)
        }

        // JSON types
        Type::JSON | Type::JSONB => {
            let v: serde_json::Value = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get JSON value: {}", e))
            })?;
            v
        }

        // BYTEA (binary data) - encode as Base64
        Type::BYTEA => {
            let v: Vec<u8> = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get bytea value: {}", e))
            })?;
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&v);
            serde_json::Value::String(encoded)
        }

        // Timestamps - convert to ISO 8601 strings
        Type::TIMESTAMP => {
            use chrono::NaiveDateTime;
            let v: NaiveDateTime = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get timestamp value: {}", e))
            })?;
            serde_json::Value::String(v.format("%Y-%m-%dT%H:%M:%S").to_string())
        }
        Type::TIMESTAMPTZ => {
            use chrono::{DateTime, Utc};
            let v: DateTime<Utc> = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get timestamptz value: {}", e))
            })?;
            serde_json::Value::String(v.to_rfc3339())
        }

        // Date
        Type::DATE => {
            use chrono::NaiveDate;
            let v: NaiveDate = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get date value: {}", e))
            })?;
            serde_json::Value::String(v.format("%Y-%m-%d").to_string())
        }

        // Time
        Type::TIME => {
            use chrono::NaiveTime;
            let v: NaiveTime = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get time value: {}", e))
            })?;
            serde_json::Value::String(v.format("%H:%M:%S").to_string())
        }

        // UUID
        Type::UUID => {
            use uuid::Uuid;
            let v: Uuid = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get UUID value: {}", e))
            })?;
            serde_json::Value::String(v.to_string())
        }

        // Arrays (convert to JSON arrays recursively)
        // Note: PostgreSQL array support is complex - for MVP, convert to string representation
        _ if col_type.name().ends_with("[]") => {
            // For arrays, we'll use a simple string representation for MVP
            // Full array support would require recursive type handling
            let v: String = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get array value: {}", e))
            })?;
            serde_json::Value::String(v)
        }

        // Default: try to get as string
        _ => {
            let v: String = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!(
                    "Failed to convert PostgreSQL type '{}' to JSON: {}",
                    col_type.name(),
                    e
                ))
            })?;
            serde_json::Value::String(v)
        }
    };

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a running PostgreSQL instance
    // They are integration tests that should be run with:
    // cargo test --features postgres -- --ignored

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_validate_connection() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let result = PostgresEngine::validate_connection(&config);
        assert!(result.is_ok(), "Connection validation failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("PostgreSQL"));
        assert_eq!(info.connected_database, "postgres");
        assert_eq!(info.user, "postgres");
    }

    #[test]
    fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );
        config.engine = DatabaseType::SQLite;

        let result = PostgresEngine::validate_connection(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Expected PostgreSQL engine"));
    }

    #[test]
    fn test_validate_connection_missing_host() {
        let config = ConnectionConfig {
            engine: DatabaseType::Postgres,
            host: None,
            port: Some(5432),
            user: Some("postgres".to_string()),
            password: Some("postgres".to_string()),
            database: Some("postgres".to_string()),
            file: None,
        };

        let result = PostgresEngine::validate_connection(&config);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("PostgreSQL requires 'host' parameter"));
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_introspect_schema() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table first
        let create_caps = Capabilities::with_ddl();
        let _ = PostgresEngine::execute(
            &config,
            "DROP TABLE IF EXISTS test_users",
            &create_caps,
        );
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email VARCHAR(255)
            )",
            &create_caps,
        );

        // Introspect
        let result = PostgresEngine::introspect(&config, Some("public"));
        assert!(result.is_ok(), "Introspection failed: {:?}", result.err());

        let schema = result.unwrap();
        let test_table = schema.tables.iter().find(|t| t.name == "test_users");
        assert!(test_table.is_some(), "test_users table not found");

        let table = test_table.unwrap();
        assert_eq!(table.schema, Some("public".to_string()));
        assert!(table.columns.len() >= 3);

        // Check primary key
        assert!(table.primary_key.is_some());
        let pk = table.primary_key.as_ref().unwrap();
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0], "id");

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_users", &create_caps);
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_select_query() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::read_only();
        let result = PostgresEngine::execute(&config, "SELECT 1 AS num, 'test' AS str", &caps);
        assert!(result.is_ok(), "Query execution failed: {:?}", result.err());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns.len(), 2);
        assert_eq!(query_result.rows.len(), 1);
        assert_eq!(query_result.rows_affected, None);

        let row = &query_result.rows[0];
        assert_eq!(row.get("num").unwrap(), &serde_json::json!(1));
        assert_eq!(row.get("str").unwrap(), &serde_json::json!("test"));
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_insert_without_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table
        let ddl_caps = Capabilities::with_ddl();
        let _ = PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_insert", &ddl_caps);
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_insert (id SERIAL PRIMARY KEY, name TEXT)",
            &ddl_caps,
        );

        // Try to insert without write capability
        let caps = Capabilities::read_only();
        let result = PostgresEngine::execute(
            &config,
            "INSERT INTO test_insert (name) VALUES ('test')",
            &caps,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Write operations require --allow-write"));

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_insert", &ddl_caps);
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_insert_with_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table
        let ddl_caps = Capabilities::with_ddl();
        let _ = PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_insert2", &ddl_caps);
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_insert2 (id SERIAL PRIMARY KEY, name TEXT)",
            &ddl_caps,
        );

        // Insert with write capability
        let write_caps = Capabilities::with_write();
        let result = PostgresEngine::execute(
            &config,
            "INSERT INTO test_insert2 (name) VALUES ('test')",
            &write_caps,
        );

        assert!(result.is_ok(), "Insert failed: {:?}", result.err());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows_affected, Some(1));

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_insert2", &ddl_caps);
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_ddl_without_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::with_write();
        let result = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_ddl (id SERIAL PRIMARY KEY)",
            &caps,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("DDL operations require --allow-ddl"));
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_ddl_with_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::with_ddl();
        let _ = PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_ddl2", &caps);
        let result = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_ddl2 (id SERIAL PRIMARY KEY)",
            &caps,
        );

        assert!(result.is_ok(), "DDL execution failed: {:?}", result.err());

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_ddl2", &caps);
    }

    #[test]
    #[ignore] // Requires running PostgreSQL instance
    fn test_execute_max_rows_limit() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create and populate test table
        let ddl_caps = Capabilities::with_ddl();
        let _ = PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_limit", &ddl_caps);
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_limit (id SERIAL PRIMARY KEY, name TEXT)",
            &ddl_caps,
        );

        let write_caps = Capabilities::with_write();
        for i in 1..=10 {
            let _ = PostgresEngine::execute(
                &config,
                &format!("INSERT INTO test_limit (name) VALUES ('User {}')", i),
                &write_caps,
            );
        }

        // Query with row limit
        let caps = Capabilities {
            max_rows: Some(5),
            ..Capabilities::read_only()
        };
        let result = PostgresEngine::execute(&config, "SELECT * FROM test_limit", &caps);

        assert!(result.is_ok(), "Query failed: {:?}", result.err());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows.len(), 5); // Limited to 5 rows

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_limit", &ddl_caps);
    }

    #[test]
    fn test_extract_index_columns() {
        let index_def = "CREATE INDEX idx_users_email ON public.users USING btree (email)";
        let columns = extract_index_columns(index_def);
        assert_eq!(columns, vec!["email"]);

        let index_def_multi = "CREATE INDEX idx_composite ON public.orders USING btree (user_id, order_date)";
        let columns_multi = extract_index_columns(index_def_multi);
        assert_eq!(columns_multi, vec!["user_id", "order_date"]);
    }
}
