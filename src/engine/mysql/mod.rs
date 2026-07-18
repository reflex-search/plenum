//! `MySQL` Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for `MySQL` databases (including `MariaDB`).
//!
//! # Features
//! - Client-server connections via TCP
//! - Schema introspection via `information_schema`
//! - Capability-enforced query execution
//! - `MySQL` and `MariaDB` version detection
//!
//! # Implementation Notes
//! - Uses `mysql_async` (async driver, requires tokio runtime)
//! - Async operations are wrapped in synchronous interface
//! - Handles `MySQL` implicit commits for DDL operations
//! - ENUM and SET types converted to strings
//! - JSON type support (`MySQL` 5.7+)
//! - BLOB data is Base64-encoded for JSON safety
//! - Timeouts enforced via `tokio::time::timeout`
//! - Row limits enforced in application code
//! - Schema filtering supported (`MySQL` has explicit schemas/databases)

use mysql_async::{prelude::*, Conn, OptsBuilder, Params, Row, SslOpts, Value};
use std::collections::HashMap; // Used for grouping foreign keys during introspection
use std::time::{Duration, Instant};

use crate::capability::{strip_explain_prefix, validate_query};
use crate::engine::{
    is_explain_query, Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine,
    DatabaseType, ExplainFormat, ExplainPlanNode, ForeignKeyInfo, IndexInfo, IndexSummary,
    IntrospectOperation, IntrospectResult, QueryResult, SslMode, TableFields, TableInfo, TlsConfig,
    ViewInfo,
};
use crate::error::{PlenumError, Result};

/// Extra grace added to the server-side `MAX_EXECUTION_TIME` on top of the
/// client-side deadline.
///
/// The client-side `tokio::time::timeout` (set to exactly `timeout_ms`) is the
/// authoritative timeout and surfaces `QUERY_TIMEOUT`. The server-side
/// `MAX_EXECUTION_TIME` is deliberately set *longer* so it acts only as a
/// server-side cleanup backstop for a query the client has already abandoned —
/// it must NOT fire first. Firing first would let statements that swallow the
/// interrupt (notably `SELECT SLEEP(n)`, which returns `1` when interrupted
/// rather than erroring) complete "successfully" and report partial data as a
/// success, defeating the timeout (REF-258 Bug 4).
const SERVER_TIMEOUT_BACKSTOP_GRACE: Duration = Duration::from_secs(5);

/// `MySQL` database engine implementation
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
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {e}"))
        })?;

        // Get MySQL version
        let version_row: Row = conn
            .query_first("SELECT VERSION()")
            .await
            .map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query MySQL version: {e}"))
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
                PlenumError::connection_failed(format!("Failed to query current database: {e}"))
            })?
            .ok_or_else(|| PlenumError::connection_failed("No database returned".to_string()))?;

        // Handle NULL result when using wildcard database ("*").
        // `DATABASE()` returns SQL NULL in wildcard mode; `get::<String>` would panic
        // converting NULL, so read as `Option<String>` (outer Option = index presence,
        // inner Option = NULL vs value).
        let connected_database: String = match db_row.get::<Option<String>, _>(0) {
            Some(Some(db)) => db,
            _ => "(no database selected)".to_string(), // Wildcard mode (NULL)
        };

        // Get current user
        let user_row: Row = conn
            .query_first("SELECT CURRENT_USER()")
            .await
            .map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query current user: {e}"))
            })?
            .ok_or_else(|| PlenumError::connection_failed("No user returned".to_string()))?;

        let user: String = user_row
            .get(0)
            .ok_or_else(|| PlenumError::connection_failed("Failed to extract user".to_string()))?;

        // Close connection
        conn.disconnect()
            .await
            .map_err(|e| PlenumError::connection_failed(format!("Failed to disconnect: {e}")))?;

        Ok(ConnectionInfo { database_version, server_info, connected_database, user })
    }

    async fn introspect(
        config: &ConnectionConfig,
        operation: &IntrospectOperation,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<IntrospectResult> {
        // Validate config is for MySQL
        if config.engine != DatabaseType::MySQL {
            return Err(PlenumError::invalid_input(format!(
                "Expected MySQL engine, got {}",
                config.engine
            )));
        }

        // Handle database override by modifying config
        let mut effective_config = config.clone();
        if let Some(db) = database {
            effective_config.database = Some(db.to_string());
        }

        // Build connection options
        let opts = build_mysql_opts(&effective_config)?;

        // Connect to MySQL
        let mut conn = Conn::new(opts).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {e}"))
        })?;

        // Route to appropriate handler based on operation
        let result = match operation {
            IntrospectOperation::ListDatabases => list_databases_mysql(&mut conn).await?,

            IntrospectOperation::ListSchemas => {
                // MySQL doesn't have separate schemas - database = schema
                return Err(PlenumError::invalid_input(
                    "MySQL does not have separate schemas. In MySQL, databases and schemas are synonymous. Use --list-databases instead."
                ));
            }

            IntrospectOperation::ListTables => {
                let target_schema = determine_target_schema(&mut conn, schema).await?;
                list_tables_mysql(&mut conn, &target_schema).await?
            }

            IntrospectOperation::ListViews => {
                let target_schema = determine_target_schema(&mut conn, schema).await?;
                list_views_mysql(&mut conn, &target_schema).await?
            }

            IntrospectOperation::ListIndexes { table } => {
                let target_schema = determine_target_schema(&mut conn, schema).await?;
                list_indexes_mysql(&mut conn, &target_schema, table.as_deref()).await?
            }

            IntrospectOperation::TableDetails { name, fields } => {
                let target_schema = determine_target_schema(&mut conn, schema).await?;
                get_table_details_mysql(&mut conn, &target_schema, name, fields).await?
            }

            IntrospectOperation::ViewDetails { name } => {
                let target_schema = determine_target_schema(&mut conn, schema).await?;
                get_view_details_mysql(&mut conn, &target_schema, name).await?
            }
        };

        // Close connection
        conn.disconnect().await.map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to disconnect: {e}"))
        })?;

        Ok(result)
    }

    async fn execute(
        config: &ConnectionConfig,
        query: &str,
        params: &[serde_json::Value],
        caps: &Capabilities,
    ) -> Result<QueryResult> {
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
            PlenumError::connection_failed(format!("Failed to connect to MySQL: {e}"))
        })?;

        // Defense in depth: enforce session-level read-only at the database layer.
        // This rejects DML writes even if the SQL parser is somehow bypassed (REF-261).
        // Note: MySQL DDL (CREATE/DROP/ALTER) causes implicit commits and is not covered
        // by transaction read-only mode — that class is already blocked by the parser.
        conn.exec_drop("SET SESSION TRANSACTION READ ONLY", ()).await.map_err(|e| {
            PlenumError::engine_error(
                "mysql",
                format!("Failed to enforce session read-only mode: {e}"),
            )
        })?;

        // Set server-side MAX_EXECUTION_TIME as a cleanup backstop so MySQL eventually
        // cancels a query the client has abandoned. It is set LONGER than the client-side
        // deadline (see SERVER_TIMEOUT_BACKSTOP_GRACE): the client-side timeout is the
        // authoritative one, because MAX_EXECUTION_TIME does not reliably error for
        // statements that swallow the interrupt (e.g. SELECT SLEEP()). Only applies to
        // SELECT statements in MySQL.
        if let Some(timeout_ms) = caps.timeout_ms {
            let server_limit = Duration::from_millis(timeout_ms) + SERVER_TIMEOUT_BACKSTOP_GRACE;
            conn.exec_drop(
                format!("SET SESSION MAX_EXECUTION_TIME = {}", server_limit.as_millis()),
                (),
            )
            .await
            .map_err(|e| {
                PlenumError::engine_error("mysql", format!("Failed to set MAX_EXECUTION_TIME: {e}"))
            })?;
        }

        // The client-side tokio timeout is the authoritative deadline: it fires at exactly
        // timeout_ms and surfaces QUERY_TIMEOUT. This guarantees a bounded query that runs
        // to the limit is reported as a timeout error rather than as a success with partial
        // or interrupted data (REF-258 Bug 4) — even for statements like SELECT SLEEP() that
        // the server-side MAX_EXECUTION_TIME would let return "successfully".

        // Structured explain path: rewrite to EXPLAIN FORMAT=JSON, normalize the plan tree.
        if caps.explain_format == Some(ExplainFormat::Structured) {
            if !is_explain_query(query) {
                return Err(PlenumError::invalid_input(
                    "--explain-format structured requires an EXPLAIN statement; \
                     non-EXPLAIN queries must omit this flag",
                ));
            }
            let inner = strip_explain_prefix(query);
            let start = Instant::now();
            let plan = execute_structured_explain_mysql(&mut conn, &inner).await?;
            let elapsed = start.elapsed();
            conn.disconnect().await.ok();
            return Ok(QueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
                rows_affected: None,
                execution_ms: elapsed.as_millis() as u64,
                rows_truncated: false,
                truncated_by: None,
                plan: Some(plan),
            });
        }

        let start = Instant::now();
        let mut query_result = if let Some(timeout_ms) = caps.timeout_ms {
            let deadline = Duration::from_millis(timeout_ms);
            tokio::time::timeout(deadline, execute_query(&mut conn, query, params, caps))
                .await
                .map_err(|_| {
                PlenumError::query_timeout(format!(
                    "Query exceeded the client-side timeout of {timeout_ms}ms"
                ))
            })??
        } else {
            execute_query(&mut conn, query, params, caps).await?
        };

        let elapsed = start.elapsed();
        query_result.execution_ms = elapsed.as_millis() as u64;

        // Close connection
        conn.disconnect().await.map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to disconnect: {e}"))
        })?;

        Ok(query_result)
    }
}

