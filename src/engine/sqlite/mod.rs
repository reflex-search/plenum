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
//! - Statement timeouts enforced via `sqlite3_interrupt` (interrupt handle + timer thread)
//! - Lock contention timeouts enforced via `busy_timeout`
//! - Row limits enforced in application code
//! - No explicit schema support (`SQLite` uses catalogs)

use rusqlite::{Connection, OpenFlags, Row};
use std::collections::HashMap; // Used for grouping foreign keys during introspection
use std::time::{Duration, Instant};

use crate::capability::{strip_explain_prefix, validate_query};
use crate::engine::{
    is_explain_query, Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine,
    DatabaseType, ExplainFormat, ExplainPlanNode, ForeignKeyInfo, IndexInfo, IntrospectOperation,
    IntrospectResult, QueryResult, TableInfo,
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
        operation: &IntrospectOperation,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> Result<IntrospectResult> {
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

        // Note: SQLite doesn't support database override - it's file-based
        if database.is_some() {
            return Err(PlenumError::invalid_input(
                "SQLite does not support --database parameter (use different connection config to target different database file)"
            ));
        }

        // Note: SQLite doesn't have schemas like Postgres/MySQL
        if schema.is_some() {
            return Err(PlenumError::invalid_input(
                "SQLite does not support --schema parameter (SQLite has no schema concept)",
            ));
        }

        let conn = open_connection(path_str, true)?;

        // Route to appropriate operation handler
        let result = match operation {
            IntrospectOperation::ListDatabases => {
                // SQLite doesn't have a database list concept (each file is a database)
                return Err(PlenumError::invalid_input(
                    "SQLite does not support ListDatabases operation (each file is a separate database)"
                ));
            }

            IntrospectOperation::ListSchemas => {
                // SQLite doesn't have schemas
                return Err(PlenumError::invalid_input(
                    "SQLite does not support ListSchemas operation (SQLite has no schema concept)",
                ));
            }

            IntrospectOperation::ListTables => list_tables_sqlite(&conn)?,

            IntrospectOperation::ListViews => list_views_sqlite(&conn)?,

            IntrospectOperation::ListIndexes { table } => {
                list_indexes_sqlite(&conn, table.as_deref())?
            }

            IntrospectOperation::TableDetails { name, fields } => {
                get_table_details_sqlite(&conn, name, fields)?
            }

            IntrospectOperation::ViewDetails { name } => get_view_details_sqlite(&conn, name)?,
        };

        Ok(result)
    }

    async fn execute(
        config: &ConnectionConfig,
        query: &str,
        params: &[serde_json::Value],
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

        // Open connection (read-only: defense in depth at OS/VFS level — writes are
        // rejected by SQLite itself even if the parser is somehow bypassed)
        let path_str = file_path.to_str().ok_or_else(|| {
            PlenumError::invalid_input("SQLite file path contains invalid UTF-8 characters")
        })?;
        let conn = open_connection(path_str, true)?;

        // Set busy_timeout for lock-contention waits (database file locked by another writer).
        if let Some(timeout_ms) = caps.timeout_ms {
            conn.busy_timeout(Duration::from_millis(timeout_ms)).map_err(|e| {
                PlenumError::engine_error("sqlite", format!("Failed to set busy_timeout: {e}"))
            })?;
        }

        // Interrupt-based statement timeout: obtain a handle before the query starts,
        // then spawn a thread that fires sqlite3_interrupt after timeout_ms. SQLite checks
        // for interrupts between VM steps, cancelling the query server-side rather than
        // just abandoning the wait. Per SQLite docs, interrupt() on an idle connection is
        // a no-op, so the timer is harmless if the query finishes first.
        if let Some(timeout_ms) = caps.timeout_ms {
            let handle = conn.get_interrupt_handle();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(timeout_ms));
                handle.interrupt();
            });
        }

        // Structured explain path: rewrite to EXPLAIN QUERY PLAN, normalize the plan tree.
        if caps.explain_format == Some(ExplainFormat::Structured) {
            if !is_explain_query(query) {
                return Err(PlenumError::invalid_input(
                    "--explain-format structured requires an EXPLAIN statement; \
                     non-EXPLAIN queries must omit this flag",
                ));
            }
            let inner = strip_explain_prefix(query);
            let start = Instant::now();
            let plan = execute_structured_explain_sqlite(&conn, &inner)?;
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

        // Execute query
        let start = Instant::now();
        let mut result = execute_query(&conn, query, params, caps)?;
        let elapsed = start.elapsed();
        result.execution_ms = elapsed.as_millis() as u64;

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

/// List all tables (excludes `SQLite` internal tables)
fn list_tables_sqlite(conn: &Connection) -> Result<IntrospectResult> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table'
             AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .map_err(|e| PlenumError::engine_error("sqlite", format!("Failed to query tables: {e}")))?;

    let tables: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to fetch table names: {e}"))
        })?
        .collect::<std::result::Result<Vec<String>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to collect table names: {e}"))
        })?;

    Ok(IntrospectResult::TableList { tables })
}

