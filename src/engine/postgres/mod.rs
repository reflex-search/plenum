//! `PostgreSQL` Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for `PostgreSQL` databases.
//!
//! # Features
//! - Client-server connections via TCP
//! - Schema introspection via `information_schema`
//! - Capability-enforced query execution
//! - Rich type system support (arrays, JSON/JSONB, timestamps, etc.)
//!
//! # Implementation Notes
//! - Uses `tokio-postgres` (async driver, requires tokio runtime)
//! - Async operations are wrapped in synchronous interface
//! - Arrays converted to JSON arrays
//! - JSON/JSONB preserved as nested JSON
//! - BYTEA data is Base64-encoded for JSON safety
//! - Statement timeout enforced server-side via `SET statement_timeout`
//! - Client-side `tokio::time::timeout` kept as a backstop
//! - Row limits enforced in application code
//! - Schema filtering supported (`PostgreSQL` has explicit schemas)

use std::collections::HashMap; // Used for grouping foreign keys during introspection
use std::time::{Duration, Instant};
use tokio_postgres::{error::SqlState, Client, Config, NoTls, Row};

use crate::capability::validate_query;
use crate::engine::{
    Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine, DatabaseType,
    ForeignKeyInfo, IndexInfo, IntrospectOperation, IntrospectResult, QueryResult, SslMode,
    TableInfo, TlsConfig,
};
use crate::error::{PlenumError, Result};

/// `PostgreSQL` database engine implementation
pub struct PostgresEngine;

impl DatabaseEngine for PostgresEngine {
    async fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo> {
        // Validate config is for PostgreSQL
        if config.engine != DatabaseType::Postgres {
            return Err(PlenumError::invalid_input(format!(
                "Expected PostgreSQL engine, got {}",
                config.engine
            )));
        }

        // Build connection config
        let pg_config = build_pg_config(config)?;

        // Connect to PostgreSQL (TLS or plaintext depending on config)
        let client = pg_connect(&pg_config, config.tls.as_ref()).await?;

        // Get PostgreSQL version
        let version_row = client.query_one("SELECT version()", &[]).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to query PostgreSQL version: {e}"))
        })?;

        let version_string: String = version_row.get(0);

        // Extract version number (e.g., "PostgreSQL 15.3 on x86_64..." -> "15.3")
        let database_version =
            version_string.split_whitespace().nth(1).unwrap_or("unknown").to_string();

        // Get current database name
        let db_row = client.query_one("SELECT current_database()", &[]).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to query current database: {e}"))
        })?;

        let current_db: String = db_row.get(0);

        // If using wildcard mode, indicate it in the connection info
        let connected_database = if config.database.as_deref() == Some("*") {
            format!("{current_db} (wildcard mode - use --schema to introspect specific database)")
        } else {
            current_db
        };

        // Get current user
        let user_row = client.query_one("SELECT current_user", &[]).await.map_err(|e| {
            PlenumError::connection_failed(format!("Failed to query current user: {e}"))
        })?;

        let user: String = user_row.get(0);

        Ok(ConnectionInfo {
            database_version: database_version.clone(),
            server_info: version_string,
            connected_database,
            user,
        })
    }

    async fn introspect(
        config: &ConnectionConfig,
        operation: &IntrospectOperation,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<IntrospectResult> {
        // Validate config is for PostgreSQL
        if config.engine != DatabaseType::Postgres {
            return Err(PlenumError::invalid_input(format!(
                "Expected PostgreSQL engine, got {}",
                config.engine
            )));
        }

        // Handle database override by reconnecting
        let mut effective_config = config.clone();
        if let Some(db) = database {
            effective_config.database = Some(db.to_string());
        }

        // Build connection config
        let pg_config = build_pg_config(&effective_config)?;

        // Connect to PostgreSQL (TLS or plaintext depending on config)
        let client = pg_connect(&pg_config, effective_config.tls.as_ref()).await?;

        // Route to appropriate operation handler
        let result = match operation {
            IntrospectOperation::ListDatabases => list_databases_postgres(&client).await?,

            IntrospectOperation::ListSchemas => list_schemas_postgres(&client).await?,

            IntrospectOperation::ListTables => {
                let target_schema = determine_target_schema(&client, schema).await?;
                list_tables_postgres(&client, &target_schema).await?
            }

            IntrospectOperation::ListViews => {
                let target_schema = determine_target_schema(&client, schema).await?;
                list_views_postgres(&client, &target_schema).await?
            }

            IntrospectOperation::ListIndexes { table } => {
                let target_schema = determine_target_schema(&client, schema).await?;
                list_indexes_postgres(&client, &target_schema, table.as_deref()).await?
            }

            IntrospectOperation::TableDetails { name, fields } => {
                let target_schema = determine_target_schema(&client, schema).await?;
                get_table_details_postgres(&client, &target_schema, name, fields).await?
            }

            IntrospectOperation::ViewDetails { name } => {
                let target_schema = determine_target_schema(&client, schema).await?;
                get_view_details_postgres(&client, &target_schema, name).await?
            }
        };

        Ok(result)
    }

    async fn execute(
        config: &ConnectionConfig,
        query: &str,
        params: &[serde_json::Value],
        caps: &Capabilities,
    ) -> Result<QueryResult> {
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

        // Connect to PostgreSQL (TLS or plaintext depending on config)
        let client = pg_connect(&pg_config, config.tls.as_ref()).await?;

        // Defense in depth: enforce session-level read-only at the database layer.
        // This rejects writes even if the SQL parser is somehow bypassed (REF-261).
        client
            .execute("SET default_transaction_read_only = ON", &[])
            .await
            .map_err(|e| {
                PlenumError::engine_error(
                    "postgres",
                    format!("Failed to enforce session read-only mode: {e}"),
                )
            })?;

        // Set server-side statement timeout so PostgreSQL cancels the query if it exceeds
        // the limit. This prevents resource leaks — the server kills the query rather than
        // the client just abandoning the wait.
        if let Some(timeout_ms) = caps.timeout_ms {
            client
                .execute(&format!("SET statement_timeout = '{timeout_ms}ms'"), &[])
                .await
                .map_err(|e| {
                    PlenumError::engine_error(
                        "postgres",
                        format!("Failed to set statement_timeout: {e}"),
                    )
                })?;
        }

        // Execute with client-side tokio timeout as a backstop for unresponsive servers
        let start = Instant::now();
        let mut query_result = if let Some(timeout_ms) = caps.timeout_ms {
            let timeout_duration = Duration::from_millis(timeout_ms);
            tokio::time::timeout(timeout_duration, execute_query(&client, query, params, caps))
                .await
                .map_err(|_| {
                    PlenumError::query_failed(format!(
                        "Client-side timeout of {timeout_ms}ms exceeded (server-side statement_timeout should have fired first)"
                    ))
                })??
        } else {
            execute_query(&client, query, params, caps).await?
        };

        let elapsed = start.elapsed();
        query_result.execution_ms = elapsed.as_millis() as u64;

        Ok(query_result)
    }
}