/// Build `MySQL` connection options from `ConnectionConfig`
fn build_mysql_opts(config: &ConnectionConfig) -> Result<OptsBuilder> {
    let host = config
        .host
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'host' parameter"))?;

    let port =
        config.port.ok_or_else(|| PlenumError::invalid_input("MySQL requires 'port' parameter"))?;

    let user = config
        .user
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'user' parameter"))?;

    let password = config
        .password
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("MySQL requires 'password' parameter"))?;

    // Database can be "*" for wildcard (no database selected) or a specific database name
    let database = config.database.as_ref();

    // Check if database is wildcard ("*") - if so, connect without selecting a database
    let db_name = match database {
        Some(db) if db == "*" => None, // Wildcard - no database selected
        Some(db) => Some(db.as_str()), // Explicit database
        None => {
            return Err(PlenumError::invalid_input(
                "MySQL requires 'database' parameter (use \"*\" for no database)",
            ))
        }
    };

    let mut opts = OptsBuilder::default()
        .ip_or_hostname(host)
        .tcp_port(port)
        .user(Some(user))
        .pass(Some(password))
        .db_name(db_name);

    // Apply TLS options when sslmode is not Disable.
    if let Some(tls_config) = &config.tls {
        if tls_config.sslmode != SslMode::Disable {
            let ssl_opts = build_mysql_ssl_opts(tls_config)?;
            opts = opts.ssl_opts(ssl_opts);
        }
    }

    Ok(opts)
}

/// Build `mysql_async::SslOpts` from a `TlsConfig`.
///
/// Error messages deliberately omit cert/key paths to prevent credential leakage.
fn build_mysql_ssl_opts(tls: &TlsConfig) -> Result<SslOpts> {
    let mut ssl_opts = SslOpts::default();

    match &tls.sslmode {
        SslMode::Require => {
            // Require TLS; skip both cert validation and hostname check.
            ssl_opts = ssl_opts
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true);
        }
        SslMode::VerifyCa => {
            // Verify cert against CA; skip hostname check.
            let ca_path = tls.ca_cert.as_ref().ok_or_else(|| {
                PlenumError::connection_failed(
                    "sslmode=verify-ca requires a CA certificate (--ssl-ca)",
                )
            })?;
            ssl_opts = ssl_opts
                .with_root_certs(vec![ca_path.clone().into()])
                .with_danger_skip_domain_validation(true);
        }
        SslMode::VerifyFull => {
            // Full verification: CA cert + hostname.
            let ca_path = tls.ca_cert.as_ref().ok_or_else(|| {
                PlenumError::connection_failed(
                    "sslmode=verify-full requires a CA certificate (--ssl-ca)",
                )
            })?;
            ssl_opts = ssl_opts.with_root_certs(vec![ca_path.clone().into()]);
        }
        SslMode::Disable => {
            unreachable!("Disable mode is filtered before calling this function")
        }
    }

    // mTLS: load client cert + key if provided.
    if let (Some(cert_path), Some(key_path)) = (&tls.client_cert, &tls.client_key) {
        let identity =
            mysql_async::ClientIdentity::new(cert_path.clone().into(), key_path.clone().into());
        ssl_opts = ssl_opts.with_client_identity(Some(identity));
    }

    Ok(ssl_opts)
}

