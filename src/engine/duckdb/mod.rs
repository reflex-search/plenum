//! `DuckDB` Database Engine Implementation
//!
//! This module implements the `DatabaseEngine` trait for `DuckDB` databases.
//!
//! # Features
//! - File-based connections (`/path/to/db.duckdb`)
//! - In-memory connections (`:memory:`)
//! - Schema introspection via `DuckDB` catalog functions (`duckdb_tables()`,
//!   `duckdb_views()`, `duckdb_columns()`, `duckdb_constraints()`, `duckdb_indexes()`)
//! - Capability-enforced query execution
//!
//! # Implementation Notes
//! - Uses the `duckdb` crate (synchronous driver, rusqlite-style API)
//! - Connections open with `AccessMode::ReadOnly` — defense in depth at the
//!   storage layer; writes are rejected by `DuckDB` itself even if the parser
//!   is somehow bypassed. In-memory databases cannot be opened read-only
//!   (there is nothing on disk to protect), so `:memory:` opens read-write
//!   and relies on the capability parser.
//! - BLOB data is Base64-encoded for JSON safety
//! - Statement timeouts enforced via `InterruptHandle` (interrupt + timer thread)
//! - Row limits enforced in application code
//! - `DuckDB` supports schemas; introspection defaults to the `main` schema

use duckdb::types::{TimeUnit, Value};
use duckdb::{params_from_iter, AccessMode, Config, Connection};
use std::time::{Duration, Instant};

use crate::capability::{strip_explain_prefix, validate_query};
use crate::engine::{
    is_explain_query, Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine,
    DatabaseType, ExplainFormat, ExplainPlanNode, ForeignKeyInfo, IndexInfo, IndexSummary,
    IntrospectOperation, IntrospectResult, QueryResult, TableFields, TableInfo, ViewInfo,
};
use crate::error::{PlenumError, Result};

/// `DuckDB` database engine implementation
pub struct DuckDbEngine;

/// Default schema used when no `--schema` is provided.
const DEFAULT_SCHEMA: &str = "main";

impl DatabaseEngine for DuckDbEngine {
    async fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo> {
        let file_path = extract_file_path(config)?;
        let conn = open_connection(&file_path)?;

        // Get DuckDB version (e.g. "v1.5.4")
        let version: String =
            conn.query_row("SELECT version()", [], |row| row.get(0)).map_err(|e| {
                PlenumError::connection_failed(format!("Failed to query DuckDB version: {e}"))
            })?;

        let db_name = database_display_name(config);

        Ok(ConnectionInfo {
            database_version: version.clone(),
            server_info: format!("DuckDB {version}"),
            connected_database: db_name,
            user: "N/A".to_string(), // DuckDB has no user concept
        })
    }

    async fn introspect(
        config: &ConnectionConfig,
        operation: &IntrospectOperation,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<IntrospectResult> {
        let file_path = extract_file_path(config)?;

        // DuckDB is file-based: each file is one database. No database override.
        if database.is_some() {
            return Err(PlenumError::invalid_input(
                "DuckDB does not support --database parameter (use a different connection config to target a different database file)"
            ));
        }

        let schema_name = schema.unwrap_or(DEFAULT_SCHEMA);
        let conn = open_connection(&file_path)?;

        let result = match operation {
            IntrospectOperation::ListDatabases => list_databases_duckdb(&conn)?,
            IntrospectOperation::ListSchemas => list_schemas_duckdb(&conn)?,
            IntrospectOperation::ListTables => list_tables_duckdb(&conn, schema_name)?,
            IntrospectOperation::ListViews => list_views_duckdb(&conn, schema_name)?,
            IntrospectOperation::ListIndexes { table } => {
                list_indexes_duckdb(&conn, schema_name, table.as_deref())?
            }
            IntrospectOperation::TableDetails { name, fields } => {
                get_table_details_duckdb(&conn, schema_name, name, fields)?
            }
            IntrospectOperation::ViewDetails { name } => {
                get_view_details_duckdb(&conn, schema_name, name)?
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
        // Validate query against capabilities before opening any connection
        validate_query(query, caps, DatabaseType::DuckDB)?;

        let file_path = extract_file_path(config)?;
        let conn = open_connection(&file_path)?;

        // Interrupt-based statement timeout: obtain a handle before the query
        // starts, then spawn a thread that fires the interrupt after timeout_ms.
        // DuckDB checks the interrupt flag during execution, cancelling the
        // query server-side rather than just abandoning the wait.
        if let Some(timeout_ms) = caps.timeout_ms {
            let handle = conn.interrupt_handle();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(timeout_ms));
                handle.interrupt();
            });
        }

        // Structured explain path: rewrite to EXPLAIN (FORMAT JSON), normalize.
        if caps.explain_format == Some(ExplainFormat::Structured) {
            if !is_explain_query(query) {
                return Err(PlenumError::invalid_input(
                    "--explain-format structured requires an EXPLAIN statement; \
                     non-EXPLAIN queries must omit this flag",
                ));
            }
            let inner = strip_explain_prefix(query);
            let start = Instant::now();
            let plan = execute_structured_explain_duckdb(&conn, &inner)?;
            let elapsed = start.elapsed();
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
        let mut result = execute_query(&conn, query, params, caps)?;
        let elapsed = start.elapsed();
        result.execution_ms = elapsed.as_millis() as u64;

        Ok(result)
    }
}

/// Validate the config targets `DuckDB` and extract the file path as a string.
fn extract_file_path(config: &ConnectionConfig) -> Result<String> {
    if config.engine != DatabaseType::DuckDB {
        return Err(PlenumError::invalid_input(format!(
            "Expected DuckDB engine, got {}",
            config.engine
        )));
    }

    let file_path = config
        .file
        .as_ref()
        .ok_or_else(|| PlenumError::invalid_input("DuckDB requires 'file' parameter"))?;

    file_path.to_str().map(std::string::ToString::to_string).ok_or_else(|| {
        PlenumError::invalid_input("DuckDB file path contains invalid UTF-8 characters")
    })
}

/// Display name for the connected database (file name, or `:memory:`).
fn database_display_name(config: &ConnectionConfig) -> String {
    config.file.as_ref().map_or_else(
        || "unknown".to_string(),
        |p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| p.to_str().unwrap_or("unknown"))
                .to_string()
        },
    )
}