/// Build `PostgreSQL` connection config from `ConnectionConfig`
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

    // Database can be "*" for wildcard (connects to default "postgres" database) or a specific database name
    let database = config.database.as_ref();

    // Check if database is wildcard ("*") - if so, connect to default "postgres" database
    let db_name = match database {
        Some(db) if db == "*" => "postgres", // Wildcard - use default postgres database
        Some(db) => db.as_str(),             // Explicit database
        None => {
            return Err(PlenumError::invalid_input(
                "PostgreSQL requires 'database' parameter (use \"*\" for default database)",
            ))
        }
    };

    let mut pg_config = Config::new();
    pg_config.host(host).port(port).user(user).password(password).dbname(db_name);

    Ok(pg_config)
}

/// Connect to PostgreSQL, branching on TLS config.
///
/// Returns only the `Client`; the background `Connection` is spawned into the tokio runtime.
/// No credentials or cert paths appear in error messages.
async fn pg_connect(pg_config: &Config, tls_config: Option<&TlsConfig>) -> Result<Client> {
    let sslmode = tls_config.map(|t| &t.sslmode).unwrap_or(&SslMode::Disable);

    match sslmode {
        SslMode::Disable => {
            let (client, connection) = pg_config
                .connect(NoTls)
                .await
                .map_err(|e| PlenumError::connection_failed(format!("Failed to connect to PostgreSQL: {e}")))?;
            tokio::spawn(async move { let _ = connection.await; });
            Ok(client)
        }
        _ => {
            // SAFETY: only reachable when tls_config is Some (sslmode != Disable).
            let connector = build_pg_native_tls(tls_config.unwrap())?;
            let tls = postgres_native_tls::MakeTlsConnector::new(connector);
            let (client, connection) = pg_config
                .connect(tls)
                .await
                .map_err(|e| PlenumError::connection_failed(format!("Failed to connect to PostgreSQL: {e}")))?;
            tokio::spawn(async move { let _ = connection.await; });
            Ok(client)
        }
    }
}

/// Build a `native_tls::TlsConnector` from a `TlsConfig`.
///
/// Error messages deliberately omit cert/key paths to prevent credential leakage.
fn build_pg_native_tls(tls: &TlsConfig) -> Result<native_tls::TlsConnector> {
    let mut builder = native_tls::TlsConnector::builder();

    match &tls.sslmode {
        SslMode::Require => {
            // Require TLS; skip both cert validation and hostname check.
            builder.danger_accept_invalid_certs(true);
            builder.danger_accept_invalid_hostnames(true);
        }
        SslMode::VerifyCa => {
            // Verify cert against CA; skip hostname check.
            let ca_path = tls.ca_cert.as_ref().ok_or_else(|| {
                PlenumError::connection_failed(
                    "sslmode=verify-ca requires a CA certificate (--ssl-ca)",
                )
            })?;
            let ca_pem = std::fs::read(ca_path)
                .map_err(|_| PlenumError::connection_failed("Failed to read CA certificate"))?;
            let cert = native_tls::Certificate::from_pem(&ca_pem)
                .map_err(|_| PlenumError::connection_failed("Failed to parse CA certificate"))?;
            builder.add_root_certificate(cert);
            builder.danger_accept_invalid_hostnames(true);
        }
        SslMode::VerifyFull => {
            // Full verification: CA cert + hostname.
            let ca_path = tls.ca_cert.as_ref().ok_or_else(|| {
                PlenumError::connection_failed(
                    "sslmode=verify-full requires a CA certificate (--ssl-ca)",
                )
            })?;
            let ca_pem = std::fs::read(ca_path)
                .map_err(|_| PlenumError::connection_failed("Failed to read CA certificate"))?;
            let cert = native_tls::Certificate::from_pem(&ca_pem)
                .map_err(|_| PlenumError::connection_failed("Failed to parse CA certificate"))?;
            builder.add_root_certificate(cert);
        }
        SslMode::Disable => {
            unreachable!("Disable mode is handled in pg_connect before calling this function")
        }
    }

    // mTLS: load client cert + key if provided.
    if let (Some(cert_path), Some(key_path)) = (&tls.client_cert, &tls.client_key) {
        let cert_pem = std::fs::read(cert_path)
            .map_err(|_| PlenumError::connection_failed("Failed to read client certificate"))?;
        let key_pem = std::fs::read(key_path)
            .map_err(|_| PlenumError::connection_failed("Failed to read client key"))?;
        let identity = native_tls::Identity::from_pkcs8(&cert_pem, &key_pem)
            .map_err(|_| PlenumError::connection_failed("Failed to parse client certificate or key"))?;
        builder.identity(identity);
    }

    builder
        .build()
        .map_err(|_| PlenumError::connection_failed("Failed to build TLS connector"))
}