/// Parse `MySQL` version string to detect `MySQL` vs `MariaDB`
fn parse_mysql_version(version_string: &str) -> (String, String) {
    // Example MySQL: "8.0.35"
    // Example MariaDB: "10.11.2-MariaDB"

    if version_string.to_uppercase().contains("MARIADB") {
        // MariaDB
        let version = version_string.split('-').next().unwrap_or("unknown").to_string();
        (version.clone(), format!("MariaDB {version}"))
    } else {
        // MySQL
        let version =
            version_string.split_whitespace().next().unwrap_or(version_string).to_string();
        (version.clone(), format!("MySQL {version}"))
    }
}

// ============================================================================
// Introspection Operation Handlers
// ============================================================================

/// Determine target schema/database for operations
///
/// If `schema_filter` is provided, use it.
/// Otherwise, use the current database from the connection.
async fn determine_target_schema(conn: &mut Conn, schema_filter: Option<&str>) -> Result<String> {
    if let Some(schema) = schema_filter {
        return Ok(schema.to_string());
    }

    // Get current database
    let db_row: Row = conn
        .query_first("SELECT DATABASE()")
        .await
        .map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to query current database: {e}"))
        })?
        .ok_or_else(|| PlenumError::engine_error("mysql", "No database selected".to_string()))?;

    // Check if database is NULL (wildcard mode). `DATABASE()` returns SQL NULL when no
    // database is selected; read as `Option<String>` so NULL does not panic the converter.
    let db_name: Option<String> = db_row.get::<Option<String>, _>(0).flatten();
    db_name.ok_or_else(|| {
        PlenumError::engine_error(
            "mysql",
            "No database selected. When using wildcard database connection (\"*\"), you must specify --schema or --database parameter.".to_string()
        )
    })
}

/// List all databases
async fn list_databases_mysql(conn: &mut Conn) -> Result<IntrospectResult> {
    let rows: Vec<Row> = conn.query("SHOW DATABASES").await.map_err(|e| {
        PlenumError::engine_error("mysql", format!("Failed to list databases: {e}"))
    })?;

    let databases: Vec<String> = rows.into_iter().filter_map(|row| row.get(0)).collect();

    Ok(IntrospectResult::DatabaseList { databases })
}

/// List all table names in a schema
async fn list_tables_mysql(conn: &mut Conn, schema: &str) -> Result<IntrospectResult> {
    let query = "SELECT table_name
                 FROM information_schema.tables
                 WHERE table_schema = ?
                 AND table_type = 'BASE TABLE'
                 ORDER BY table_name";

    let rows: Vec<Row> = conn
        .exec(query, (schema,))
        .await
        .map_err(|e| PlenumError::engine_error("mysql", format!("Failed to list tables: {e}")))?;

    let tables: Vec<String> = rows.into_iter().filter_map(|row| row.get(0)).collect();

    Ok(IntrospectResult::TableList { tables })
}

/// List all view names in a schema
async fn list_views_mysql(conn: &mut Conn, schema: &str) -> Result<IntrospectResult> {
    let query = "SELECT table_name
                 FROM information_schema.views
                 WHERE table_schema = ?
                 ORDER BY table_name";

    let rows: Vec<Row> = conn
        .exec(query, (schema,))
        .await
        .map_err(|e| PlenumError::engine_error("mysql", format!("Failed to list views: {e}")))?;

    let views: Vec<String> = rows.into_iter().filter_map(|row| row.get(0)).collect();

    Ok(IntrospectResult::ViewList { views })
}

/// List all indexes (optionally filtered by table)
async fn list_indexes_mysql(
    conn: &mut Conn,
    schema: &str,
    table_filter: Option<&str>,
) -> Result<IntrospectResult> {
    let query = if table_filter.is_some() {
        "SELECT DISTINCT
            index_name,
            table_name,
            non_unique,
            GROUP_CONCAT(column_name ORDER BY seq_in_index) as columns
         FROM information_schema.statistics
         WHERE table_schema = ? AND table_name = ?
         AND index_name != 'PRIMARY'
         GROUP BY index_name, table_name, non_unique
         ORDER BY table_name, index_name"
    } else {
        "SELECT DISTINCT
            index_name,
            table_name,
            non_unique,
            GROUP_CONCAT(column_name ORDER BY seq_in_index) as columns
         FROM information_schema.statistics
         WHERE table_schema = ?
         AND index_name != 'PRIMARY'
         GROUP BY index_name, table_name, non_unique
         ORDER BY table_name, index_name"
    };

    let rows: Vec<Row> = if let Some(table) = table_filter {
        conn.exec(query, (schema, table)).await
    } else {
        conn.exec(query, (schema,)).await
    }
    .map_err(|e| PlenumError::engine_error("mysql", format!("Failed to list indexes: {e}")))?;

    let mut indexes = Vec::new();
    for row in rows {
        let name: String = row.get(0).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract index name".to_string())
        })?;
        let table: String = row.get(1).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract table name".to_string())
        })?;
        let non_unique: i64 = row.get(2).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract non_unique flag".to_string())
        })?;
        let columns_str: String = row.get(3).ok_or_else(|| {
            PlenumError::engine_error("mysql", "Failed to extract columns".to_string())
        })?;

        let columns: Vec<String> = columns_str.split(',').map(ToString::to_string).collect();

        indexes.push(IndexSummary { name, table, unique: non_unique == 0, columns });
    }

    Ok(IntrospectResult::IndexList { indexes })
}