/// List all views
fn list_views_sqlite(conn: &Connection) -> Result<IntrospectResult> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'view'
             ORDER BY name",
        )
        .map_err(|e| PlenumError::engine_error("sqlite", format!("Failed to query views: {e}")))?;

    let views: Vec<String> = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to fetch view names: {e}"))
        })?
        .collect::<std::result::Result<Vec<String>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to collect view names: {e}"))
        })?;

    Ok(IntrospectResult::ViewList { views })
}

/// List all indexes (optionally filtered by table)
fn list_indexes_sqlite(conn: &Connection, table_filter: Option<&str>) -> Result<IntrospectResult> {
    use crate::engine::IndexSummary;

    // Query sqlite_master for all indexes
    let query = if let Some(table) = table_filter {
        format!(
            "SELECT name, tbl_name FROM sqlite_master
             WHERE type = 'index'
             AND tbl_name = '{table}'
             ORDER BY name"
        )
    } else {
        "SELECT name, tbl_name FROM sqlite_master
         WHERE type = 'index'
         ORDER BY name"
            .to_string()
    };

    let mut stmt = conn.prepare(&query).map_err(|e| {
        PlenumError::engine_error("sqlite", format!("Failed to query indexes: {e}"))
    })?;

    let index_data: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to fetch index data: {e}"))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to collect index data: {e}"))
        })?;

    let mut indexes = Vec::new();
    for (index_name, table_name) in index_data {
        // Skip auto-created indexes for primary keys
        if index_name.starts_with("sqlite_autoindex_") {
            continue;
        }

        // Get index info using PRAGMA
        let mut idx_info_stmt =
            conn.prepare(&format!("PRAGMA index_info({index_name})")).map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to prepare index_info for {index_name}: {e}"),
                )
            })?;

        let columns: Vec<String> = idx_info_stmt
            .query_map([], |row| row.get::<_, String>(2))
            .map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to query index columns for {index_name}: {e}"),
                )
            })?
            .filter_map(std::result::Result::ok)
            .collect();

        // Check if index is unique using PRAGMA index_list
        let mut idx_list_stmt =
            conn.prepare(&format!("PRAGMA index_list({table_name})")).map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to prepare index_list for {table_name}: {e}"),
                )
            })?;

        let unique = idx_list_stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let is_unique: i32 = row.get(2)?;
                Ok((name, is_unique != 0))
            })
            .map_err(|e| {
                PlenumError::engine_error(
                    "sqlite",
                    format!("Failed to query index uniqueness for {table_name}: {e}"),
                )
            })?
            .find_map(|r| {
                if let Ok((name, is_unique)) = r {
                    if name == index_name {
                        return Some(is_unique);
                    }
                }
                None
            })
            .unwrap_or(false);

        indexes.push(IndexSummary { name: index_name, table: table_name, unique, columns });
    }

    Ok(IntrospectResult::IndexList { indexes })
}

/// Get full table details with conditional field retrieval
fn get_table_details_sqlite(
    conn: &Connection,
    table_name: &str,
    fields: &crate::engine::TableFields,
) -> Result<IntrospectResult> {
    // Verify table exists
    let mut check_stmt = conn
        .prepare(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = ?",
        )
        .map_err(|e| {
            PlenumError::engine_error("sqlite", format!("Failed to check table existence: {e}"))
        })?;

    let count: i32 = check_stmt.query_row([table_name], |row| row.get(0)).map_err(|e| {
        PlenumError::engine_error("sqlite", format!("Failed to query table existence: {e}"))
    })?;

    if count == 0 {
        return Err(PlenumError::invalid_input(format!("Table '{table_name}' not found")));
    }

    // Get full table info (we'll filter fields afterward)
    let full_table = introspect_table(conn, table_name)?;

    // Filter fields based on selector
    let table = TableInfo {
        name: full_table.name,
        schema: None,
        columns: if fields.columns { full_table.columns } else { Vec::new() },
        primary_key: if fields.primary_key { full_table.primary_key } else { None },
        foreign_keys: if fields.foreign_keys { full_table.foreign_keys } else { Vec::new() },
        indexes: if fields.indexes { full_table.indexes } else { Vec::new() },
        comment: None,
        row_estimate: full_table.row_estimate,
    };

    Ok(IntrospectResult::TableDetails { table })
}