/// Open a `DuckDB` connection.
///
/// File-backed databases open with `AccessMode::ReadOnly` (defense in depth —
/// `DuckDB` itself rejects writes even if the capability parser were bypassed).
/// `:memory:` databases cannot be opened read-only, so they open read-write;
/// the capability parser remains the enforcement boundary there, and an
/// in-memory database holds no pre-existing data to protect.
fn open_connection(path: &str) -> Result<Connection> {
    if path == ":memory:" {
        return Connection::open_in_memory().map_err(|e| {
            PlenumError::connection_failed(format!("Failed to open DuckDB database: {e}"))
        });
    }

    let config = Config::default().access_mode(AccessMode::ReadOnly).map_err(|e| {
        PlenumError::engine_error("duckdb", format!("Failed to configure read-only mode: {e}"))
    })?;

    Connection::open_with_flags(path, config)
        .map_err(|e| PlenumError::connection_failed(format!("Failed to open DuckDB database: {e}")))
}

/// Returns true when a duckdb error was caused by the interrupt handle firing.
fn is_duckdb_interrupt(e: &duckdb::Error) -> bool {
    e.to_string().to_uppercase().contains("INTERRUPT")
}

/// Run a single-column string query and collect the results.
fn query_string_list(conn: &Connection, sql: &str, params: &[&str]) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql).map_err(|e| {
        PlenumError::engine_error("duckdb", format!("Failed to prepare query: {e}"))
    })?;

    let values: Vec<String> = stmt
        .query_map(params_from_iter(params.iter()), |row| row.get(0))
        .map_err(|e| PlenumError::engine_error("duckdb", format!("Failed to execute query: {e}")))?
        .collect::<std::result::Result<Vec<String>, _>>()
        .map_err(|e| PlenumError::engine_error("duckdb", format!("Failed to collect rows: {e}")))?;

    Ok(values)
}

/// List attached databases (excludes `DuckDB` internal catalogs).
fn list_databases_duckdb(conn: &Connection) -> Result<IntrospectResult> {
    let databases = query_string_list(
        conn,
        "SELECT database_name FROM duckdb_databases() WHERE NOT internal ORDER BY database_name",
        &[],
    )?;
    Ok(IntrospectResult::DatabaseList { databases })
}

/// List schemas in the connected database.
///
/// Filters on `database_name = current_database()` rather than the `internal`
/// flag: `DuckDB` marks the built-in `main` schema as internal, but it is the
/// default location for user tables and must be listed.
fn list_schemas_duckdb(conn: &Connection) -> Result<IntrospectResult> {
    let schemas = query_string_list(
        conn,
        "SELECT schema_name FROM duckdb_schemas()
         WHERE database_name = current_database()
         ORDER BY schema_name",
        &[],
    )?;
    Ok(IntrospectResult::SchemaList { schemas })
}

/// List all tables in a schema (excludes `DuckDB` internal tables)
fn list_tables_duckdb(conn: &Connection, schema: &str) -> Result<IntrospectResult> {
    let tables = query_string_list(
        conn,
        "SELECT table_name FROM duckdb_tables()
         WHERE NOT internal AND schema_name = ?
         ORDER BY table_name",
        &[schema],
    )?;
    Ok(IntrospectResult::TableList { tables })
}

/// List all views in a schema (excludes `DuckDB` internal views)
fn list_views_duckdb(conn: &Connection, schema: &str) -> Result<IntrospectResult> {
    let views = query_string_list(
        conn,
        "SELECT view_name FROM duckdb_views()
         WHERE NOT internal AND schema_name = ?
         ORDER BY view_name",
        &[schema],
    )?;
    Ok(IntrospectResult::ViewList { views })
}

/// Parse a `DuckDB` expression list rendered as text (e.g. `[col_a, col_b]`)
/// into individual column names.
fn parse_bracketed_list(raw: &str) -> Vec<String> {
    raw.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// List all indexes (optionally filtered by table)
fn list_indexes_duckdb(
    conn: &Connection,
    schema: &str,
    table_filter: Option<&str>,
) -> Result<IntrospectResult> {
    let (sql, params): (&str, Vec<&str>) = if let Some(table) = table_filter {
        (
            "SELECT index_name, table_name, is_unique, CAST(expressions AS VARCHAR)
             FROM duckdb_indexes()
             WHERE schema_name = ? AND table_name = ?
             ORDER BY index_name",
            vec![schema, table],
        )
    } else {
        (
            "SELECT index_name, table_name, is_unique, CAST(expressions AS VARCHAR)
             FROM duckdb_indexes()
             WHERE schema_name = ?
             ORDER BY index_name",
            vec![schema],
        )
    };

    let mut stmt = conn.prepare(sql).map_err(|e| {
        PlenumError::engine_error("duckdb", format!("Failed to query indexes: {e}"))
    })?;

    let indexes: Vec<IndexSummary> = stmt
        .query_map(params_from_iter(params.iter()), |row| {
            let name: String = row.get(0)?;
            let table: String = row.get(1)?;
            let unique: bool = row.get(2)?;
            let expressions: String = row.get(3)?;
            Ok(IndexSummary { name, table, unique, columns: parse_bracketed_list(&expressions) })
        })
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to fetch index data: {e}"))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to collect index data: {e}"))
        })?;

    Ok(IntrospectResult::IndexList { indexes })
}