/// Get full details for a specific table (with field filtering)
async fn get_table_details_mysql(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
    fields: &TableFields,
) -> Result<IntrospectResult> {
    // Get requested fields
    let columns = if fields.columns {
        introspect_columns(conn, schema, table_name).await?
    } else {
        Vec::new()
    };

    let primary_key = if fields.primary_key {
        introspect_primary_key(conn, schema, table_name).await?
    } else {
        None
    };

    let foreign_keys = if fields.foreign_keys {
        introspect_foreign_keys(conn, schema, table_name).await?
    } else {
        Vec::new()
    };

    let indexes = if fields.indexes {
        introspect_indexes(conn, schema, table_name).await?
    } else {
        Vec::new()
    };

    let (comment, row_estimate) = introspect_table_meta(conn, schema, table_name).await?;

    let table = TableInfo {
        name: table_name.to_string(),
        schema: Some(schema.to_string()),
        columns,
        primary_key,
        foreign_keys,
        indexes,
        comment,
        row_estimate,
    };

    Ok(IntrospectResult::TableDetails { table })
}

/// Get details for a specific view
async fn get_view_details_mysql(
    conn: &mut Conn,
    schema: &str,
    view_name: &str,
) -> Result<IntrospectResult> {
    // Get view definition
    let def_query = "SELECT view_definition
                     FROM information_schema.views
                     WHERE table_schema = ? AND table_name = ?";

    let def_row: Option<Row> =
        conn.exec_first(def_query, (schema, view_name)).await.map_err(|e| {
            PlenumError::engine_error("mysql", format!("Failed to query view definition: {e}"))
        })?;

    let definition: Option<String> = def_row.and_then(|row| row.get(0));

    if definition.is_none() {
        return Err(PlenumError::engine_error(
            "mysql",
            format!("View '{view_name}' not found in schema '{schema}'"),
        ));
    }

    // Get view columns (same query as for tables)
    let columns = introspect_columns(conn, schema, view_name).await?;

    let view = ViewInfo {
        name: view_name.to_string(),
        schema: Some(schema.to_string()),
        definition,
        columns,
    };

    Ok(IntrospectResult::ViewDetails { view })
}

/// Introspect all tables in the database (DEPRECATED - kept for backward compatibility)
async fn introspect_all_tables(conn: &mut Conn, schema: &str) -> Result<Vec<TableInfo>> {
    // Query information_schema.tables for table list
    let query = "SELECT table_name
                 FROM information_schema.tables
                 WHERE table_schema = ?
                 AND table_type = 'BASE TABLE'
                 ORDER BY table_name";

    let rows: Vec<Row> = conn
        .exec(query, (schema,))
        .await
        .map_err(|e| PlenumError::engine_error("mysql", format!("Failed to query tables: {e}")))?;

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
        comment: None,
        row_estimate: None,
    })
}

/// Helper function to safely extract an optional string from a `MySQL` row
/// Returns None if the value is NULL, otherwise attempts to convert to String
fn get_optional_string(row: &Row, idx: usize) -> Option<String> {
    match row.as_ref(idx)? {
        Value::NULL => None,
        _ => row.get(idx),
    }
}

/// Introspect table columns (includes column comments from `information_schema`)
async fn introspect_columns(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
) -> Result<Vec<ColumnInfo>> {
    let query = "SELECT column_name, data_type, is_nullable, column_default, column_comment
                 FROM information_schema.columns
                 WHERE table_schema = ? AND table_name = ?
                 ORDER BY ordinal_position";

    let rows: Vec<Row> = conn.exec(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query columns for {schema}.{table_name}: {e}"),
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
        let default: Option<String> = get_optional_string(&row, 3);
        // MySQL stores empty string when no comment is set; normalise to None
        let comment: Option<String> = get_optional_string(&row, 4).filter(|s| !s.is_empty());

        columns.push(ColumnInfo {
            name: column_name,
            data_type,
            nullable: is_nullable == "YES",
            default,
            comment,
        });
    }

    Ok(columns)
}

/// Fetch table-level comment and row estimate from `information_schema.tables`
async fn introspect_table_meta(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
) -> Result<(Option<String>, Option<i64>)> {
    let query = "SELECT table_comment, table_rows
                 FROM information_schema.tables
                 WHERE table_schema = ? AND table_name = ?";

    let row: Option<Row> = conn.exec_first(query, (schema, table_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query table metadata for {schema}.{table_name}: {e}"),
        )
    })?;

    match row {
        None => Ok((None, None)),
        Some(r) => {
            let comment = get_optional_string(&r, 0).filter(|s| !s.is_empty());
            let row_estimate: Option<i64> = r.get(1);
            Ok((comment, row_estimate))
        }
    }
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
            format!("Failed to query primary key for {schema}.{table_name}: {e}"),
        )
    })?;

    if rows.is_empty() {
        return Ok(None);
    }

    let pk_columns: Vec<String> =
        rows.into_iter().filter_map(|row| get_optional_string(&row, 0)).collect();

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
            format!("Failed to query foreign keys for {schema}.{table_name}: {e}"),
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
async fn introspect_indexes(
    conn: &mut Conn,
    schema: &str,
    table_name: &str,
) -> Result<Vec<IndexInfo>> {
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
            format!("Failed to query indexes for {schema}.{table_name}: {e}"),
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

        indexes.push(IndexInfo { name: index_name, columns, unique: non_unique == 0 });
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

    let rows: Vec<Row> = conn.exec(query, (schema, table_name, index_name)).await.map_err(|e| {
        PlenumError::engine_error(
            "mysql",
            format!("Failed to query columns for index {schema}.{table_name}.{index_name}: {e}"),
        )
    })?;

    let columns: Vec<String> =
        rows.into_iter().filter_map(|row| get_optional_string(&row, 0)).collect();

    Ok(columns)
}