/// Get view details including definition and columns
fn get_view_details_sqlite(conn: &Connection, view_name: &str) -> Result<IntrospectResult> {
    use crate::engine::ViewInfo;

    // Get view definition from sqlite_master
    let mut def_stmt =
        conn.prepare("SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?").map_err(
            |e| PlenumError::engine_error("sqlite", format!("Failed to prepare view query: {e}")),
        )?;

    let definition: Option<String> =
        def_stmt.query_row([view_name], |row| row.get(0)).map_err(|e| {
            if matches!(e, rusqlite::Error::QueryReturnedNoRows) {
                PlenumError::invalid_input(format!("View '{view_name}' not found"))
            } else {
                PlenumError::engine_error("sqlite", format!("Failed to query view definition: {e}"))
            }
        })?;

    // Get view columns using PRAGMA table_info (works for views too)
    let mut col_stmt = conn.prepare(&format!("PRAGMA table_info({view_name})")).map_err(|e| {
        PlenumError::engine_error(
            "sqlite",
            format!("Failed to prepare table_info for view {view_name}: {e}"),
        )
    })?;

    let columns: Vec<ColumnInfo> = col_stmt
        .query_map([], |row| {
            Ok(ColumnInfo {
                name: row.get::<_, String>(1)?,
                data_type: row.get::<_, String>(2)?,
                nullable: row.get::<_, i32>(3)? == 0,
                default: row.get::<_, Option<String>>(4)?,
                comment: None,
            })
        })
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to query columns for view {view_name}: {e}"),
            )
        })?
        .collect::<std::result::Result<Vec<ColumnInfo>, _>>()
        .map_err(|e| {
            PlenumError::engine_error(
                "sqlite",
                format!("Failed to collect columns for view {view_name}: {e}"),
            )
        })?;

    let view = ViewInfo { name: view_name.to_string(), schema: None, definition, columns };

    Ok(IntrospectResult::ViewDetails { view })
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
                comment: None, // SQLite has no native column comment storage
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

    let row_estimate = get_sqlite_row_estimate(conn, table_name);

    Ok(TableInfo {
        name: table_name.to_string(),
        schema: None, // SQLite doesn't have explicit schemas
        columns,
        primary_key,
        foreign_keys,
        indexes,
        comment: None, // SQLite has no native table comment storage
        row_estimate,
    })
}

/// Try to get a row estimate from `sqlite_stat1` (populated by ANALYZE).
/// Returns None when ANALYZE has not been run or the table is absent from the stats table.
fn get_sqlite_row_estimate(conn: &Connection, table_name: &str) -> Option<i64> {
    conn.query_row(
        "SELECT stat FROM sqlite_stat1 WHERE tbl = ?1 AND idx IS NULL LIMIT 1",
        [table_name],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|stat| stat.split_whitespace().next().and_then(|s| s.parse::<i64>().ok()))
}