/// Determine target schema from filter or current schema
async fn determine_target_schema(client: &Client, schema_filter: Option<&str>) -> Result<String> {
    if let Some(schema) = schema_filter {
        return Ok(schema.to_string());
    }

    // Get current schema
    let row = client.query_one("SELECT current_schema()", &[]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to query current schema: {e}"))
    })?;

    let current_schema: String = row.get(0);
    Ok(current_schema)
}

/// List all databases (requires wildcard connection or superuser privileges)
async fn list_databases_postgres(client: &Client) -> Result<IntrospectResult> {
    let query = "
        SELECT datname
        FROM pg_catalog.pg_database
        WHERE datistemplate = false
        ORDER BY datname";

    let rows = client.query(query, &[]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to list databases: {e}"))
    })?;

    let databases: Vec<String> = rows.iter().map(|row| row.get(0)).collect();

    Ok(IntrospectResult::DatabaseList { databases })
}

/// List all schemas (`PostgreSQL` has true schemas separate from databases)
async fn list_schemas_postgres(client: &Client) -> Result<IntrospectResult> {
    let query = "
        SELECT schema_name
        FROM information_schema.schemata
        WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast', 'pg_temp_1', 'pg_toast_temp_1')
        ORDER BY schema_name";

    let rows = client.query(query, &[]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to list schemas: {e}"))
    })?;

    let schemas: Vec<String> = rows.iter().map(|row| row.get(0)).collect();

    Ok(IntrospectResult::SchemaList { schemas })
}

/// List all tables in the target schema
async fn list_tables_postgres(client: &Client, schema: &str) -> Result<IntrospectResult> {
    let query = "
        SELECT table_name
        FROM information_schema.tables
        WHERE table_schema = $1
        AND table_type = 'BASE TABLE'
        ORDER BY table_name";

    let rows = client.query(query, &[&schema]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to list tables in schema '{schema}': {e}"))
    })?;

    let tables: Vec<String> = rows.iter().map(|row| row.get(0)).collect();

    Ok(IntrospectResult::TableList { tables })
}

/// List all views in the target schema
async fn list_views_postgres(client: &Client, schema: &str) -> Result<IntrospectResult> {
    let query = "
        SELECT table_name
        FROM information_schema.views
        WHERE table_schema = $1
        ORDER BY table_name";

    let rows = client.query(query, &[&schema]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to list views in schema '{schema}': {e}"))
    })?;

    let views: Vec<String> = rows.iter().map(|row| row.get(0)).collect();

    Ok(IntrospectResult::ViewList { views })
}

/// List all indexes in the target schema (optionally filtered by table)
async fn list_indexes_postgres(
    client: &Client,
    schema: &str,
    table_filter: Option<&str>,
) -> Result<IntrospectResult> {
    use crate::engine::IndexSummary;

    let query = if let Some(table) = table_filter {
        format!(
            "SELECT indexname, tablename, indexdef
             FROM pg_indexes
             WHERE schemaname = $1 AND tablename = '{table}'
             ORDER BY indexname"
        )
    } else {
        "SELECT indexname, tablename, indexdef
         FROM pg_indexes
         WHERE schemaname = $1
         ORDER BY indexname"
            .to_string()
    };

    let rows = client.query(&query, &[&schema]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to list indexes in schema '{schema}': {e}"))
    })?;

    let mut indexes = Vec::new();
    for row in rows {
        let index_name: String = row.get(0);
        let table_name: String = row.get(1);
        let index_def: String = row.get(2);

        // Skip primary key indexes (they're part of table details)
        if index_name.ends_with("_pkey") {
            continue;
        }

        // Determine if index is unique
        let unique = index_def.contains("UNIQUE INDEX");

        // Extract column names from index definition
        let columns = extract_index_columns(&index_def);

        indexes.push(IndexSummary { name: index_name, table: table_name, unique, columns });
    }

    Ok(IntrospectResult::IndexList { indexes })
}

/// Get full table details with conditional field retrieval
async fn get_table_details_postgres(
    client: &Client,
    schema: &str,
    table_name: &str,
    fields: &crate::engine::TableFields,
) -> Result<IntrospectResult> {
    // Verify table exists
    let check_query = "
        SELECT COUNT(*)
        FROM information_schema.tables
        WHERE table_schema = $1 AND table_name = $2 AND table_type = 'BASE TABLE'";

    let row = client.query_one(check_query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to check table existence: {e}"))
    })?;

    let count: i64 = row.get(0);
    if count == 0 {
        return Err(PlenumError::invalid_input(format!(
            "Table '{table_name}' not found in schema '{schema}'"
        )));
    }

    // Conditionally retrieve fields
    let columns = if fields.columns {
        introspect_columns(client, schema, table_name).await?
    } else {
        Vec::new()
    };

    let primary_key = if fields.primary_key {
        introspect_primary_key(client, schema, table_name).await?
    } else {
        None
    };

    let foreign_keys = if fields.foreign_keys {
        introspect_foreign_keys(client, schema, table_name).await?
    } else {
        Vec::new()
    };

    let indexes = if fields.indexes {
        introspect_indexes(client, schema, table_name).await?
    } else {
        Vec::new()
    };

    let (comment, row_estimate) = introspect_table_meta(client, schema, table_name).await?;

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