/// Convert a JSON value to a `mysql_async` native `Value` for parameter binding.
/// Uses `?` `MySQL` placeholders.
fn json_to_mysql_value(val: &serde_json::Value) -> Value {
    match val {
        serde_json::Value::Null => Value::NULL,
        serde_json::Value::Bool(b) => Value::Int(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else {
                Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Bytes(s.as_bytes().to_vec()),
        v => Value::Bytes(v.to_string().into_bytes()),
    }
}

/// Returns true when a `mysql_async` error is a server-side `MAX_EXECUTION_TIME` timeout (error 3024).
fn is_mysql_statement_timeout(e: &mysql_async::Error) -> bool {
    match e {
        mysql_async::Error::Server(ref server_err) => server_err.code == 3024,
        _ => false,
    }
}

/// Map a `mysql_async` execution error to a `PlenumError`, surfacing server-side
/// `MAX_EXECUTION_TIME` cancellation as `QUERY_TIMEOUT` and everything else as a
/// generic query failure.
fn map_mysql_exec_error(e: &mysql_async::Error) -> PlenumError {
    if is_mysql_statement_timeout(e) {
        PlenumError::query_timeout(format!(
            "Query cancelled by MySQL server-side MAX_EXECUTION_TIME: {e}"
        ))
    } else {
        PlenumError::query_failed(format!("Failed to execute query: {e}"))
    }
}

/// Execute query and return `QueryResult`
async fn execute_query(
    conn: &mut Conn,
    query: &str,
    params: &[serde_json::Value],
    caps: &Capabilities,
) -> Result<QueryResult> {
    // Classify the statement. MySQL async doesn't have a prepare-then-check
    // pattern like tokio-postgres, so use a keyword heuristic. `EXPLAIN` (in all
    // its FORMAT/ANALYZE/EXTENDED forms) returns a result set and must be treated
    // as row-returning, otherwise its plan rows are silently dropped (REF-258 Bug 2).
    let query_upper = query.trim_start().to_uppercase();
    let returns_rows = query_upper.starts_with("SELECT")
        || query_upper.starts_with("SHOW")
        || query_upper.starts_with("DESCRIBE")
        || query_upper.starts_with("DESC")
        || query_upper.starts_with("EXPLAIN")
        || (query_upper.starts_with("WITH") && query_upper.contains("SELECT"));

    // Protocol selection: the binary prepared-statement protocol (exec/exec_iter)
    // rejects several permitted statement classes — notably transaction-control
    // statements (BEGIN, START TRANSACTION) fail with error 1295 "not supported in
    // the prepared statement protocol yet" (REF-258 Bug 1). Route through the text
    // protocol (query/query_iter) whenever there are no parameters to bind, and only
    // fall back to the prepared protocol when bound params require it.
    let use_text_protocol = params.is_empty();

    // Convert JSON params → mysql_async positional params (only used when binding).
    let mysql_params: Params = if params.is_empty() {
        Params::Empty
    } else {
        Params::Positional(params.iter().map(json_to_mysql_value).collect())
    };

    if returns_rows {
        // Query returns rows. Use the text protocol for unparameterized queries
        // (so EXPLAIN/transaction-adjacent statements execute) and the prepared
        // protocol only when bound params are present.
        let rows: Vec<Row> = if use_text_protocol {
            conn.query(query).await
        } else {
            conn.exec(query, mysql_params).await
        }
        .map_err(|e| map_mysql_exec_error(&e))?;

        // Get column names from first row (if any)
        let column_names: Vec<String> = if let Some(first_row) = rows.first() {
            first_row.columns_ref().iter().map(|col| col.name_str().to_string()).collect()
        } else {
            Vec::new()
        };

        // Apply offset and max_rows with truncation detection
        let offset = caps.offset.unwrap_or(0);
        let effective = if offset <= rows.len() { &rows[offset..] } else { &[][..] };
        let max = caps.max_rows.unwrap_or(usize::MAX);
        let rows_truncated = effective.len() > max;
        let take = effective.len().min(max);

        let mut rows_data = Vec::new();
        for row in &effective[..take] {
            rows_data.push(row_to_json(row)?);
        }

        Ok(QueryResult {
            columns: column_names,
            rows: rows_data,
            rows_affected: None,
            execution_ms: 0,
            rows_truncated,
            truncated_by: None,
            plan: None,
        })
    } else {
        // Non-row statement (e.g. transaction control: BEGIN/START TRANSACTION).
        // These have no result set; capture affected rows. Prefer the text protocol
        // for unparameterized statements so transaction control succeeds (Bug 1).
        let rows_affected = if use_text_protocol {
            let result = conn.query_iter(query).await.map_err(|e| map_mysql_exec_error(&e))?;
            let affected = result.affected_rows();
            drop(result);
            affected
        } else {
            let result =
                conn.exec_iter(query, mysql_params).await.map_err(|e| map_mysql_exec_error(&e))?;
            let affected = result.affected_rows();
            drop(result);
            affected
        };

        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: Some(rows_affected),
            execution_ms: 0,
            rows_truncated: false,
            truncated_by: None,
            plan: None,
        })
    }
}

/// Execute `EXPLAIN FORMAT=JSON` against the inner SQL and normalize the result.
async fn execute_structured_explain_mysql(
    conn: &mut Conn,
    inner_sql: &str,
) -> Result<ExplainPlanNode> {
    let sql = format!("EXPLAIN FORMAT=JSON {inner_sql}");

    let rows: Vec<Row> = conn.query(sql).await.map_err(|e| {
        PlenumError::query_failed(format!("Failed to execute EXPLAIN FORMAT=JSON: {e}"))
    })?;

    let row = rows.first().ok_or_else(|| {
        PlenumError::query_failed("EXPLAIN FORMAT=JSON returned no rows".to_string())
    })?;

    // MySQL returns one text column containing the JSON plan
    let plan_json: String = row.get(0).ok_or_else(|| {
        PlenumError::query_failed("EXPLAIN FORMAT=JSON row was empty".to_string())
    })?;

    let plan_value: serde_json::Value = serde_json::from_str(&plan_json).map_err(|e| {
        PlenumError::query_failed(format!("Failed to parse EXPLAIN JSON from MySQL: {e}"))
    })?;

    let query_block = plan_value.get("query_block").ok_or_else(|| {
        PlenumError::query_failed(
            "Unexpected MySQL EXPLAIN JSON structure (missing query_block)".to_string(),
        )
    })?;

    Ok(normalize_mysql_query_block(query_block))
}