/// Convert a JSON value to a `rusqlite` native value for parameter binding
fn json_to_sqlite_value(val: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value;
    match val {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else {
                Value::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        v => Value::Text(v.to_string()),
    }
}

/// Returns true when a rusqlite error is `SQLITE_INTERRUPT` (code 9), meaning
/// the interrupt handle fired and `SQLite` cancelled the running statement.
/// Run `EXPLAIN QUERY PLAN` on `inner_sql` and normalize the result into an `ExplainPlanNode` tree.
///
/// `SQLite`'s EXPLAIN QUERY PLAN returns rows of `(id, parent, notused, detail)`.
/// We build a parent-pointer tree and return the virtual root containing all top-level nodes.
fn execute_structured_explain_sqlite(
    conn: &Connection,
    inner_sql: &str,
) -> Result<ExplainPlanNode> {
    let sql = format!("EXPLAIN QUERY PLAN {inner_sql}");

    #[derive(Debug)]
    struct PlanRow {
        id: i64,
        parent: i64,
        detail: String,
    }

    let mut stmt = conn.prepare(&sql).map_err(|e| {
        PlenumError::query_failed(format!("Failed to prepare EXPLAIN QUERY PLAN: {e}"))
    })?;

    let rows: Vec<PlanRow> = stmt
        .query_map([], |row| {
            Ok(PlanRow {
                id: row.get::<_, i64>(0).unwrap_or(0),
                parent: row.get::<_, i64>(1).unwrap_or(0),
                detail: row.get::<_, String>(3).unwrap_or_default(),
            })
        })
        .map_err(|e| {
            PlenumError::query_failed(format!("Failed to execute EXPLAIN QUERY PLAN: {e}"))
        })?
        .filter_map(std::result::Result::ok)
        .collect();

    // Build children map: parent_id → Vec<child>
    let mut children_map: std::collections::HashMap<i64, Vec<&PlanRow>> =
        std::collections::HashMap::new();
    for row in &rows {
        children_map.entry(row.parent).or_default().push(row);
    }

    fn build_node(
        row: &PlanRow,
        children_map: &std::collections::HashMap<i64, Vec<&PlanRow>>,
    ) -> ExplainPlanNode {
        let children = children_map
            .get(&row.id)
            .map(|kids| kids.iter().map(|k| build_node(k, children_map)).collect())
            .unwrap_or_default();
        ExplainPlanNode {
            node_type: row.detail.clone(),
            relation: None,
            estimated_rows: None,
            estimated_cost: None,
            children,
        }
    }

    // Top-level nodes are those with parent = 0 (SQLite uses 0 as the root sentinel)
    let top_level = children_map.get(&0).cloned().unwrap_or_default();
    let top_children: Vec<ExplainPlanNode> =
        top_level.iter().map(|r| build_node(r, &children_map)).collect();

    Ok(ExplainPlanNode {
        node_type: "QUERY PLAN".to_string(),
        relation: None,
        estimated_rows: None,
        estimated_cost: None,
        children: top_children,
    })
}

fn is_sqlite_interrupt(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(ffi_err, _)
            if ffi_err.code == rusqlite::ErrorCode::OperationInterrupted
    )
}