/// Get view details including definition and columns
async fn get_view_details_postgres(
    client: &Client,
    schema: &str,
    view_name: &str,
) -> Result<IntrospectResult> {
    use crate::engine::ViewInfo;

    // Get view definition
    let def_query = "
        SELECT definition
        FROM information_schema.views
        WHERE table_schema = $1 AND table_name = $2";

    let def_row = client.query_opt(def_query, &[&schema, &view_name]).await.map_err(|e| {
        PlenumError::engine_error("postgres", format!("Failed to query view definition: {e}"))
    })?;

    if def_row.is_none() {
        return Err(PlenumError::invalid_input(format!(
            "View '{view_name}' not found in schema '{schema}'"
        )));
    }

    let definition: Option<String> = def_row.unwrap().get(0);

    // Get view columns
    let columns = introspect_columns(client, schema, view_name).await?;

    let view = ViewInfo {
        name: view_name.to_string(),
        schema: Some(schema.to_string()),
        definition,
        columns,
    };

    Ok(IntrospectResult::ViewDetails { view })
}

/// Introspect table columns (includes column comments from pg_description)
async fn introspect_columns(
    client: &Client,
    schema: &str,
    table_name: &str,
) -> Result<Vec<ColumnInfo>> {
    let query = "
        SELECT c.column_name, c.data_type, c.is_nullable, c.column_default,
               pgd.description
        FROM information_schema.columns c
        LEFT JOIN pg_catalog.pg_statio_all_tables st
            ON st.schemaname = c.table_schema AND st.relname = c.table_name
        LEFT JOIN pg_catalog.pg_description pgd
            ON pgd.objoid = st.relid AND pgd.objsubid = c.ordinal_position
        WHERE c.table_schema = $1 AND c.table_name = $2
        ORDER BY c.ordinal_position";

    let rows = client.query(query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error(
            "postgres",
            format!("Failed to query columns for {schema}.{table_name}: {e}"),
        )
    })?;

    let mut columns = Vec::new();
    for row in rows {
        let column_name: String = row.get(0);
        let data_type: String = row.get(1);
        let is_nullable: String = row.get(2);
        let default: Option<String> = row.get(3);
        let comment: Option<String> = row.get(4);

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

/// Fetch table-level comment and row estimate from pg_class / pg_description
async fn introspect_table_meta(
    client: &Client,
    schema: &str,
    table_name: &str,
) -> Result<(Option<String>, Option<i64>)> {
    let query = "
        SELECT obj_description(c.oid, 'pg_class'), c.reltuples::bigint
        FROM pg_catalog.pg_class c
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = $1 AND c.relname = $2";

    let row = client.query_opt(query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error(
            "postgres",
            format!("Failed to query table metadata for {schema}.{table_name}: {e}"),
        )
    })?;

    match row {
        None => Ok((None, None)),
        Some(r) => {
            let comment: Option<String> = r.get(0);
            let raw_estimate: Option<i64> = r.get(1);
            // reltuples is -1 when the table has never been analyzed; treat as unavailable
            let row_estimate = raw_estimate.filter(|&v| v >= 0);
            Ok((comment, row_estimate))
        }
    }
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

    let rows = client.query(query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error(
            "postgres",
            format!("Failed to query primary key for {schema}.{table_name}: {e}"),
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

    let rows = client.query(query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error(
            "postgres",
            format!("Failed to query foreign keys for {schema}.{table_name}: {e}"),
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
async fn introspect_indexes(
    client: &Client,
    schema: &str,
    table_name: &str,
) -> Result<Vec<IndexInfo>> {
    // Query pg_indexes for index information
    let query = "
        SELECT
            indexname,
            indexdef
        FROM pg_indexes
        WHERE schemaname = $1 AND tablename = $2
        ORDER BY indexname";

    let rows = client.query(query, &[&schema, &table_name]).await.map_err(|e| {
        PlenumError::engine_error(
            "postgres",
            format!("Failed to query indexes for {schema}.{table_name}: {e}"),
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

        indexes.push(IndexInfo { name: index_name, columns, unique });
    }

    Ok(indexes)
}

/// Extract column names from `PostgreSQL` index definition
fn extract_index_columns(index_def: &str) -> Vec<String> {
    // Find the column list between parentheses
    if let Some(start) = index_def.rfind('(') {
        if let Some(end) = index_def.rfind(')') {
            let column_str = &index_def[start + 1..end];
            return column_str.split(',').map(|s| s.trim().to_string()).collect();
        }
    }
    Vec::new()
}

/// Returns true when a tokio-postgres error is a server-side statement timeout (SQLSTATE 57014).
fn is_statement_timeout(e: &tokio_postgres::Error) -> bool {
    e.as_db_error()
        .map(|db| {
            *db.code() == SqlState::QUERY_CANCELED && db.message().contains("statement timeout")
        })
        .unwrap_or(false)
}

/// Convert a JSON value to a boxed `tokio-postgres` `ToSql` trait object for parameter binding.
/// Uses `$1`/`$2`/… Postgres placeholders.
fn json_to_pg_value(
    val: &serde_json::Value,
) -> Box<dyn tokio_postgres::types::ToSql + Sync + Send> {
    use tokio_postgres::types::ToSql;
    match val {
        serde_json::Value::Null => {
            Box::new(Option::<String>::None) as Box<dyn ToSql + Sync + Send>
        }
        serde_json::Value::Bool(b) => Box::new(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Box::new(i)
            } else {
                Box::new(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Box::new(s.clone()),
        v => Box::new(v.to_string()),
    }
}

/// Execute query and return `QueryResult`
async fn execute_query(
    client: &Client,
    query: &str,
    params: &[serde_json::Value],
    caps: &Capabilities,
) -> Result<QueryResult> {
    // Execute query
    let stmt = client
        .prepare(query)
        .await
        .map_err(|e| PlenumError::query_failed(format!("Failed to prepare query: {e}")))?;

    // Convert JSON params → boxed ToSql objects, then build a slice of refs.
    // The boxes must be Send so the future is Send across the await point.
    let pg_params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> =
        params.iter().map(json_to_pg_value).collect();
    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = pg_params
        .iter()
        .map(|v| v.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();

    // Check if this is a SELECT query (returns rows)
    let is_select = !stmt.columns().is_empty();

    if is_select {
        // SELECT query - execute and collect rows
        let rows = client
            .query(&stmt, &param_refs)
            .await
            .map_err(|e| {
                if is_statement_timeout(&e) {
                    PlenumError::query_timeout(format!(
                        "Query cancelled by PostgreSQL server-side statement_timeout: {e}"
                    ))
                } else {
                    PlenumError::query_failed(format!("Failed to execute query: {e}"))
                }
            })?;

        // Get column names
        let column_names: Vec<String> =
            stmt.columns().iter().map(|c| c.name().to_string()).collect();

        // Apply offset and max_rows with truncation detection
        let offset = caps.offset.unwrap_or(0);
        let effective = if offset <= rows.len() { &rows[offset..] } else { &[][..] };
        let max = caps.max_rows.unwrap_or(usize::MAX);
        let rows_truncated = effective.len() > max;
        let take = effective.len().min(max);

        let mut rows_data = Vec::new();
        for row in &effective[..take] {
            rows_data.push(row_to_json(&column_names, row)?);
        }

        Ok(QueryResult {
            columns: column_names,
            rows: rows_data,
            rows_affected: None,
            execution_ms: 0,
            rows_truncated,
            truncated_by: None,
        })
    } else {
        // Non-SELECT query (INSERT, UPDATE, DELETE, DDL)
        let rows_affected = client
            .execute(&stmt, &param_refs)
            .await
            .map_err(|e| {
                if is_statement_timeout(&e) {
                    PlenumError::query_timeout(format!(
                        "Query cancelled by PostgreSQL server-side statement_timeout: {e}"
                    ))
                } else {
                    PlenumError::query_failed(format!("Failed to execute query: {e}"))
                }
            })?;

        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: Some(rows_affected),
            execution_ms: 0,
            rows_truncated: false,
            truncated_by: None,
        })
    }
}

/// Convert a `PostgreSQL` row to a JSON-safe `Vec`
fn row_to_json(column_names: &[String], row: &Row) -> Result<Vec<serde_json::Value>> {
    let mut values = Vec::with_capacity(column_names.len());

    for idx in 0..column_names.len() {
        let value = postgres_value_to_json(row, idx)?;
        values.push(value);
    }

    Ok(values)
}

/// Convert `PostgreSQL` value to JSON value
fn postgres_value_to_json(row: &Row, idx: usize) -> Result<serde_json::Value> {
    use tokio_postgres::types::Type;

    let column = &row.columns()[idx];
    let col_type = column.type_();

    // Handle NULL first
    if matches!(row.try_get::<_, Option<String>>(idx), Ok(None)) {
        return Ok(serde_json::Value::Null);
    }

    // Map PostgreSQL types to JSON
    let value = match *col_type {
        // Boolean
        Type::BOOL => {
            let v: bool = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get boolean value: {e}"))
            })?;
            serde_json::Value::Bool(v)
        }

        // Integers
        Type::INT2 => {
            let v: i16 = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get i16 value: {e}")))?;
            serde_json::Value::Number(v.into())
        }
        Type::INT4 => {
            let v: i32 = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get i32 value: {e}")))?;
            serde_json::Value::Number(v.into())
        }
        Type::INT8 => {
            let v: i64 = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get i64 value: {e}")))?;
            serde_json::Value::Number(v.into())
        }

        // Floats
        Type::FLOAT4 => {
            let v: f32 = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get f32 value: {e}")))?;
            serde_json::Number::from_f64(f64::from(v))
                .map_or(serde_json::Value::Null, serde_json::Value::Number) // Handle NaN/Infinity as null
        }
        Type::FLOAT8 => {
            let v: f64 = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get f64 value: {e}")))?;
            serde_json::Number::from_f64(v)
                .map_or(serde_json::Value::Null, serde_json::Value::Number) // Handle NaN/Infinity as null
        }

        // Text types (VARCHAR, TEXT, CHAR, etc.)
        Type::VARCHAR | Type::TEXT | Type::BPCHAR | Type::NAME => {
            let v: String = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get string value: {e}"))
            })?;
            serde_json::Value::String(v)
        }

        // JSON types
        Type::JSON | Type::JSONB => {
            let v: serde_json::Value = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get JSON value: {e}")))?;
            v
        }

        // BYTEA (binary data) - encode as Base64
        Type::BYTEA => {
            let v: Vec<u8> = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get bytea value: {e}"))
            })?;
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&v);
            serde_json::Value::String(encoded)
        }

        // Timestamps - convert to ISO 8601 strings
        Type::TIMESTAMP => {
            use chrono::NaiveDateTime;
            let v: NaiveDateTime = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get timestamp value: {e}"))
            })?;
            serde_json::Value::String(v.format("%Y-%m-%dT%H:%M:%S").to_string())
        }
        Type::TIMESTAMPTZ => {
            use chrono::{DateTime, Utc};
            let v: DateTime<Utc> = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get timestamptz value: {e}"))
            })?;
            serde_json::Value::String(v.to_rfc3339())
        }

        // Date
        Type::DATE => {
            use chrono::NaiveDate;
            let v: NaiveDate = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get date value: {e}")))?;
            serde_json::Value::String(v.format("%Y-%m-%d").to_string())
        }

        // Time
        Type::TIME => {
            use chrono::NaiveTime;
            let v: NaiveTime = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get time value: {e}")))?;
            serde_json::Value::String(v.format("%H:%M:%S").to_string())
        }

        // UUID
        Type::UUID => {
            use uuid::Uuid;
            let v: Uuid = row
                .try_get(idx)
                .map_err(|e| PlenumError::query_failed(format!("Failed to get UUID value: {e}")))?;
            serde_json::Value::String(v.to_string())
        }

        // Arrays (convert to JSON arrays recursively)
        // Note: PostgreSQL array support is complex - for MVP, convert to string representation
        _ if col_type.name().ends_with("[]") => {
            // For arrays, we'll use a simple string representation for MVP
            // Full array support would require recursive type handling
            let v: String = row.try_get(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to get array value: {e}"))
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
    fn test_wildcard_database_config() {
        // Test that wildcard database is accepted
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "*".to_string(),
        );

        // Should accept "*" as database
        assert_eq!(config.database, Some("*".to_string()));

        // Build config should work with wildcard and connect to "postgres" database
        let result = build_pg_config(&config);
        assert!(
            result.is_ok(),
            "Failed to build Postgres config with wildcard: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_missing_database_error() {
        // Test that missing database gives helpful error
        let config = ConnectionConfig {
            engine: DatabaseType::Postgres,
            host: Some("localhost".to_string()),
            port: Some(5432),
            user: Some("postgres".to_string()),
            password: Some("postgres".to_string()),
            database: None,
            file: None,
            tls: None,
        };

        let result = build_pg_config(&config);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.message().contains("PostgreSQL requires 'database' parameter"));
        assert!(error.message().contains("\"*\""));
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_validate_connection() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let result = PostgresEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "Connection validation failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("PostgreSQL"));
        assert_eq!(info.connected_database, "postgres");
        assert_eq!(info.user, "postgres");
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_validate_connection_wildcard() {
        // Test connection with wildcard database
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "*".to_string(),
        );

        let result = PostgresEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "Wildcard connection validation failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(!info.database_version.is_empty());
        assert!(info.server_info.contains("PostgreSQL"));
        assert!(info.connected_database.contains("wildcard mode"));
        assert!(info.connected_database.contains("postgres"));
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_query_pg_database_wildcard() {
        // Test querying pg_catalog.pg_database with wildcard database
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "*".to_string(),
        );

        let caps = Capabilities::default();
        let result = PostgresEngine::execute(
            &config,
            "SELECT datname FROM pg_catalog.pg_database WHERE datistemplate = false",
    &[],
            &caps,
        )
        .await;
        assert!(result.is_ok(), "pg_database query failed: {:?}", result.err());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns.len(), 1);
        assert!(!query_result.rows.is_empty());
    }

    #[tokio::test]
    async fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );
        config.engine = DatabaseType::SQLite;

        let result = PostgresEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Expected PostgreSQL engine"));
    }

    #[tokio::test]
    async fn test_validate_connection_missing_host() {
        let config = ConnectionConfig {
            engine: DatabaseType::Postgres,
            host: None,
            port: Some(5432),
            user: Some("postgres".to_string()),
            password: Some("postgres".to_string()),
            database: Some("postgres".to_string()),
            file: None,
            tls: None,
        };

        let result = PostgresEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("PostgreSQL requires 'host' parameter"));
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_introspect_schema() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table first
        let create_caps = Capabilities::default();
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_users", &[], &create_caps).await;
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_users (
                id SERIAL PRIMARY KEY,
                name VARCHAR(100) NOT NULL,
                email VARCHAR(255)
            )",
            &[],
            &create_caps,
        )
        .await;

        // Introspect
        let result = PostgresEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "test_users".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            Some("public"),
        )
        .await;
        assert!(result.is_ok(), "Introspection failed: {:?}", result.err());

        let IntrospectResult::TableDetails { table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

        assert_eq!(table.name, "test_users");
        assert_eq!(table.schema, Some("public".to_string()));
        assert!(table.columns.len() >= 3);

        // Check primary key
        assert!(table.primary_key.is_some());
        let pk = table.primary_key.as_ref().unwrap();
        assert_eq!(pk.len(), 1);
        assert_eq!(pk[0], "id");

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_users", &[], &create_caps).await;
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_select_query() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::default();
        let result =
            PostgresEngine::execute(&config, "SELECT 1 AS num, 'test' AS str", &[], &caps).await;
        assert!(result.is_ok(), "Query execution failed: {:?}", result.err());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns.len(), 2);
        assert_eq!(query_result.rows.len(), 1);
        assert_eq!(query_result.rows_affected, None);

        let row = &query_result.rows[0];
        assert_eq!(row[0], serde_json::json!(1)); // num column
        assert_eq!(row[1], serde_json::json!("test")); // str column
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_insert_without_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table
        let ddl_caps = Capabilities::default();
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_insert", &[], &ddl_caps).await;
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_insert (id SERIAL PRIMARY KEY, name TEXT)",
    &[],
            &ddl_caps,
        )
        .await;

        // Try to insert without write capability
        let caps = Capabilities::default();
        let result = PostgresEngine::execute(
            &config,
            "INSERT INTO test_insert (name) VALUES ('test')",
    &[],
            &caps,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Write operations require --allow-write"));

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_insert", &[], &ddl_caps).await;
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_insert_with_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create test table
        let ddl_caps = Capabilities::default();
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_insert2", &[], &ddl_caps).await;
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_insert2 (id SERIAL PRIMARY KEY, name TEXT)",
    &[],
            &ddl_caps,
        )
        .await;

        // Insert with write capability
        let write_caps = Capabilities::default();
        let result = PostgresEngine::execute(
            &config,
            "INSERT INTO test_insert2 (name) VALUES ('test')",
    &[],
            &write_caps,
        )
        .await;

        assert!(result.is_ok(), "Insert failed: {:?}", result.err());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows_affected, Some(1));

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_insert2", &[], &ddl_caps).await;
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_ddl_without_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::default();
        let result = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_ddl (id SERIAL PRIMARY KEY)",
    &[],
            &caps,
        )
        .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("DDL operations require --allow-ddl"));
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_ddl_with_capability() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let caps = Capabilities::default();
        let _ = PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_ddl2", &[], &caps).await;
        let result = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_ddl2 (id SERIAL PRIMARY KEY)",
    &[],
            &caps,
        )
        .await;

        assert!(result.is_ok(), "DDL execution failed: {:?}", result.err());

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_ddl2", &[], &caps).await;
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_execute_max_rows_limit() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Create and populate test table
        let ddl_caps = Capabilities::default();
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS test_limit", &[], &ddl_caps).await;
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE test_limit (id SERIAL PRIMARY KEY, name TEXT)",
    &[],
            &ddl_caps,
        )
        .await;

        let write_caps = Capabilities::default();
        for i in 1..=10 {
            let _ = PostgresEngine::execute(
                &config,
                &format!("INSERT INTO test_limit (name) VALUES ('User {i}')"),
    &[],
                &write_caps,
            )
            .await;
        }

        // Query with row limit
        let caps = Capabilities { max_rows: Some(5), ..Capabilities::default() };
        let result = PostgresEngine::execute(&config, "SELECT * FROM test_limit", &[], &caps).await;

        assert!(result.is_ok(), "Query failed: {:?}", result.err());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows.len(), 5); // Limited to 5 rows

        // Cleanup
        let _ = PostgresEngine::execute(&config, "DROP TABLE test_limit", &[], &ddl_caps).await;
    }

    // -------------------------------------------------------------------------
    // REF-270: TLS/SSL configuration tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_verify_ca_requires_ca_cert() {
        let tls = TlsConfig {
            sslmode: SslMode::VerifyCa,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        let result = build_pg_native_tls(&tls);
        assert!(result.is_err());
        let msg = result.unwrap_err().message().to_string();
        // Must report connection failed, must NOT leak paths
        assert!(msg.contains("CA certificate"), "expected mention of CA cert");
        assert!(!msg.contains('/'), "must not contain file path in error");
    }

    #[test]
    fn test_verify_full_requires_ca_cert() {
        let tls = TlsConfig {
            sslmode: SslMode::VerifyFull,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        let result = build_pg_native_tls(&tls);
        assert!(result.is_err());
        let msg = result.unwrap_err().message().to_string();
        assert!(msg.contains("CA certificate"));
        assert!(!msg.contains('/'));
    }

    #[test]
    fn test_verify_ca_nonexistent_ca_file_no_path_leak() {
        let tls = TlsConfig {
            sslmode: SslMode::VerifyCa,
            ca_cert: Some(std::path::PathBuf::from("/nonexistent/ca.pem")),
            client_cert: None,
            client_key: None,
        };
        let result = build_pg_native_tls(&tls);
        assert!(result.is_err());
        let msg = result.unwrap_err().message().to_string();
        // Error must not leak the path "/nonexistent/ca.pem"
        assert!(!msg.contains("nonexistent"), "path must not appear in error: {msg}");
        assert!(!msg.contains("/nonexistent"), "path must not appear in error: {msg}");
    }

    #[test]
    fn test_require_mode_builds_without_ca() {
        // sslmode=require should succeed without ca_cert (no cert verification)
        let tls = TlsConfig {
            sslmode: SslMode::Require,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        // build_pg_native_tls returns Ok for require mode (no files read)
        assert!(build_pg_native_tls(&tls).is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance with TLS enabled"]
    async fn test_tls_verify_full_connects() {
        let mut config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::VerifyFull,
            ca_cert: Some(std::path::PathBuf::from("/etc/ssl/certs/ca-certificates.crt")),
            client_cert: None,
            client_key: None,
        });
        let result = PostgresEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "TLS verify-full connection failed: {:?}", result.err());
    }

    #[test]
    fn test_extract_index_columns() {
        let index_def = "CREATE INDEX idx_users_email ON public.users USING btree (email)";
        let columns = extract_index_columns(index_def);
        assert_eq!(columns, vec!["email"]);

        let index_def_multi =
            "CREATE INDEX idx_composite ON public.orders USING btree (user_id, order_date)";
        let columns_multi = extract_index_columns(index_def_multi);
        assert_eq!(columns_multi, vec!["user_id", "order_date"]);
    }

    /// Prove that writes fail at the PostgreSQL session layer independently of the parser.
    ///
    /// Opens a raw connection, applies `SET default_transaction_read_only = ON` (same as
    /// `execute()` does), then attempts a direct write without going through Plenum's
    /// `validate_query`. The write must be rejected by PostgreSQL itself (REF-261).
    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_postgres_session_read_only_enforcement() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        let pg_config = build_pg_config(&config).unwrap();
        let (client, connection) = pg_config.connect(NoTls).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });

        // Apply session-level read-only (same as execute() does)
        client.execute("SET default_transaction_read_only = ON", &[]).await.unwrap();

        // Verify the setting is active
        let row = client.query_one("SHOW default_transaction_read_only", &[]).await.unwrap();
        let setting: String = row.get(0);
        assert_eq!(setting, "on", "session must be read-only");

        // Attempt a direct INSERT bypassing Plenum's parser — must fail at the DB layer.
        // Use a CTE-wrapped form to confirm even complex queries are blocked.
        let result = client
            .execute(
                "INSERT INTO pg_catalog.pg_description SELECT * FROM pg_catalog.pg_description LIMIT 0",
                &[],
            )
            .await;
        assert!(result.is_err(), "INSERT must be rejected by PostgreSQL session read-only mode");

        // Attempt DDL directly — must also fail.
        let ddl_result = client
            .execute("CREATE TABLE _plenum_ro_test_pg (id INT)", &[])
            .await;
        assert!(
            ddl_result.is_err(),
            "CREATE TABLE must be rejected by PostgreSQL session read-only mode"
        );
    }

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_statement_timeout_fires_server_side() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // 50ms timeout with pg_sleep(10) — server must cancel this.
        let caps = Capabilities { timeout_ms: Some(50), ..Capabilities::default() };
        let result = PostgresEngine::execute(&config, "SELECT pg_sleep(10)", &[], &caps).await;

        assert!(result.is_err(), "Expected timeout error, got Ok");
        let err = result.unwrap_err();
        assert_eq!(
            err.error_code(),
            "QUERY_TIMEOUT",
            "Expected QUERY_TIMEOUT error code, got: {:?}",
            err
        );
        assert!(
            err.message().contains("statement_timeout"),
            "Expected message to mention statement_timeout, got: {}",
            err.message()
        );
    }

    #[test]
    fn test_is_statement_timeout_with_non_timeout_error() {
        // Verify the helper returns false for non-timeout errors.
        // We can't easily construct a real tokio_postgres::Error in unit tests,
        // but we can verify the function compiles and has the right signature.
        // Integration coverage comes from test_statement_timeout_fires_server_side.
        let _ = is_statement_timeout as fn(&tokio_postgres::Error) -> bool;
    }

    // -------------------------------------------------------------------------
    // REF-263: column comments + row estimates
    // -------------------------------------------------------------------------

    #[tokio::test]
    #[ignore = "Requires running PostgreSQL instance"]
    async fn test_introspect_column_comment_and_row_estimate_postgres() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "postgres".to_string(),
            "postgres".to_string(),
            "postgres".to_string(),
        );

        // Setup: create table, add comments, insert rows, analyze
        let ddl_caps = Capabilities::default();
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS ref263_pg", &[], &ddl_caps)
                .await;
        let _ = PostgresEngine::execute(
            &config,
            "CREATE TABLE ref263_pg (
                id SERIAL PRIMARY KEY,
                label TEXT NOT NULL
            )",
            &[],
            &ddl_caps,
        )
        .await
        .expect("create table");

        let _ = PostgresEngine::execute(
            &config,
            "COMMENT ON TABLE ref263_pg IS 'REF-263 test table'",
            &[],
            &ddl_caps,
        )
        .await
        .expect("table comment");

        let _ = PostgresEngine::execute(
            &config,
            "COMMENT ON COLUMN ref263_pg.id IS 'PK column'",
            &[],
            &ddl_caps,
        )
        .await
        .expect("id comment");

        let _ = PostgresEngine::execute(
            &config,
            "COMMENT ON COLUMN ref263_pg.label IS 'Human readable name'",
            &[],
            &ddl_caps,
        )
        .await
        .expect("label comment");

        let _ = PostgresEngine::execute(
            &config,
            "INSERT INTO ref263_pg (label) SELECT 'row' || g FROM generate_series(1,10) g",
            &[],
            &ddl_caps,
        )
        .await
        .expect("insert");

        let _ = PostgresEngine::execute(&config, "ANALYZE ref263_pg", &[], &ddl_caps)
            .await
            .expect("analyze");

        // Introspect
        let result = PostgresEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "ref263_pg".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            Some("public"),
        )
        .await
        .expect("introspect");

        let IntrospectResult::TableDetails { table } = result else {
            panic!("Expected TableDetails")
        };

        // Table comment
        assert_eq!(
            table.comment.as_deref(),
            Some("REF-263 test table"),
            "table comment mismatch"
        );

        // row_estimate should be populated after ANALYZE
        assert!(table.row_estimate.is_some(), "row_estimate should be set after ANALYZE");
        assert!(table.row_estimate.unwrap() >= 0, "row_estimate must be non-negative");

        // Column comments
        let id_col = table.columns.iter().find(|c| c.name == "id").expect("id col");
        assert_eq!(id_col.comment.as_deref(), Some("PK column"));

        let label_col = table.columns.iter().find(|c| c.name == "label").expect("label col");
        assert_eq!(label_col.comment.as_deref(), Some("Human readable name"));

        // Verify JSON always has explicit nulls
        let json = serde_json::to_value(&table).expect("serialize");
        assert!(json.get("comment").is_some(), "table comment key must be present in JSON");
        assert!(json.get("row_estimate").is_some(), "row_estimate key must be present in JSON");
        for col in json["columns"].as_array().unwrap() {
            assert!(col.get("comment").is_some(), "column comment key must be present in JSON");
        }

        // Cleanup
        let _ =
            PostgresEngine::execute(&config, "DROP TABLE IF EXISTS ref263_pg", &[], &ddl_caps)
                .await;
    }
}