/// Normalize a `MySQL` `query_block` into an `ExplainPlanNode`.
fn normalize_mysql_query_block(block: &serde_json::Value) -> ExplainPlanNode {
    let estimated_cost = block
        .get("cost_info")
        .and_then(|c| c.get("query_cost"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<f64>().ok());

    ExplainPlanNode {
        node_type: "query_block".to_string(),
        relation: None,
        estimated_rows: None,
        estimated_cost,
        children: collect_mysql_children(block),
    }
}

/// Collect child plan nodes from a `MySQL` plan block.
fn collect_mysql_children(block: &serde_json::Value) -> Vec<ExplainPlanNode> {
    let mut children = Vec::new();

    // Direct single-table access
    if let Some(table) = block.get("table") {
        children.push(normalize_mysql_table_node(table));
    }

    // JOIN: nested_loop is an array of {table: {...}} entries
    if let Some(nested) = block.get("nested_loop").and_then(serde_json::Value::as_array) {
        for item in nested {
            if let Some(table) = item.get("table") {
                children.push(normalize_mysql_table_node(table));
            }
        }
    }

    // Derived / subquery blocks
    for key in &["select_list_subqueries", "attached_subqueries", "grouping_operation"] {
        if let Some(arr) = block.get(key).and_then(serde_json::Value::as_array) {
            for item in arr {
                if let Some(qb) = item.get("query_block") {
                    children.push(normalize_mysql_query_block(qb));
                }
            }
        }
    }

    children
}

/// Normalize a `MySQL` `table` object into an `ExplainPlanNode`.
fn normalize_mysql_table_node(table: &serde_json::Value) -> ExplainPlanNode {
    let node_type = table
        .get("access_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let relation = table.get("table_name").and_then(serde_json::Value::as_str).map(String::from);

    let estimated_rows = table.get("rows_examined_per_scan").and_then(serde_json::Value::as_f64);

    let estimated_cost = table
        .get("cost_info")
        .and_then(|c| c.get("read_cost"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<f64>().ok());

    ExplainPlanNode { node_type, relation, estimated_rows, estimated_cost, children: Vec::new() }
}

/// Convert a `MySQL` row to a JSON-safe `Vec`
fn row_to_json(row: &Row) -> Result<Vec<serde_json::Value>> {
    let mut values = Vec::with_capacity(row.columns_ref().len());

    for idx in 0..row.columns_ref().len() {
        let value = mysql_value_to_json(row, idx)?;
        values.push(value);
    }

    Ok(values)
}

/// Convert `MySQL` value to JSON value
fn mysql_value_to_json(row: &Row, idx: usize) -> Result<serde_json::Value> {
    let value = row
        .as_ref(idx)
        .ok_or_else(|| PlenumError::query_failed(format!("Failed to get value at index {idx}")))?;

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

        Value::Float(f) => serde_json::Number::from_f64(f64::from(*f))
            .map_or(serde_json::Value::Null, serde_json::Value::Number), // Handle NaN/Infinity as null

        Value::Double(d) => serde_json::Number::from_f64(*d)
            .map_or(serde_json::Value::Null, serde_json::Value::Number), // Handle NaN/Infinity as null

        Value::Date(year, month, day, hour, minute, second, micro) => {
            // Format as ISO 8601 datetime string
            let datetime_str = format!(
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{micro:06}"
            );
            serde_json::Value::String(datetime_str)
        }

        Value::Time(is_negative, days, hours, minutes, seconds, microseconds) => {
            // Format as time duration string
            let sign = if *is_negative { "-" } else { "" };
            let total_hours = days * 24 + u32::from(*hours);
            let time_str =
                format!("{sign}{total_hours}:{minutes:02}:{seconds:02}.{microseconds:06}");
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
    fn test_wildcard_database_config() {
        // Test that wildcard database is accepted
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "*".to_string(),
        );

        // Should accept "*" as database
        assert_eq!(config.database, Some("*".to_string()));

        // Build opts should work with wildcard
        let result = build_mysql_opts(&config);
        assert!(result.is_ok(), "Failed to build MySQL opts with wildcard: {:?}", result.err());
    }

    #[test]
    fn test_missing_database_error() {
        // Test that missing database gives helpful error
        let config = ConnectionConfig {
            engine: DatabaseType::MySQL,
            host: Some("localhost".to_string()),
            port: Some(3306),
            user: Some("root".to_string()),
            password: Some("password".to_string()),
            database: None,
            file: None,
            tls: None,
        };

        let result = build_mysql_opts(&config);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message().contains("MySQL requires 'database' parameter"));
        assert!(error.message().contains("\"*\""));
    }

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_validate_connection() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let result = MySqlEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "Connection validation failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("MySQL") || info.server_info.contains("MariaDB"));
    }

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_validate_connection_wildcard() {
        // Test connection with wildcard database
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "*".to_string(),
        );

        let result = MySqlEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "Wildcard connection validation failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("MySQL") || info.server_info.contains("MariaDB"));
        assert_eq!(info.connected_database, "(no database selected)");
    }

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_query_show_databases_wildcard() {
        // Test SHOW DATABASES query with wildcard database
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "*".to_string(),
        );

        let caps = Capabilities::default();
        let result = MySqlEngine::execute(&config, "SHOW DATABASES", &[], &caps).await;
        assert!(result.is_ok(), "SHOW DATABASES failed: {:?}", result.err());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns.len(), 1);
        assert!(!query_result.rows.is_empty());
    }

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_introspect_without_schema_wildcard() {
        // Test that introspect fails without schema when using wildcard
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "*".to_string(),
        );

        let result =
            MySqlEngine::introspect(&config, &IntrospectOperation::ListTables, None, None).await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message().contains("wildcard database"));
        assert!(error.message().contains("--schema"));
    }

    #[tokio::test]
    async fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.engine = DatabaseType::Postgres;

        let result = MySqlEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Expected MySQL engine"));
    }

    #[tokio::test]
    async fn test_validate_connection_missing_host() {
        let config = ConnectionConfig {
            engine: DatabaseType::MySQL,
            host: None,
            port: Some(3306),
            user: Some("root".to_string()),
            password: Some("password".to_string()),
            database: Some("test".to_string()),
            file: None,
            tls: None,
        };

        let result = MySqlEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("MySQL requires 'host' parameter"));
    }

    // -------------------------------------------------------------------------
    // REF-270: TLS/SSL configuration tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_mysql_verify_ca_requires_ca_cert() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::VerifyCa,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        });
        let result = build_mysql_opts(&config);
        assert!(result.is_err());
        let msg = result.unwrap_err().message();
        assert!(msg.contains("CA certificate"), "expected mention of CA cert: {msg}");
        assert!(!msg.contains('/'), "must not contain file path: {msg}");
    }

    #[test]
    fn test_mysql_verify_full_requires_ca_cert() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::VerifyFull,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        });
        let result = build_mysql_opts(&config);
        assert!(result.is_err());
        let msg = result.unwrap_err().message();
        assert!(msg.contains("CA certificate"), "expected mention of CA cert: {msg}");
        assert!(!msg.contains('/'), "must not contain file path: {msg}");
    }

    #[test]
    fn test_mysql_require_mode_builds_without_ca() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::Require,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        });
        // sslmode=require should not fail at opts-build time (no CA needed)
        assert!(build_mysql_opts(&config).is_ok());
    }

    #[test]
    fn test_mysql_disable_mode_no_ssl_opts() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::Disable,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        });
        // sslmode=disable → plaintext; no SSL opts needed and should not error
        assert!(build_mysql_opts(&config).is_ok());
    }

    /// Verify an encrypted (`sslmode=require`) connection succeeds against a `MySQL` server.
    ///
    /// The stock `mysql:8.0` image serves auto-generated self-signed certificates whose CA
    /// is not in any system trust store and whose CN does not match `localhost`, so
    /// `verify-ca`/`verify-full` cannot succeed against it without provisioning custom certs.
    /// `require` establishes TLS while accepting the self-signed cert, which is the property
    /// this integration test exercises: that TLS negotiation works end-to-end.
    #[tokio::test]
    #[ignore = "Requires running MySQL instance with TLS enabled"]
    async fn test_mysql_tls_require_connects() {
        let mut config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::Require,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        });
        let result = MySqlEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "TLS require connection failed: {:?}", result.err());
    }

    // Additional integration tests would follow the pattern from postgres/mod.rs
    // Testing introspection, query execution, capability enforcement, etc.

    /// Prove that DML writes fail at the `MySQL` session layer independently of the parser.
    ///
    /// Applies `SET SESSION TRANSACTION READ ONLY` (same as `execute()` does), then
    /// attempts a direct DML write without going through Plenum's `validate_query`.
    /// The write must be rejected by `MySQL` itself (REF-261).
    ///
    /// Note: `MySQL` DDL statements (CREATE/DROP/ALTER) issue implicit commits and are
    /// not subject to transaction read-only enforcement — they are handled exclusively
    /// by the parser. This test covers the DML threat class only.
    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_mysql_session_read_only_enforcement() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let opts = build_mysql_opts(&config).unwrap();
        let mut conn = Conn::new(opts).await.unwrap();

        // Set up a test table using a separate write (outside of read-only session)
        conn.exec_drop("CREATE TABLE IF NOT EXISTS _plenum_ro_test_mysql (id INT)", ())
            .await
            .unwrap();

        // Apply session-level read-only (same as execute() does)
        conn.exec_drop("SET SESSION TRANSACTION READ ONLY", ()).await.unwrap();

        // Verify the setting is active
        let row: Option<Row> =
            conn.query_first("SELECT @@session.transaction_read_only").await.unwrap();
        let value: i64 = row.unwrap().get(0).unwrap();
        assert_eq!(value, 1, "session transaction_read_only must be 1");

        // Attempt a direct INSERT bypassing Plenum's parser — must fail at the DB layer.
        let result = conn.exec_drop("INSERT INTO _plenum_ro_test_mysql VALUES (1)", ()).await;
        assert!(result.is_err(), "INSERT must be rejected by MySQL session read-only mode");

        // Clean up (requires a new connection since this one is read-only)
        conn.disconnect().await.ok();

        let opts2 = build_mysql_opts(&config).unwrap();
        let mut conn2 = Conn::new(opts2).await.unwrap();
        conn2.exec_drop("DROP TABLE IF EXISTS _plenum_ro_test_mysql", ()).await.ok();
        conn2.disconnect().await.ok();
    }

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_timeout_fires_on_heavy_query() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        // 50ms timeout against a heavy cartesian join over information_schema. The
        // authoritative client-side timeout must cancel it and surface QUERY_TIMEOUT.
        let caps = Capabilities { timeout_ms: Some(50), ..Capabilities::default() };
        let result = MySqlEngine::execute(
            &config,
            "SELECT COUNT(*) FROM information_schema.columns a, \
             information_schema.columns b, information_schema.columns c",
            &[],
            &caps,
        )
        .await;

        assert!(result.is_err(), "Expected timeout error, got Ok");
        let err = result.unwrap_err();
        assert_eq!(
            err.error_code(),
            "QUERY_TIMEOUT",
            "Expected QUERY_TIMEOUT error code, got: {err:?}"
        );
        assert!(
            err.message().to_lowercase().contains("timeout"),
            "Expected message to mention timeout, got: {}",
            err.message()
        );
    }

    /// REF-258 Bug 4: `SELECT SLEEP(n)` swallows the server-side `MAX_EXECUTION_TIME`
    /// interrupt (returns `1` instead of erroring), so relying on the server alone
    /// reports partial/interrupted data as a success. The authoritative client-side
    /// timeout must turn this into a distinct `QUERY_TIMEOUT` error instead.
    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_timeout_sleep_reports_error_not_partial_success() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let caps = Capabilities { timeout_ms: Some(500), ..Capabilities::default() };
        let result = MySqlEngine::execute(&config, "SELECT SLEEP(3)", &[], &caps).await;

        assert!(result.is_err(), "SLEEP past the timeout must be an error, not a success");
        assert_eq!(
            result.unwrap_err().error_code(),
            "QUERY_TIMEOUT",
            "Expected QUERY_TIMEOUT for an interrupted SLEEP"
        );
    }

    /// REF-258 Bug 1: transaction-control statements fail under the binary
    /// prepared-statement protocol (error 1295). They must execute via the text
    /// protocol instead.
    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_transaction_control_executes() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let caps = Capabilities::default();
        for stmt in ["BEGIN", "START TRANSACTION", "COMMIT", "ROLLBACK"] {
            let result = MySqlEngine::execute(&config, stmt, &[], &caps).await;
            assert!(result.is_ok(), "{stmt} should execute via text protocol: {:?}", result.err());
        }
    }

    /// REF-258 Bug 2: `EXPLAIN` returns a result set whose rows were previously
    /// dropped (classified as a non-row statement). The plan rows must be captured.
    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_explain_captures_plan_rows() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        // Seed a table via a separate writable connection (Plenum's engine is read-only).
        let opts = build_mysql_opts(&config).expect("opts");
        let mut setup = Conn::new(opts).await.expect("connect");
        setup.exec_drop("DROP TABLE IF EXISTS ref258_explain", ()).await.expect("drop");
        setup
            .exec_drop("CREATE TABLE ref258_explain (id INT PRIMARY KEY)", ())
            .await
            .expect("create");
        setup.disconnect().await.ok();

        let caps = Capabilities::default();

        // Plain EXPLAIN must return the plan rows (non-empty result set).
        let result =
            MySqlEngine::execute(&config, "EXPLAIN SELECT id FROM ref258_explain", &[], &caps)
                .await
                .expect("EXPLAIN should succeed");
        assert!(!result.columns.is_empty(), "EXPLAIN must return columns");
        assert!(!result.rows.is_empty(), "EXPLAIN must return plan rows");
        assert!(result.rows_affected.is_none(), "EXPLAIN is a row-returning statement");

        // EXPLAIN FORMAT=JSON must also return its single plan row.
        let json_result = MySqlEngine::execute(
            &config,
            "EXPLAIN FORMAT=JSON SELECT id FROM ref258_explain",
            &[],
            &caps,
        )
        .await
        .expect("EXPLAIN FORMAT=JSON should succeed");
        assert!(!json_result.rows.is_empty(), "EXPLAIN FORMAT=JSON must return a plan row");

        // Cleanup.
        let opts2 = build_mysql_opts(&config).expect("opts2");
        let mut conn2 = Conn::new(opts2).await.expect("connect2");
        conn2.exec_drop("DROP TABLE IF EXISTS ref258_explain", ()).await.ok();
        conn2.disconnect().await.ok();
    }

    // -------------------------------------------------------------------------
    // REF-263: column comments + row estimates
    // -------------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "Requires running MySQL instance"]
    async fn test_introspect_column_comment_and_row_estimate_mysql() {
        let config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "root".to_string(),
            "password".to_string(),
            "test".to_string(),
        );

        let opts = build_mysql_opts(&config).expect("opts");
        let mut conn = Conn::new(opts).await.expect("connect");

        conn.exec_drop("DROP TABLE IF EXISTS ref263_mysql", ()).await.expect("drop");
        conn.exec_drop(
            "CREATE TABLE ref263_mysql (
                id INT PRIMARY KEY AUTO_INCREMENT COMMENT 'PK column',
                label VARCHAR(100) COMMENT 'Human readable name'
            ) COMMENT='REF-263 test table'",
            (),
        )
        .await
        .expect("create");

        conn.exec_drop("INSERT INTO ref263_mysql (label) VALUES ('a'), ('b'), ('c')", ())
            .await
            .expect("insert");

        conn.disconnect().await.ok();

        let result = MySqlEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "ref263_mysql".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            Some("test"),
        )
        .await
        .expect("introspect");

        let IntrospectResult::TableDetails { table } = result else {
            panic!("Expected TableDetails")
        };

        // Table comment
        assert_eq!(table.comment.as_deref(), Some("REF-263 test table"), "table comment mismatch");

        // row_estimate: MySQL's table_rows is an estimate and may be null for empty tables,
        // but for 3 rows it should be Some.
        assert!(table.row_estimate.is_some(), "row_estimate should be populated");

        // Column comments
        let id_col = table.columns.iter().find(|c| c.name == "id").expect("id col");
        assert_eq!(id_col.comment.as_deref(), Some("PK column"));

        let label_col = table.columns.iter().find(|c| c.name == "label").expect("label col");
        assert_eq!(label_col.comment.as_deref(), Some("Human readable name"));

        // Verify JSON has explicit nulls for columns without comments, and non-null for those with
        let json = serde_json::to_value(&table).expect("serialize");
        assert!(json.get("comment").is_some(), "table comment key must be present in JSON");
        assert!(json.get("row_estimate").is_some(), "row_estimate key must be present in JSON");

        // Cleanup
        let opts2 = build_mysql_opts(&config).expect("opts2");
        let mut conn2 = Conn::new(opts2).await.expect("connect2");
        conn2.exec_drop("DROP TABLE IF EXISTS ref263_mysql", ()).await.ok();
        conn2.disconnect().await.ok();
    }

    // -------------------------------------------------------------------------
    // REF-282: normalize_mysql_table_node / normalize_mysql_query_block unit tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_normalize_mysql_table_node_full_scan() {
        let table = serde_json::json!({
            "table_name": "users",
            "access_type": "ALL",
            "rows_examined_per_scan": 42.0,
            "cost_info": { "read_cost": "1.25" }
        });
        let result = normalize_mysql_table_node(&table);
        assert_eq!(result.node_type, "ALL");
        assert_eq!(result.relation, Some("users".to_string()));
        assert_eq!(result.estimated_rows, Some(42.0));
        assert_eq!(result.estimated_cost, Some(1.25));
        assert!(result.children.is_empty());
    }

    #[test]
    fn test_normalize_mysql_table_node_missing_optional_fields() {
        let table = serde_json::json!({ "table_name": "orders", "access_type": "ref" });
        let result = normalize_mysql_table_node(&table);
        assert_eq!(result.node_type, "ref");
        assert_eq!(result.relation, Some("orders".to_string()));
        assert_eq!(result.estimated_rows, None);
        assert_eq!(result.estimated_cost, None);
    }

    #[test]
    fn test_normalize_mysql_query_block_with_table() {
        let block = serde_json::json!({
            "cost_info": { "query_cost": "3.50" },
            "table": {
                "table_name": "items",
                "access_type": "ALL",
                "rows_examined_per_scan": 10.0
            }
        });
        let result = normalize_mysql_query_block(&block);
        assert_eq!(result.node_type, "query_block");
        assert_eq!(result.estimated_cost, Some(3.50));
        assert_eq!(result.children.len(), 1);
        assert_eq!(result.children[0].relation, Some("items".to_string()));
    }
}