/// Get full table details with conditional field retrieval
fn get_table_details_duckdb(
    conn: &Connection,
    schema: &str,
    table_name: &str,
    fields: &TableFields,
) -> Result<IntrospectResult> {
    let full_table = introspect_table(conn, schema, table_name)?;

    let table = TableInfo {
        name: full_table.name,
        schema: full_table.schema,
        columns: if fields.columns { full_table.columns } else { Vec::new() },
        primary_key: if fields.primary_key { full_table.primary_key } else { None },
        foreign_keys: if fields.foreign_keys { full_table.foreign_keys } else { Vec::new() },
        indexes: if fields.indexes { full_table.indexes } else { Vec::new() },
        comment: full_table.comment,
        row_estimate: full_table.row_estimate,
    };

    Ok(IntrospectResult::TableDetails { table })
}

/// Get view details including definition and columns
fn get_view_details_duckdb(
    conn: &Connection,
    schema: &str,
    view_name: &str,
) -> Result<IntrospectResult> {
    let mut def_stmt = conn
        .prepare(
            "SELECT sql FROM duckdb_views()
             WHERE schema_name = ? AND view_name = ?",
        )
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to prepare view query: {e}"))
        })?;

    let definition: Option<String> = def_stmt
        .query_row(params_from_iter([schema, view_name].iter()), |row| row.get(0))
        .map_err(|e| {
            if matches!(e, duckdb::Error::QueryReturnedNoRows) {
                PlenumError::invalid_input(format!("View '{view_name}' not found"))
            } else {
                PlenumError::engine_error("duckdb", format!("Failed to query view definition: {e}"))
            }
        })?;

    let columns = get_columns(conn, schema, view_name)?;

    let view = ViewInfo {
        name: view_name.to_string(),
        schema: Some(schema.to_string()),
        definition,
        columns,
    };

    Ok(IntrospectResult::ViewDetails { view })
}

/// Get column info for a table or view via `duckdb_columns()`
fn get_columns(conn: &Connection, schema: &str, table_name: &str) -> Result<Vec<ColumnInfo>> {
    let mut stmt = conn
        .prepare(
            "SELECT column_name, data_type, is_nullable, column_default, comment
             FROM duckdb_columns()
             WHERE schema_name = ? AND table_name = ?
             ORDER BY column_index",
        )
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to prepare column query: {e}"))
        })?;

    let columns: Vec<ColumnInfo> = stmt
        .query_map(params_from_iter([schema, table_name].iter()), |row| {
            Ok(ColumnInfo {
                name: row.get(0)?,
                data_type: row.get(1)?,
                nullable: row.get(2)?,
                default: row.get::<_, Option<String>>(3)?,
                comment: row.get::<_, Option<String>>(4)?,
            })
        })
        .map_err(|e| PlenumError::engine_error("duckdb", format!("Failed to query columns: {e}")))?
        .collect::<std::result::Result<Vec<ColumnInfo>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to collect columns: {e}"))
        })?;

    Ok(columns)
}

/// Introspect a single table and return `TableInfo`
fn introspect_table(conn: &Connection, schema: &str, table_name: &str) -> Result<TableInfo> {
    // Verify table exists and fetch comment + row estimate in one pass
    let mut check_stmt = conn
        .prepare(
            "SELECT comment, estimated_size FROM duckdb_tables()
             WHERE NOT internal AND schema_name = ? AND table_name = ?",
        )
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to check table existence: {e}"))
        })?;

    let (comment, row_estimate): (Option<String>, Option<i64>) = check_stmt
        .query_row(params_from_iter([schema, table_name].iter()), |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|e| {
            if matches!(e, duckdb::Error::QueryReturnedNoRows) {
                PlenumError::invalid_input(format!("Table '{table_name}' not found"))
            } else {
                PlenumError::engine_error("duckdb", format!("Failed to query table: {e}"))
            }
        })?;

    let columns = get_columns(conn, schema, table_name)?;

    // Primary key from duckdb_constraints()
    let mut pk_stmt = conn
        .prepare(
            "SELECT array_to_string(constraint_column_names, ',')
             FROM duckdb_constraints()
             WHERE schema_name = ? AND table_name = ? AND constraint_type = 'PRIMARY KEY'",
        )
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to prepare pk query: {e}"))
        })?;

    let pk_raw: Option<String> = pk_stmt
        .query_row(params_from_iter([schema, table_name].iter()), |row| row.get(0))
        .map(Some)
        .or_else(|e| {
            if matches!(e, duckdb::Error::QueryReturnedNoRows) {
                Ok(None)
            } else {
                Err(PlenumError::engine_error(
                    "duckdb",
                    format!("Failed to query primary key: {e}"),
                ))
            }
        })?;

    let primary_key = pk_raw.map(|raw| {
        raw.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>()
    });

    // Foreign keys from duckdb_constraints()
    let mut fk_stmt = conn
        .prepare(
            "SELECT array_to_string(constraint_column_names, ','),
                    referenced_table,
                    array_to_string(referenced_column_names, ',')
             FROM duckdb_constraints()
             WHERE schema_name = ? AND table_name = ? AND constraint_type = 'FOREIGN KEY'
             ORDER BY constraint_index",
        )
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to prepare fk query: {e}"))
        })?;

    let fk_rows: Vec<(String, String, String)> = fk_stmt
        .query_map(params_from_iter([schema, table_name].iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to query foreign keys: {e}"))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("duckdb", format!("Failed to collect foreign keys: {e}"))
        })?;

    let foreign_keys: Vec<ForeignKeyInfo> = fk_rows
        .into_iter()
        .enumerate()
        .map(|(i, (cols, ref_table, ref_cols))| ForeignKeyInfo {
            name: format!("fk_{table_name}_{i}"),
            columns: cols.split(',').map(|s| s.trim().to_string()).collect(),
            referenced_table: ref_table,
            referenced_columns: ref_cols.split(',').map(|s| s.trim().to_string()).collect(),
        })
        .collect();

    // Indexes from duckdb_indexes()
    let IntrospectResult::IndexList { indexes: index_summaries } =
        list_indexes_duckdb(conn, schema, Some(table_name))?
    else {
        return Err(PlenumError::engine_error("duckdb", "Unexpected index list result"));
    };

    let indexes: Vec<IndexInfo> = index_summaries
        .into_iter()
        .map(|s| IndexInfo { name: s.name, columns: s.columns, unique: s.unique })
        .collect();

    Ok(TableInfo {
        name: table_name.to_string(),
        schema: Some(schema.to_string()),
        columns,
        primary_key,
        foreign_keys,
        indexes,
        comment,
        row_estimate,
    })
}