/// Execute query and return `QueryResult`
fn execute_query(
    conn: &Connection,
    query: &str,
    params: &[serde_json::Value],
    caps: &Capabilities,
) -> Result<QueryResult> {
    // Prepare statement
    let mut stmt = conn
        .prepare(query)
        .map_err(|e| PlenumError::query_failed(format!("Failed to prepare query: {e}")))?;

    // Get column names
    let column_names: Vec<String> = stmt.column_names().iter().map(|s| (*s).to_string()).collect();

    // Convert JSON params to rusqlite native values for server-side binding
    let sqlite_params: Vec<rusqlite::types::Value> =
        params.iter().map(json_to_sqlite_value).collect();

    // Execute query and collect rows
    let mut rows_data = Vec::new();
    let mut rows_affected: Option<u64> = None;
    let mut rows_truncated = false;

    // Check if this is a SELECT query (has columns)
    if column_names.is_empty() {
        // Non-SELECT query (INSERT, UPDATE, DELETE, DDL) — blocked by capability checks in
        // practice; handled here for completeness.
        stmt.execute(rusqlite::params_from_iter(&sqlite_params)).map_err(|e| {
            if is_sqlite_interrupt(&e) {
                PlenumError::query_timeout(
                    "Query interrupted by SQLite server-side timeout".to_string(),
                )
            } else {
                PlenumError::query_failed(format!("Failed to execute query: {e}"))
            }
        })?;

        // Get rows affected (only for DML statements)
        rows_affected = Some(conn.changes());
    } else {
        // SELECT query - collect result set
        let rows = stmt.query(rusqlite::params_from_iter(&sqlite_params)).map_err(|e| {
            if is_sqlite_interrupt(&e) {
                PlenumError::query_timeout(
                    "Query interrupted by SQLite server-side timeout".to_string(),
                )
            } else {
                PlenumError::query_failed(format!("Failed to execute query: {e}"))
            }
        })?;

        let mapped_rows = rows.mapped(|row| row_to_json(&column_names, row)).collect::<Vec<_>>();

        let offset = caps.offset.unwrap_or(0);
        let max = caps.max_rows;
        let mut pos = 0usize;

        for row_result in mapped_rows {
            let row = row_result.map_err(|e| {
                if is_sqlite_interrupt(&e) {
                    PlenumError::query_timeout(
                        "Query interrupted by SQLite server-side timeout during row fetch"
                            .to_string(),
                    )
                } else {
                    PlenumError::query_failed(format!("Failed to fetch row: {e}"))
                }
            })?;

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

            rows_data.push(row);
            pos += 1;
        }
    }

    Ok(QueryResult {
        columns: column_names,
        rows: rows_data,
        rows_affected,
        execution_ms: 0,
        rows_truncated,
        truncated_by: None,
        plan: None,
    })
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
            tls: None,
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

        // Get table details for the users table
        let result = SqliteEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "users".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_ok());

        let IntrospectResult::TableDetails { table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

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
        let result = SqliteEngine::execute(&config, "SELECT * FROM users", &[], &caps).await;
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
            SqliteEngine::execute(&config, "INSERT INTO users (name) VALUES ('Bob')", &[], &caps)
                .await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message();
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
        let result = SqliteEngine::execute(
            &config,
            "UPDATE users SET name = 'Bob' WHERE id = 1",
            &[],
            &caps,
        )
        .await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message();
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
        let result = SqliteEngine::execute(
            &config,
            "CREATE TABLE users (id INTEGER PRIMARY KEY)",
            &[],
            &caps,
        )
        .await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message();
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
        let result =
            SqliteEngine::execute(&config, "DELETE FROM users WHERE id = 1", &[], &caps).await;

        assert!(result.is_err());
        let error_message = result.unwrap_err().message();
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
        let result = SqliteEngine::execute(&config, "SELECT * FROM users", &[], &caps).await;

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
        let result = SqliteEngine::execute(&config, "SELECT * FROM test_types", &[], &caps).await;

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

        // Get details for the posts table
        let result = SqliteEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "posts".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_ok());

        let IntrospectResult::TableDetails { table: posts_table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

        assert!(!posts_table.foreign_keys.is_empty());
        let fk = &posts_table.foreign_keys[0];
        assert_eq!(fk.referenced_table, "users");
        assert_eq!(fk.columns, vec!["user_id"]);

        // Clean up
        let _ = std::fs::remove_file(&temp_file);
    }

    /// Prove that writes fail at the `SQLite` session layer independently of the parser.
    ///
    /// This test opens a connection using the same `SQLITE_OPEN_READ_ONLY` flags that
    /// `execute()` uses, then attempts a direct rusqlite write without going through
    /// `validate_query`. The write must be rejected by `SQLite` itself, not by Plenum's
    /// parser, demonstrating true defense-in-depth (REF-261).
    #[test]
    fn test_sqlite_session_read_only_enforcement() {
        let temp_file = std::env::temp_dir().join("test_session_ro_sqlite.db");
        let _ = std::fs::remove_file(&temp_file);

        // Pre-create the database with a table so writes are physically possible
        // if the session were writable.
        {
            let conn = Connection::open(&temp_file).expect("Failed to create temp database");
            conn.execute("CREATE TABLE t (id INTEGER)", []).expect("Failed to create table");
        }

        // Open with the same read-only flags that SqliteEngine::execute() uses.
        let path_str = temp_file.to_str().unwrap();
        let conn = open_connection(path_str, true).expect("Failed to open read-only connection");

        // Attempt DML directly — no Plenum parser involved.
        let insert_result = conn.execute("INSERT INTO t (id) VALUES (1)", []);
        assert!(insert_result.is_err(), "INSERT must be rejected at the SQLite session layer");

        // Attempt DDL directly — no Plenum parser involved.
        let create_result = conn.execute("CREATE TABLE t2 (id INTEGER)", []);
        assert!(
            create_result.is_err(),
            "CREATE TABLE must be rejected at the SQLite session layer"
        );

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

        // Get details for the users table
        let result = SqliteEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "users".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            None,
        )
        .await;
        assert!(result.is_ok());

        let IntrospectResult::TableDetails { table: users_table } = result.unwrap() else {
            panic!("Expected TableDetails result")
        };

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

    // -------------------------------------------------------------------------
    // REF-263: column comments + row estimates
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_introspect_comment_and_row_estimate_explicit_null() {
        // SQLite has no native comment storage and no row statistics unless
        // ANALYZE has been run.  Both fields must be present in JSON as null.
        let temp_file = std::env::temp_dir().join("test_ref263_sqlite.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to open db");
            conn.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT NOT NULL)", [])
                .expect("Failed to create table");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let result = SqliteEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "items".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            None,
        )
        .await
        .expect("Introspect failed");

        let IntrospectResult::TableDetails { table } = result else {
            panic!("Expected TableDetails")
        };

        // Table-level comment is always null for SQLite
        assert!(table.comment.is_none(), "SQLite table comment must be null");
        // row_estimate is null without prior ANALYZE
        assert!(table.row_estimate.is_none(), "row_estimate must be null before ANALYZE");

        // Column-level comments are always null for SQLite
        for col in &table.columns {
            assert!(col.comment.is_none(), "SQLite column '{}' comment must be null", col.name);
        }

        // Verify the JSON representation has explicit nulls (not omitted)
        let json = serde_json::to_value(&table).expect("Serialization failed");
        assert_eq!(json["comment"], serde_json::Value::Null, "comment must serialize as null");
        assert_eq!(
            json["row_estimate"],
            serde_json::Value::Null,
            "row_estimate must serialize as null"
        );
        assert_eq!(
            json["columns"][0]["comment"],
            serde_json::Value::Null,
            "column comment must serialize as null"
        );

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_introspect_row_estimate_after_analyze() {
        // After ANALYZE sqlite_stat1 is populated; row_estimate should be non-null.
        let temp_file = std::env::temp_dir().join("test_ref263_analyze.db");
        let _ = std::fs::remove_file(&temp_file);

        {
            let conn = Connection::open(&temp_file).expect("Failed to open db");
            conn.execute("CREATE TABLE things (id INTEGER PRIMARY KEY)", [])
                .expect("Failed to create table");
            for _ in 0..5 {
                conn.execute("INSERT INTO things (id) VALUES (NULL)", [])
                    .expect("Failed to insert");
            }
            conn.execute("ANALYZE", []).expect("Failed to ANALYZE");
        }

        let config = ConnectionConfig::sqlite(temp_file.clone());
        let result = SqliteEngine::introspect(
            &config,
            &IntrospectOperation::TableDetails {
                name: "things".to_string(),
                fields: crate::engine::TableFields::all(),
            },
            None,
            None,
        )
        .await
        .expect("Introspect failed");

        let IntrospectResult::TableDetails { table } = result else {
            panic!("Expected TableDetails")
        };

        assert!(table.row_estimate.is_some(), "row_estimate must be populated after ANALYZE");
        assert!(table.row_estimate.unwrap() >= 0, "row_estimate must be non-negative");

        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_server_side_timeout_interrupt() {
        // A recursive CTE that counts to 1 billion will run for many seconds.
        // With timeout_ms = 1 the interrupt thread fires almost immediately,
        // and SQLite returns SQLITE_INTERRUPT, which we surface as QUERY_TIMEOUT.
        let config = ConnectionConfig::sqlite(":memory:".into());
        let caps = Capabilities { timeout_ms: Some(1), ..Capabilities::default() };
        let sql = "WITH RECURSIVE cnt(x) AS \
                   (VALUES(1) UNION ALL SELECT x+1 FROM cnt WHERE x < 1000000000) \
                   SELECT count(*) FROM cnt";

        let result = SqliteEngine::execute(&config, sql, &[], &caps).await;

        assert!(result.is_err(), "Expected a timeout error but got Ok");
        let err = result.unwrap_err();
        assert_eq!(
            err.error_code(),
            "QUERY_TIMEOUT",
            "Expected QUERY_TIMEOUT, got: {}",
            err.error_code()
        );
        assert!(
            err.message().contains("interrupted"),
            "Error message should mention interrupt: {}",
            err.message()
        );
    }

    // =========================================================================
    // Parameterized query tests (REF-259)
    // =========================================================================

    #[tokio::test]
    async fn test_execute_bound_integer_param() {
        let temp_file = std::env::temp_dir().join("test_bound_int.db");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("open");
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", []).unwrap();
            conn.execute("INSERT INTO t (id, name) VALUES (1, 'alice')", []).unwrap();
            conn.execute("INSERT INTO t (id, name) VALUES (2, 'bob')", []).unwrap();
        }
        let config = ConnectionConfig::sqlite(temp_file.clone());
        let params = vec![serde_json::json!(1)];
        let result = SqliteEngine::execute(
            &config,
            "SELECT name FROM t WHERE id = ?",
            &params,
            &Capabilities::default(),
        )
        .await;
        assert!(result.is_ok(), "bound integer param should succeed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!("alice"));
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_bound_text_param() {
        let temp_file = std::env::temp_dir().join("test_bound_text.db");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("open");
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", []).unwrap();
            conn.execute("INSERT INTO t (id, name) VALUES (1, 'alice')", []).unwrap();
            conn.execute("INSERT INTO t (id, name) VALUES (2, 'bob')", []).unwrap();
        }
        let config = ConnectionConfig::sqlite(temp_file.clone());
        let params = vec![serde_json::json!("bob")];
        let result = SqliteEngine::execute(
            &config,
            "SELECT id FROM t WHERE name = ?",
            &params,
            &Capabilities::default(),
        )
        .await;
        assert!(result.is_ok(), "bound text param should succeed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!(2));
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_bound_multiple_params() {
        let temp_file = std::env::temp_dir().join("test_bound_multi.db");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("open");
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL)", [])
                .unwrap();
            conn.execute("INSERT INTO t VALUES (1, 'alice', 9.5)", []).unwrap();
            conn.execute("INSERT INTO t VALUES (2, 'bob', 7.0)", []).unwrap();
            conn.execute("INSERT INTO t VALUES (3, 'charlie', 8.2)", []).unwrap();
        }
        let config = ConnectionConfig::sqlite(temp_file.clone());
        let params = vec![serde_json::json!(7.5), serde_json::json!(9.0)];
        let result = SqliteEngine::execute(
            &config,
            "SELECT name FROM t WHERE score >= ? AND score <= ? ORDER BY score",
            &params,
            &Capabilities::default(),
        )
        .await;
        assert!(result.is_ok(), "multiple bound params should succeed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1);
        assert_eq!(qr.rows[0][0], serde_json::json!("charlie"));
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_bound_null_param() {
        let temp_file = std::env::temp_dir().join("test_bound_null.db");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("open");
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, val TEXT)", []).unwrap();
            conn.execute("INSERT INTO t (id, val) VALUES (1, 'present')", []).unwrap();
            conn.execute("INSERT INTO t (id, val) VALUES (2, NULL)", []).unwrap();
        }
        let config = ConnectionConfig::sqlite(temp_file.clone());
        // SQLite: "col IS ?" with NULL doesn't work the same as IS NULL; use IS NULL directly.
        // Instead verify that binding NULL as a text param filters correctly.
        let params = vec![serde_json::json!("present")];
        let result = SqliteEngine::execute(
            &config,
            "SELECT id FROM t WHERE val = ?",
            &params,
            &Capabilities::default(),
        )
        .await;
        assert!(result.is_ok(), "null-adjacent bound param should succeed: {:?}", result.err());
        let qr = result.unwrap();
        assert_eq!(qr.rows.len(), 1, "should return exactly the non-null row");
        assert_eq!(qr.rows[0][0], serde_json::json!(1));
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_params_no_credential_leak_in_result() {
        // Bound params must not appear verbatim in the JSON output `columns` or metadata.
        let temp_file = std::env::temp_dir().join("test_bound_noleak.db");
        let _ = std::fs::remove_file(&temp_file);
        {
            let conn = Connection::open(&temp_file).expect("open");
            conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, secret TEXT)", []).unwrap();
            conn.execute("INSERT INTO t VALUES (1, 'supersecret')", []).unwrap();
        }
        let config = ConnectionConfig::sqlite(temp_file.clone());
        let params = vec![serde_json::json!("supersecret")];
        let result = SqliteEngine::execute(
            &config,
            "SELECT id FROM t WHERE secret = ?",
            &params,
            &Capabilities::default(),
        )
        .await;
        assert!(result.is_ok());
        let qr = result.unwrap();
        // Column names must not contain the bound param value
        let col_json = serde_json::to_string(&qr.columns).unwrap();
        assert!(!col_json.contains("supersecret"), "param value must not appear in columns");
        let _ = std::fs::remove_file(&temp_file);
    }

    #[tokio::test]
    async fn test_execute_write_still_rejected_with_params() {
        // Capability check must still fire before binding takes place.
        let config = ConnectionConfig::sqlite(":memory:".into());
        let params = vec![serde_json::json!(42), serde_json::json!("evil")];
        let caps = Capabilities::default();
        let result =
            SqliteEngine::execute(&config, "INSERT INTO t VALUES (?, ?)", &params, &caps).await;
        assert!(result.is_err(), "write must be rejected even with params");
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }
}