/// Run `EXPLAIN (FORMAT JSON)` on `inner_sql` and normalize the result into an
/// `ExplainPlanNode` tree.
///
/// `DuckDB` returns rows of `(explain_key, explain_value)` where the value is a
/// JSON array of plan nodes: `[{"name": ..., "extra_info": {...}, "children": [...]}]`.
fn execute_structured_explain_duckdb(
    conn: &Connection,
    inner_sql: &str,
) -> Result<ExplainPlanNode> {
    let sql = format!("EXPLAIN (FORMAT JSON) {inner_sql}");

    let mut stmt = conn.prepare(&sql).map_err(|e| {
        PlenumError::query_failed(format!("Failed to prepare EXPLAIN (FORMAT JSON): {e}"))
    })?;

    let json_text: String = stmt.query_row([], |row| row.get(1)).map_err(|e| {
        PlenumError::query_failed(format!("Failed to execute EXPLAIN (FORMAT JSON): {e}"))
    })?;

    let parsed: serde_json::Value = serde_json::from_str(&json_text).map_err(|e| {
        PlenumError::query_failed(format!("Failed to parse DuckDB EXPLAIN JSON: {e}"))
    })?;

    fn build_node(node: &serde_json::Value) -> ExplainPlanNode {
        let node_type =
            node.get("name").and_then(|v| v.as_str()).unwrap_or("UNKNOWN").trim().to_string();

        let extra = node.get("extra_info");
        let relation = extra
            .and_then(|e| e.get("Table"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string());
        let estimated_rows = extra
            .and_then(|e| e.get("Estimated Cardinality"))
            .and_then(|v| v.as_str().map_or_else(|| v.as_f64(), |s| s.trim().parse::<f64>().ok()));

        let children = node
            .get("children")
            .and_then(|v| v.as_array())
            .map(|kids| kids.iter().map(build_node).collect())
            .unwrap_or_default();

        ExplainPlanNode { node_type, relation, estimated_rows, estimated_cost: None, children }
    }

    let top_children: Vec<ExplainPlanNode> = match &parsed {
        serde_json::Value::Array(nodes) => nodes.iter().map(build_node).collect(),
        other => vec![build_node(other)],
    };

    Ok(ExplainPlanNode {
        node_type: "QUERY PLAN".to_string(),
        relation: None,
        estimated_rows: None,
        estimated_cost: None,
        children: top_children,
    })
}

/// Convert a JSON parameter value to a `duckdb` native value for binding
fn json_to_duckdb_value(val: &serde_json::Value) -> Value {
    match val {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::BigInt(i)
            } else {
                Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        v => Value::Text(v.to_string()),
    }
}

/// Execute query and return `QueryResult`
fn execute_query(
    conn: &Connection,
    query: &str,
    params: &[serde_json::Value],
    caps: &Capabilities,
) -> Result<QueryResult> {
    let mut stmt = conn
        .prepare(query)
        .map_err(|e| PlenumError::query_failed(format!("Failed to prepare query: {e}")))?;

    let duckdb_params: Vec<Value> = params.iter().map(json_to_duckdb_value).collect();

    let mut rows = stmt.query(params_from_iter(duckdb_params.iter())).map_err(|e| {
        if is_duckdb_interrupt(&e) {
            PlenumError::query_timeout("Query interrupted by DuckDB server-side timeout")
        } else {
            PlenumError::query_failed(format!("Failed to execute query: {e}"))
        }
    })?;

    // Column names are only available after execution in the duckdb crate.
    let column_names: Vec<String> =
        rows.as_ref().map(duckdb::Statement::column_names).unwrap_or_default();

    let offset = caps.offset.unwrap_or(0);
    let max = caps.max_rows;
    let mut pos = 0usize;
    let mut rows_data: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut rows_truncated = false;

    loop {
        let next = rows.next().map_err(|e| {
            if is_duckdb_interrupt(&e) {
                PlenumError::query_timeout(
                    "Query interrupted by DuckDB server-side timeout during row fetch",
                )
            } else {
                PlenumError::query_failed(format!("Failed to fetch row: {e}"))
            }
        })?;
        let Some(row) = next else { break };

        // Skip offset rows
        if pos < offset {
            pos += 1;
            continue;
        }

        // Probe one row past max_rows to detect truncation
        if let Some(m) = max {
            if rows_data.len() >= m {
                rows_truncated = true;
                break;
            }
        }

        let mut values = Vec::with_capacity(column_names.len());
        for idx in 0..column_names.len() {
            let value_ref = row.get_ref(idx).map_err(|e| {
                PlenumError::query_failed(format!("Failed to read column {idx}: {e}"))
            })?;
            values.push(duckdb_value_to_json(&value_ref.to_owned()));
        }
        rows_data.push(values);
        pos += 1;
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
}

/// Format a `DuckDB` timestamp/time value (count of `unit` since the epoch /
/// midnight) as an ISO-8601 string, falling back to the raw integer if the
/// value is out of chrono's representable range.
fn format_timestamp(unit: TimeUnit, value: i64) -> serde_json::Value {
    let micros = unit.to_micros(value);
    chrono::DateTime::from_timestamp_micros(micros).map_or_else(
        || serde_json::Value::Number(micros.into()),
        |dt| serde_json::Value::String(dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string()),
    )
}

fn format_time(unit: TimeUnit, value: i64) -> serde_json::Value {
    let micros = unit.to_micros(value);
    let raw = serde_json::Value::Number(micros.into());
    // TIME values are non-negative micros since midnight; a negative value is
    // out of range and falls back to the raw integer.
    let Ok(secs) = u32::try_from(micros / 1_000_000) else { return raw };
    let Ok(sub_micros) = u32::try_from(micros % 1_000_000) else { return raw };
    chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, sub_micros * 1000).map_or_else(
        || serde_json::Value::Number(micros.into()),
        |t| serde_json::Value::String(t.format("%H:%M:%S%.6f").to_string()),
    )
}

fn format_date(days_since_epoch: i32) -> serde_json::Value {
    chrono::DateTime::from_timestamp(i64::from(days_since_epoch) * 86_400, 0).map_or_else(
        || serde_json::Value::Number(days_since_epoch.into()),
        |dt| serde_json::Value::String(dt.date_naive().format("%Y-%m-%d").to_string()),
    )
}

/// Convert an f64 to JSON, mapping NaN/Infinity to null
fn f64_to_json(f: f64) -> serde_json::Value {
    serde_json::Number::from_f64(f).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// Convert an owned `DuckDB` value to a JSON value.
///
/// Scalar types map to their natural JSON equivalents. Values that JSON cannot
/// represent natively are stringified deterministically:
/// - `HUGEINT` / `UHUGEINT` and `DECIMAL` → string (preserves precision)
/// - `TIMESTAMP` / `DATE` / `TIME` → ISO-8601 string
/// - `BLOB` → Base64 string
/// - `INTERVAL` → object with `months` / `days` / `nanos`
/// - Nested types (`LIST`, `ARRAY`, `STRUCT`, `MAP`, `UNION`, `ENUM`) convert
///   recursively to JSON arrays / objects.
fn duckdb_value_to_json(value: &Value) -> serde_json::Value {
    use serde_json::Value as Json;

    match value {
        Value::Null => Json::Null,
        Value::Boolean(b) => Json::Bool(*b),
        Value::TinyInt(i) => Json::Number((*i).into()),
        Value::SmallInt(i) => Json::Number((*i).into()),
        Value::Int(i) => Json::Number((*i).into()),
        Value::BigInt(i) => Json::Number((*i).into()),
        // HUGEINT exceeds JSON's i64 range; preserve precision as a string
        Value::HugeInt(i) => Json::String(i.to_string()),
        Value::UTinyInt(i) => Json::Number((*i).into()),
        Value::USmallInt(i) => Json::Number((*i).into()),
        Value::UInt(i) => Json::Number((*i).into()),
        Value::UBigInt(i) => Json::Number((*i).into()),
        Value::Float(f) => f64_to_json(f64::from(*f)),
        Value::Double(f) => f64_to_json(*f),
        // DECIMAL preserves exact precision as a string
        Value::Decimal(d) => Json::String(d.to_string()),
        Value::Timestamp(unit, v) => format_timestamp(*unit, *v),
        Value::Text(s) | Value::Enum(s) => Json::String(s.clone()),
        Value::Blob(b) => {
            use base64::Engine;
            Json::String(base64::engine::general_purpose::STANDARD.encode(b))
        }
        Value::Date32(d) => format_date(*d),
        Value::Time64(unit, v) => format_time(*unit, *v),
        Value::Interval { months, days, nanos } => serde_json::json!({
            "months": months,
            "days": days,
            "nanos": nanos,
        }),
        Value::List(items) | Value::Array(items) => {
            Json::Array(items.iter().map(duckdb_value_to_json).collect())
        }
        Value::Struct(map) => {
            let obj: serde_json::Map<String, Json> =
                map.iter().map(|(k, v)| (k.clone(), duckdb_value_to_json(v))).collect();
            Json::Object(obj)
        }
        Value::Map(map) => {
            let obj: serde_json::Map<String, Json> = map
                .iter()
                .map(|(k, v)| {
                    let key = match k {
                        Value::Text(s) | Value::Enum(s) => s.clone(),
                        other => match duckdb_value_to_json(other) {
                            Json::String(s) => s,
                            j => j.to_string(),
                        },
                    };
                    (key, duckdb_value_to_json(v))
                })
                .collect();
            Json::Object(obj)
        }
        Value::Union(inner) => duckdb_value_to_json(inner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DatabaseType;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn fixture_path(tag: &str) -> PathBuf {
        let id = FIXTURE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("plenum_duckdb_{tag}_{pid}_{id}.duckdb"))
    }

    #[tokio::test]
    async fn test_validate_connection_memory() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let result = DuckDbEngine::validate_connection(&config).await;
        assert!(result.is_ok(), "validate failed: {:?}", result.err());

        let info = result.unwrap();
        assert!(info.database_version.starts_with('v'), "unexpected: {}", info.database_version);
        assert!(info.server_info.contains("DuckDB"));
        assert_eq!(info.connected_database, ":memory:");
        assert_eq!(info.user, "N/A");
    }

    #[tokio::test]
    async fn test_validate_connection_wrong_engine() {
        let mut config = ConnectionConfig::duckdb(":memory:".into());
        config.engine = DatabaseType::SQLite;

        let result = DuckDbEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Expected DuckDB engine"));
    }

    #[tokio::test]
    async fn test_validate_connection_missing_file() {
        let config = ConnectionConfig {
            engine: DatabaseType::DuckDB,
            file: None,
            tls: None,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
        };

        let result = DuckDbEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("DuckDB requires 'file' parameter"));
    }

    #[tokio::test]
    async fn test_validate_connection_nonexistent_file() {
        let config =
            ConnectionConfig::duckdb(std::env::temp_dir().join("plenum_no_such_file.duckdb"));
        let result = DuckDbEngine::validate_connection(&config).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().error_code(), "CONNECTION_FAILED");
    }

    #[tokio::test]
    async fn test_introspect_schema() {
        let temp_file = fixture_path("introspect");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute_batch(
                "CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    email TEXT
                )",
            )
            .expect("Failed to create table");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "users".to_string(),
                fields: TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_ok(), "introspect failed: {:?}", result.err());

        let IntrospectResult::TableDetails { table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

        assert_eq!(table.name, "users");
        assert_eq!(table.schema.as_deref(), Some("main"));
        assert_eq!(table.columns.len(), 3);

        let pk = table.primary_key.as_ref().expect("primary key missing");
        assert_eq!(pk, &vec!["id".to_string()]);

        // NOT NULL detection
        let name_col = table.columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name_col.nullable);
        let email_col = table.columns.iter().find(|c| c.name == "email").unwrap();
        assert!(email_col.nullable);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_table_not_found() {
        let temp_file = fixture_path("notfound");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE t (id INTEGER)").unwrap();
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "missing".to_string(),
                fields: TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("not found"));
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_list_schemas() {
        let temp_file = fixture_path("schemas");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE SCHEMA analytics; CREATE TABLE analytics.t (id INTEGER)")
                .unwrap();
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let result =
            DuckDbEngine::introspect(&config, &IntrospectOperation::ListSchemas, None, None)
                .await
                .expect("ListSchemas failed");

        let IntrospectResult::SchemaList { schemas } = result else {
            panic!("Expected SchemaList")
        };
        assert!(schemas.contains(&"main".to_string()));
        assert!(schemas.contains(&"analytics".to_string()));

        // Schema-scoped table listing
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::ListTables,
            None,
            Some("analytics"),
        )
        .await
        .expect("ListTables failed");
        let IntrospectResult::TableList { tables } = result else { panic!("Expected TableList") };
        assert_eq!(tables, vec!["t".to_string()]);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_database_override_rejected() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::ListTables,
            Some("other"),
            None,
        )
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("--database"));
    }

    #[tokio::test]
    async fn test_execute_select_query() {
        let temp_file = fixture_path("select");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO users VALUES (1, 'Alice')",
            )
            .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities::default();
        let result = DuckDbEngine::execute(&config, "SELECT * FROM users", &[], &caps).await;
        assert!(result.is_ok(), "select failed: {:?}", result.err());

        let query_result = result.unwrap();
        assert_eq!(query_result.columns, vec!["id".to_string(), "name".to_string()]);
        assert_eq!(query_result.rows.len(), 1);
        assert_eq!(query_result.rows[0][1], serde_json::json!("Alice"));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_insert_rejected() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let caps = Capabilities::default();
        let result =
            DuckDbEngine::execute(&config, "INSERT INTO users VALUES (1, 'Bob')", &[], &caps).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
        assert!(err.message().contains("Plenum is read-only"));
    }

    #[tokio::test]
    async fn test_execute_ddl_rejected() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let caps = Capabilities::default();
        for sql in [
            "CREATE TABLE t (id INTEGER)",
            "DROP TABLE t",
            "ALTER TABLE t ADD COLUMN c INTEGER",
            "UPDATE t SET id = 2",
            "DELETE FROM t",
            "ATTACH ':memory:' AS other",
            "COPY t TO 'out.csv'",
        ] {
            let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;
            assert!(result.is_err(), "expected rejection for: {sql}");
            assert_eq!(result.unwrap_err().error_code(), "CAPABILITY_VIOLATION", "sql: {sql}");
        }
    }

    #[tokio::test]
    async fn test_execute_max_rows_limit() {
        let temp_file = fixture_path("maxrows");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE nums AS SELECT range AS n FROM range(10)")
                .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities { max_rows: Some(5), ..Capabilities::default() };
        let result =
            DuckDbEngine::execute(&config, "SELECT * FROM nums ORDER BY n", &[], &caps).await;

        assert!(result.is_ok(), "query failed: {:?}", result.err());
        let query_result = result.unwrap();
        assert_eq!(query_result.rows.len(), 5);
        assert!(query_result.rows_truncated);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_offset_pagination() {
        let temp_file = fixture_path("offset");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE nums AS SELECT range AS n FROM range(10)").unwrap();
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities { max_rows: Some(3), offset: Some(4), ..Capabilities::default() };
        let result =
            DuckDbEngine::execute(&config, "SELECT n FROM nums ORDER BY n", &[], &caps).await;
        let qr = result.expect("query failed");
        assert_eq!(qr.rows.len(), 3);
        assert_eq!(qr.rows[0][0], serde_json::json!(4));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_all_data_types() {
        let temp_file = fixture_path("types");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch(
                "CREATE TABLE test_types (
                    c_bool BOOLEAN,
                    c_int INTEGER,
                    c_bigint BIGINT,
                    c_hugeint HUGEINT,
                    c_double DOUBLE,
                    c_decimal DECIMAL(18,4),
                    c_text VARCHAR,
                    c_blob BLOB,
                    c_date DATE,
                    c_time TIME,
                    c_timestamp TIMESTAMP,
                    c_list INTEGER[],
                    c_struct STRUCT(a INTEGER, b VARCHAR),
                    c_null VARCHAR
                );
                INSERT INTO test_types VALUES (
                    TRUE,
                    42,
                    9223372036854775807,
                    170141183460469231731687303715884105727,
                    3.5,
                    12345.6789,
                    'café résumé 🚀',
                    '\\xDE\\xAD\\xBE\\xEF'::BLOB,
                    DATE '2024-01-15',
                    TIME '13:45:30',
                    TIMESTAMP '2024-01-15 13:45:30',
                    [1, 2, 3],
                    {a: 7, b: 'x'},
                    NULL
                )",
            )
            .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities::default();
        let result = DuckDbEngine::execute(&config, "SELECT * FROM test_types", &[], &caps).await;

        assert!(result.is_ok(), "query failed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        let row = &qr.rows[0];

        assert_eq!(row[0], serde_json::json!(true));
        assert_eq!(row[1], serde_json::json!(42));
        assert_eq!(row[2], serde_json::json!(9_223_372_036_854_775_807_i64));
        // HUGEINT → string (exceeds JSON i64 range)
        assert_eq!(row[3], serde_json::json!("170141183460469231731687303715884105727"));
        assert_eq!(row[4], serde_json::json!(3.5));
        // DECIMAL → string (preserves precision)
        assert_eq!(row[5], serde_json::json!("12345.6789"));
        assert_eq!(row[6], serde_json::json!("café résumé 🚀"));
        // BLOB → Base64
        assert_eq!(row[7], serde_json::json!("3q2+7w=="));
        assert_eq!(row[8], serde_json::json!("2024-01-15"));
        assert!(row[9].as_str().unwrap().starts_with("13:45:30"));
        assert!(row[10].as_str().unwrap().starts_with("2024-01-15 13:45:30"));
        assert_eq!(row[11], serde_json::json!([1, 2, 3]));
        assert_eq!(row[12], serde_json::json!({"a": 7, "b": "x"}));
        assert_eq!(row[13], serde_json::Value::Null);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_foreign_keys() {
        let temp_file = fixture_path("fk");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
                 CREATE TABLE posts (
                     id INTEGER PRIMARY KEY,
                     user_id INTEGER,
                     FOREIGN KEY (user_id) REFERENCES users(id)
                 )",
            )
            .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "posts".to_string(),
                fields: TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_ok(), "introspect failed: {:?}", result.err());

        let IntrospectResult::TableDetails { table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

        assert!(!table.foreign_keys.is_empty(), "expected foreign keys");
        let fk = &table.foreign_keys[0];
        assert_eq!(fk.referenced_table, "users");
        assert_eq!(fk.columns, vec!["user_id"]);
        assert_eq!(fk.referenced_columns, vec!["id"]);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_indexes_and_views() {
        let temp_file = fixture_path("idx");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);
                 CREATE INDEX idx_email ON users(email);
                 CREATE UNIQUE INDEX idx_email_unique ON users(email);
                 CREATE VIEW v_users AS SELECT id FROM users",
            )
            .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());

        // ListIndexes
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::ListIndexes { table: Some("users".to_string()) },
            None,
            None,
        )
        .await
        .expect("ListIndexes failed");
        let IntrospectResult::IndexList { indexes } = result else { panic!("Expected IndexList") };
        assert!(indexes.iter().any(|i| i.name == "idx_email" && !i.unique));
        let uniq = indexes.iter().find(|i| i.name == "idx_email_unique").expect("unique idx");
        assert!(uniq.unique);
        assert_eq!(uniq.columns, vec!["email"]);

        // ListViews + ViewDetails
        let result = DuckDbEngine::introspect(&config, &IntrospectOperation::ListViews, None, None)
            .await
            .expect("ListViews failed");
        let IntrospectResult::ViewList { views } = result else { panic!("Expected ViewList") };
        assert_eq!(views, vec!["v_users".to_string()]);

        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::ViewDetails { name: "v_users".to_string() },
            None,
            None,
        )
        .await
        .expect("ViewDetails failed");
        let IntrospectResult::ViewDetails { view } = result else { panic!("Expected ViewDetails") };
        assert_eq!(view.name, "v_users");
        assert!(view.definition.is_some());
        assert_eq!(view.columns.len(), 1);

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_comments_and_row_estimate() {
        let temp_file = fixture_path("comments");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT);
                 COMMENT ON TABLE items IS 'inventory items';
                 COMMENT ON COLUMN items.label IS 'display label';
                 INSERT INTO items SELECT range, 'x' FROM range(5)",
            )
            .expect("seed");
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let result = DuckDbEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "items".to_string(),
                fields: TableFields::all(),
            },
            None,
            None,
        )
        .await
        .expect("introspect failed");

        let IntrospectResult::TableDetails { table } = result else {
            panic!("Expected TableDetails")
        };

        assert_eq!(table.comment.as_deref(), Some("inventory items"));
        let label = table.columns.iter().find(|c| c.name == "label").unwrap();
        assert_eq!(label.comment.as_deref(), Some("display label"));
        assert_eq!(table.row_estimate, Some(5));

        let _ = std::fs::remove_file(&temp_file);
    }

    /// Prove that writes fail at the `DuckDB` storage layer independently of
    /// the parser: a connection opened with `AccessMode::ReadOnly` must reject
    /// direct DML/DDL without going through `validate_query`.
    #[test]
    fn test_duckdb_session_read_only_enforcement() {
        let temp_file = fixture_path("session_ro");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE t (id INTEGER)").expect("seed");
        }

        let conn = open_connection(temp_file.to_str().unwrap())
            .expect("Failed to open read-only connection");

        let insert_result = conn.execute("INSERT INTO t VALUES (1)", []);
        assert!(insert_result.is_err(), "INSERT must be rejected at the DuckDB storage layer");

        let create_result = conn.execute("CREATE TABLE t2 (id INTEGER)", []);
        assert!(
            create_result.is_err(),
            "CREATE TABLE must be rejected at the DuckDB storage layer"
        );

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_server_side_timeout_interrupt() {
        // A large cross-join aggregate runs for many seconds; with a 50ms
        // timeout the interrupt thread fires mid-execution and DuckDB returns
        // an INTERRUPT error, surfaced as QUERY_TIMEOUT.
        let config = ConnectionConfig::duckdb(":memory:".into());
        let caps = Capabilities { timeout_ms: Some(50), ..Capabilities::default() };
        let sql = "SELECT max(a.range * b.range + a.range) \
                   FROM range(200000) a, range(200000) b";

        let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;

        assert!(result.is_err(), "Expected a timeout error but got Ok");
        let err = result.unwrap_err();
        assert_eq!(
            err.error_code(),
            "QUERY_TIMEOUT",
            "Expected QUERY_TIMEOUT, got {}: {}",
            err.error_code(),
            err.message()
        );
    }

    #[tokio::test]
    async fn test_execute_structured_explain() {
        let temp_file = fixture_path("explain");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE t AS SELECT range AS n FROM range(100)").unwrap();
        }

        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities {
            explain_format: Some(ExplainFormat::Structured),
            ..Capabilities::default()
        };
        let result =
            DuckDbEngine::execute(&config, "EXPLAIN SELECT * FROM t WHERE n > 5", &[], &caps).await;
        assert!(result.is_ok(), "structured explain failed: {:?}", result.err());
        let qr = result.unwrap();
        let plan = qr.plan.expect("plan missing");
        assert_eq!(plan.node_type, "QUERY PLAN");
        assert!(!plan.children.is_empty(), "plan should have children");

        // Structured explain on a non-EXPLAIN query is rejected
        let result = DuckDbEngine::execute(&config, "SELECT 1", &[], &caps).await;
        assert!(result.is_err());

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_native_explain_passthrough() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let caps = Capabilities::default();
        let result = DuckDbEngine::execute(&config, "EXPLAIN SELECT 1", &[], &caps).await;
        assert!(result.is_ok(), "native explain failed: {:?}", result.err());
        let qr = result.unwrap();
        assert!(qr.plan.is_none());
        assert!(!qr.rows.is_empty());
    }

    // =========================================================================
    // Parameterized query tests
    // =========================================================================

    #[tokio::test]
    async fn test_execute_bound_params() {
        let temp_file = fixture_path("params");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch(
                "CREATE TABLE t (id INTEGER, name TEXT, score DOUBLE);
                 INSERT INTO t VALUES (1, 'alice', 9.5), (2, 'bob', 7.0), (3, 'charlie', 8.2)",
            )
            .unwrap();
        }
        let config = ConnectionConfig::duckdb(temp_file.clone());

        // Integer param
        let params = vec![serde_json::json!(1)];
        let qr = DuckDbEngine::execute(
            &config,
            "SELECT name FROM t WHERE id = ?",
            &params,
            &Capabilities::default(),
        )
        .await
        .expect("bound integer param");
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!("alice"));

        // Text param
        let params = vec![serde_json::json!("bob")];
        let qr = DuckDbEngine::execute(
            &config,
            "SELECT id FROM t WHERE name = ?",
            &params,
            &Capabilities::default(),
        )
        .await
        .expect("bound text param");
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!(2));

        // Multiple float params
        let params = vec![serde_json::json!(7.5), serde_json::json!(9.0)];
        let qr = DuckDbEngine::execute(
            &config,
            "SELECT name FROM t WHERE score >= ? AND score <= ? ORDER BY score",
            &params,
            &Capabilities::default(),
        )
        .await
        .expect("multiple bound params");
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!("charlie"));

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_write_still_rejected_with_params() {
        let config = ConnectionConfig::duckdb(":memory:".into());
        let params = vec![serde_json::json!(42), serde_json::json!("evil")];
        let caps = Capabilities::default();
        let result =
            DuckDbEngine::execute(&config, "INSERT INTO t VALUES (?, ?)", &params, &caps).await;
        assert!(result.is_err(), "write must be rejected even with params");
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[tokio::test]
    async fn test_execute_show_describe_summarize_allowed() {
        let temp_file = fixture_path("show");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("create");
            conn.execute_batch("CREATE TABLE t AS SELECT range AS n FROM range(10)").unwrap();
        }
        let config = ConnectionConfig::duckdb(temp_file.clone());
        let caps = Capabilities::default();

        for sql in ["SHOW TABLES", "DESCRIBE t", "SUMMARIZE t", "PRAGMA table_info('t')"] {
            let result = DuckDbEngine::execute(&config, sql, &[], &caps).await;
            assert!(result.is_ok(), "expected {sql} to be allowed: {:?}", result.err());
        }

        let _ = std::fs::remove_file(&temp_file);
    }
}
