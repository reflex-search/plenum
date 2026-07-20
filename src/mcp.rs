//! MCP (Model Context Protocol) Server
//!
//! This module implements an MCP server using manual JSON-RPC 2.0 over stdio.
//! We follow the proven pattern from reflex-search rather than using the unstable rmcp crate.
//!
//! # Architecture
//!
//! - **Transport**: JSON-RPC 2.0 over stdio (line-based)
//! - **Dependencies**: Only `serde_json` and anyhow (no MCP-specific crates)
//! - **Protocol**: Implements MCP specification manually
//!
//! # Design Principles
//!
//! 1. **Stateless**: Each tool invocation is completely independent
//! 2. **Simple**: Direct JSON-RPC implementation, no macro magic
//! 3. **Debuggable**: Easy to understand and troubleshoot
//! 4. **Reusable**: All tools call existing library functions
//!
//! # MCP Tools
//!
//! - `introspect` - Introspect database schema
//! - `query` - Execute constrained SQL queries
//!
//! Connection management is handled via the `plenum connect` CLI command or by
//! directly editing configuration files (`.plenum/config.json` or `~/.config/plenum/connections.json`).
//!
//! # Usage
//!
//! Start the MCP server with: `plenum mcp`
//!
//! Configure in Claude Desktop:
//! ```json
//! {
//!   "mcpServers": {
//!     "plenum": {
//!       "command": "plenum",
//!       "args": ["mcp"]
//!     }
//!   }
//! }
//! ```

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::config::{ConfigLocation, KeychainEntry, StoredConnection};
use crate::{parse_dsn, redact_dsn, Capabilities, ConnectionConfig, DatabaseEngine, DatabaseType};

// Import database engines
#[cfg(feature = "duckdb")]
use crate::engine::duckdb::DuckDbEngine;
#[cfg(feature = "mysql")]
use crate::engine::mysql::MySqlEngine;
#[cfg(feature = "postgres")]
use crate::engine::postgres::PostgresEngine;
#[cfg(feature = "sqlite")]
use crate::engine::sqlite::SqliteEngine;

// ============================================================================
// Connection Binding (captured at server registration)
// ============================================================================

/// Connection binding captured from `plenum mcp` flags at server registration.
///
/// The MCP server is long-lived and its launcher's cwd is not a meaningful
/// signal (Claude Desktop, editors, and daemons start it from arbitrary
/// directories). This binding pins config resolution to an explicit source so
/// resolution never silently depends on cwd.
///
/// Per-call tool arguments (`dsn`, `connection`, `engine`, ...) still take
/// precedence; the binding only supplies the default a call omits.
#[derive(Debug, Clone, Default)]
pub struct McpBinding {
    /// `--project-path`: pins config resolution to this project path.
    project_path: Option<String>,
    /// `--name`: pins to a named connection within the resolved project.
    name: Option<String>,
    /// `--dsn-env`: reads the DSN from this named environment variable at
    /// connection time. Only the named variable is ever read — never an ambient
    /// fallback such as `DATABASE_URL` or `PGPASSWORD`.
    dsn_env: Option<String>,
}

impl McpBinding {
    /// Construct a binding from `plenum mcp` flags, rejecting conflicting
    /// combinations.
    ///
    /// `--dsn-env` names a complete one-off connection source, so it cannot be
    /// combined with the saved-config selectors `--project-path` / `--name`.
    ///
    /// # Errors
    /// Returns an error when `--dsn-env` is combined with `--project-path` or
    /// `--name`.
    pub fn new(
        project_path: Option<String>,
        name: Option<String>,
        dsn_env: Option<String>,
    ) -> Result<Self> {
        if dsn_env.is_some() && (project_path.is_some() || name.is_some()) {
            return Err(anyhow!(
                "'--dsn-env' cannot be combined with '--project-path' or '--name': \
                 a DSN environment variable is a complete connection source, \
                 not a saved-config selector"
            ));
        }
        Ok(Self { project_path, name, dsn_env })
    }
}

// ============================================================================
// JSON-RPC 2.0 Structures
// ============================================================================

/// JSON-RPC 2.0 Request
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    // Per JSON-RPC 2.0, Notifications must omit `id` entirely (not set it to null).
    // We never construct a JsonRpcResponse for a Notification, but skip_serializing_if
    // is a defensive guard against ever emitting `"id": null`, which strict clients
    // (e.g. Claude Code's Zod validators) reject.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 Error
#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// ============================================================================
// MCP Tool Result Structures
// ============================================================================

/// Text content block for MCP tool results
#[derive(Debug, Serialize)]
struct TextContent {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

impl TextContent {
    /// Create a new text content block
    fn new(text: String) -> Self {
        Self { content_type: "text".to_string(), text }
    }
}

/// MCP tool call result
#[derive(Debug, Serialize)]
struct CallToolResult {
    content: Vec<TextContent>,
    #[serde(rename = "isError")]
    is_error: bool,
}

impl CallToolResult {
    /// Create a successful tool result with JSON data
    fn success(data: impl Serialize) -> Result<Value> {
        // Serialize data to pretty JSON string
        let json_text = serde_json::to_string_pretty(&data)?;

        let result = Self { content: vec![TextContent::new(json_text)], is_error: false };

        Ok(serde_json::to_value(result)?)
    }
}

// ============================================================================
// MCP Server
// ============================================================================

/// Start the MCP server
///
/// This function runs the main MCP server loop, reading JSON-RPC requests
/// from stdin and writing JSON-RPC responses to stdout.
///
/// # Protocol
///
/// The server implements JSON-RPC 2.0 over stdio:
/// - Each request is a single line of JSON
/// - Each response is a single line of JSON
/// - Errors are returned as JSON-RPC error responses
///
/// # Errors
///
/// Returns an error if stdio communication fails or if there's a fatal error.
#[allow(clippy::future_not_send)]
pub async fn serve(binding: McpBinding) -> Result<()> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let mut stdout = io::stdout();

    for line in reader.lines() {
        let line = line?;

        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON-RPC request. Unparseable input is silently skipped: we have
        // no id to attach to a response, and emitting one with id: null violates
        // strict JSON-RPC 2.0 validators (e.g. the MCP TypeScript SDK's Zod schemas).
        let Ok(request) = serde_json::from_str::<JsonRpcRequest>(&line) else {
            continue;
        };

        // JSON-RPC 2.0: a Notification has no id and MUST NOT receive a response.
        // The MCP handshake sends `notifications/initialized` between `initialize`
        // and `tools/list`; replying to it breaks strict clients.
        if request.id.is_none() {
            continue;
        }

        let response = handle_request(request, &binding).await;
        let response_json = serde_json::to_string(&response)?;
        writeln!(stdout, "{response_json}")?;
        stdout.flush()?;
    }

    Ok(())
}

/// Handle a JSON-RPC request
///
/// Routes the request to the appropriate handler based on the method name.
async fn handle_request(request: JsonRpcRequest, binding: &McpBinding) -> JsonRpcResponse {
    let result = match request.method.as_str() {
        "initialize" => handle_initialize(request.params),
        "tools/list" => handle_list_tools(),
        "tools/call" => handle_call_tool(request.params, binding).await,
        _ => Err(anyhow!("Unknown method: {}", request.method)),
    };

    match result {
        Ok(value) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: Some(value),
            error: None,
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603, // Internal error
                message: e.to_string(),
                data: None,
            }),
        },
    }
}

// ============================================================================
// MCP Protocol Handlers
// ============================================================================

/// Handle MCP initialize request
///
/// Returns server capabilities and metadata.
fn handle_initialize(_params: Option<Value>) -> Result<Value> {
    Ok(serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": "plenum",
            "version": env!("CARGO_PKG_VERSION")
        }
    }))
}

/// Handle tools/list request
///
/// Returns the list of available MCP tools with their schemas.
fn handle_list_tools() -> Result<Value> {
    Ok(serde_json::json!({
        "tools": [
            {
                "name": "introspect",
                "description": "Introspect database schema with granular operations. NEVER dumps entire schema - requires explicit operation. IMPORTANT CONNECTION WORKFLOW: (1) RECOMMENDED: Auto-resolve (omit all connection params) - uses project's default saved connection, (2) COMMON: Named connection (use 'connection' param only) - references saved connection by name, (3) DISCOURAGED: Explicit credentials (engine + host/user/password) - ONLY for one-off scenarios, NOT for regular use. DO NOT pass credentials repeatedly - use saved connections instead. Before using explicit credentials, check if a saved connection exists. Operations (EXACTLY ONE required, mutually exclusive): list_databases (list all DBs), list_schemas (Postgres only), list_tables (table names in schema/DB), list_views (view names), list_indexes (all or filtered by table), table (full details for specific table with optional field filtering), view (view definition + columns), diff_against (structural schema diff between two named connections - returns {data:{diff:{tables_added,tables_removed,tables_changed,views_added,views_removed,views_changed}}}). Optional modifiers: 'target_database' (switch to different DB before introspecting - Postgres/MySQL only), 'schema' (filter to specific schema - Postgres/MySQL only). Returns typed JSON specific to operation (DatabaseList, SchemaList, TableList, ViewList, IndexList, TableDetails, or ViewDetails). Stateless - connection opened, operation executed, connection closed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "dsn": {
                            "type": "string",
                            "description": "One-off connection DSN/URL. Use when you have a full connection string and no saved connection exists. Mutually exclusive with 'connection' and 'engine'. Accepted schemes: postgres://, postgresql://, mysql://, sqlite:. Credentials are redacted from any error output. Example: 'postgres://user:pass@host:5432/db'. Config is never written."
                        },
                        "connection": {
                            "type": "string",
                            "description": "RECOMMENDED: Name of saved connection to use. Loads from .plenum/config.json (local) or ~/.config/plenum/connections.json (global). If omitted along with 'engine', auto-resolves project's default connection (BEST PRACTICE)."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite", "duckdb"],
                            "description": "DISCOURAGED: Database engine type for explicit one-off connections. Only use if no saved connection exists. If omitted along with 'connection', auto-resolves project's default connection (RECOMMENDED)."
                        },
                        "host": {
                            "type": "string",
                            "description": "DISCOURAGED: Database host (postgres/mysql). Only for one-off explicit connections or as named connection override. Prefer using saved connections instead."
                        },
                        "port": {
                            "type": "number",
                            "description": "DISCOURAGED: Database port (postgres/mysql). Only for one-off explicit connections or as override. Defaults: postgres=5432, mysql=3306. Prefer using saved connections."
                        },
                        "user": {
                            "type": "string",
                            "description": "DISCOURAGED: Database username (postgres/mysql). Only for one-off explicit connections or as override. DO NOT pass repeatedly - use saved connections instead."
                        },
                        "password": {
                            "type": "string",
                            "description": "DISCOURAGED: Database password (postgres/mysql). Only for one-off explicit connections or as override. DO NOT pass repeatedly - use saved connections instead."
                        },
                        "database": {
                            "type": "string",
                            "description": "DISCOURAGED: Database name (postgres/mysql). Only for one-off explicit connections or as override. Use \"*\" for wildcard mode to enable list_databases operation. Prefer using saved connections."
                        },
                        "file": {
                            "type": "string",
                            "description": "DISCOURAGED: SQLite/DuckDB database file path. Only for one-off sqlite/duckdb explicit connections or as override. Prefer using saved connections."
                        },
                        "password_env": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: name of the environment variable holding the password. The secret value never passes through Plenum. Combine with engine + host/port/user/database. Mutually exclusive with password_command and keychain_service/keychain_account."
                        },
                        "password_command": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: shell command whose stdout (trimmed) is the password. Mutually exclusive with password_env and keychain_service/keychain_account."
                        },
                        "keychain_service": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: OS keychain service name. Must be paired with keychain_account. Mutually exclusive with password_env and password_command."
                        },
                        "keychain_account": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: OS keychain account name. Must be paired with keychain_service."
                        },
                        "list_databases": {
                            "type": "boolean",
                            "description": "Operation: List all databases. Returns {\"type\": \"database_list\", \"databases\": [\"db1\", \"db2\", ...]}. Requires wildcard connection (database=\"*\"). MySQL/Postgres only. Mutually exclusive with other operations."
                        },
                        "list_schemas": {
                            "type": "boolean",
                            "description": "Operation: List all schemas in current database. Returns {\"type\": \"schema_list\", \"schemas\": [\"public\", ...]}. PostgreSQL only (MySQL: schema=database, SQLite: no schemas). Mutually exclusive with other operations."
                        },
                        "list_tables": {
                            "type": "boolean",
                            "description": "Operation: List all table names. Returns {\"type\": \"table_list\", \"tables\": [\"users\", \"posts\", ...]}. Use 'schema' to filter (Postgres/MySQL). Most common operation. Mutually exclusive with other operations."
                        },
                        "list_views": {
                            "type": "boolean",
                            "description": "Operation: List all view names. Returns {\"type\": \"view_list\", \"views\": [\"active_users\", ...]}. Use 'schema' to filter (Postgres/MySQL). Mutually exclusive with other operations."
                        },
                        "list_indexes": {
                            "type": "string",
                            "description": "Operation: List all indexes (all tables or filtered by table name). Pass table name as value to filter, or empty string for all. Returns {\"type\": \"index_list\", \"indexes\": [{\"name\": \"idx_email\", \"table\": \"users\", \"unique\": true, \"columns\": [\"email\"]}, ...]}. Mutually exclusive with other operations."
                        },
                        "table": {
                            "type": "string",
                            "description": "Operation: Get full details for specific table (name as value). Returns {\"type\": \"table_details\", \"table\": {\"name\": \"users\", \"columns\": [...], \"primary_key\": [...], \"foreign_keys\": [...], \"indexes\": [...]}}. Use field selectors (columns, primary_key, foreign_keys, indexes) to filter returned fields. Mutually exclusive with other operations."
                        },
                        "view": {
                            "type": "string",
                            "description": "Operation: Get view definition and columns (name as value). Returns {\"type\": \"view_details\", \"view\": {\"name\": \"...\", \"definition\": \"CREATE VIEW ...\", \"columns\": [...]}}. Mutually exclusive with other operations."
                        },
                        "target_database": {
                            "type": "string",
                            "description": "Optional modifier: Switch to different database before introspecting. Reconnects with different DB. Postgres/MySQL only (SQLite uses different files). Example: introspect 'production' DB tables while default connection points to 'staging'."
                        },
                        "schema": {
                            "type": "string",
                            "description": "Optional modifier: Filter results to specific schema. Works with list_tables, list_views, list_indexes, table, view operations. Postgres/MySQL only (SQLite has no schemas). Defaults to current schema if omitted."
                        },
                        "columns": {
                            "type": "boolean",
                            "description": "Table field selector: Include columns in table details. Only applies to 'table' operation. Default: true."
                        },
                        "primary_key": {
                            "type": "boolean",
                            "description": "Table field selector: Include primary key in table details. Only applies to 'table' operation. Default: true."
                        },
                        "foreign_keys": {
                            "type": "boolean",
                            "description": "Table field selector: Include foreign keys in table details. Only applies to 'table' operation. Default: true."
                        },
                        "indexes": {
                            "type": "boolean",
                            "description": "Table field selector: Include indexes in table details. Only applies to 'table' operation. Default: true. Note: This filters table detail fields, while 'list_indexes' is a separate operation."
                        },
                        "diff_against": {
                            "type": "string",
                            "description": "Operation: Structural schema diff. Pass the name of a saved connection to compare against the current (base) connection. Returns {\"diff\": {\"tables_added\": [...], \"tables_removed\": [...], \"tables_changed\": [...], \"views_added\": [...], \"views_removed\": [...], \"views_changed\": [...]}}. All arrays sorted alphabetically; identical schemas return all-empty arrays. Mutually exclusive with all other operations."
                        },
                        "diff_against_project_path": {
                            "type": "string",
                            "description": "Optional modifier for diff_against: project path to look up the diff-against connection in. Defaults to the current project path. Use for cross-project schema comparison."
                        }
                    }
                }
            },
            {
                "name": "query",
                "description": "Execute READ-ONLY SQL queries. **PLENUM IS STRICTLY READ-ONLY** - it will REJECT any write or DDL operations (INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, etc.). When you need to modify data or schema: (1) Use Plenum to introspect the schema and read current data, (2) Construct the appropriate SQL query, (3) Present the query to the user in your response for them to execute manually. NEVER attempt to execute write operations through Plenum - they will always fail. IMPORTANT SECURITY: You (the AI agent) are responsible for sanitizing all user inputs before constructing SQL - Plenum does NOT validate SQL safety. IMPORTANT CONNECTION WORKFLOW: (1) RECOMMENDED: Auto-resolve (omit all connection params) - uses project's default saved connection, (2) COMMON: Named connection (use 'connection' param only) - references saved connection by name, (3) DISCOURAGED: Explicit credentials (engine + host/user/password) - ONLY for one-off scenarios, NOT for regular use. DO NOT pass credentials repeatedly - use saved connections instead. Typical pattern: call 'connect' tool once to save credentials, then use 'query' with auto-resolution or connection name for all subsequent queries. CRITICAL MCP TOKEN LIMITS: MCP responses are limited to 25,000 tokens. Large result sets will cause complete tool failure. ALWAYS use max_rows parameter unless you are certain the table is tiny (< 10 rows). Recommended values: max_rows=10 for initial exploration, max_rows=50-100 for small known tables, max_rows=500+ only after verifying table size. Queries without max_rows on unknown tables will likely fail. Use timeout_ms to prevent long-running operations. Returns JSON with query results (rows/columns). The connection is opened, query is executed, and connection is immediately closed (stateless). Possible error codes: CAPABILITY_VIOLATION (attempted write/DDL operation), QUERY_FAILED (SQL error), CONNECTION_FAILED (connection error).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL query to execute. REQUIRED. Must be valid, vendor-specific SQL (PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL). You (the agent) are responsible for sanitizing user inputs before constructing SQL - Plenum does not validate SQL safety."
                        },
                        "dsn": {
                            "type": "string",
                            "description": "One-off connection DSN/URL. Use when you have a full connection string and no saved connection exists. Mutually exclusive with 'connection' and 'engine'. Accepted schemes: postgres://, postgresql://, mysql://, sqlite:. Credentials are redacted from any error output. Example: 'postgres://user:pass@host:5432/db'. Config is never written."
                        },
                        "connection": {
                            "type": "string",
                            "description": "RECOMMENDED: Name of saved connection to use. Connection must exist in local (.plenum/config.json) or global (~/.config/plenum/connections.json) config. If omitted along with 'engine', auto-resolves to project's default connection (BEST PRACTICE)."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite", "duckdb"],
                            "description": "DISCOURAGED: Database engine type for explicit one-off connections. Only use if no saved connection exists. Valid values: 'postgres', 'mysql', 'sqlite', 'duckdb'. If omitted along with 'connection', auto-resolves project's default connection (RECOMMENDED)."
                        },
                        "host": {
                            "type": "string",
                            "description": "DISCOURAGED: Database host (for postgres/mysql). Only for one-off explicit connections or as override. DO NOT pass repeatedly - use saved connections instead. Example: 'localhost', 'db.example.com'."
                        },
                        "port": {
                            "type": "number",
                            "description": "DISCOURAGED: Database port (for postgres/mysql). Only for one-off explicit connections or as override. Defaults: postgres=5432, mysql=3306. Prefer using saved connections."
                        },
                        "user": {
                            "type": "string",
                            "description": "DISCOURAGED: Database username (for postgres/mysql). Only for one-off explicit connections or as override. DO NOT pass repeatedly - use saved connections instead."
                        },
                        "password": {
                            "type": "string",
                            "description": "DISCOURAGED: Database password (for postgres/mysql). Only for one-off explicit connections or as override. DO NOT pass repeatedly - use saved connections instead. Passed directly - agent responsible for security."
                        },
                        "database": {
                            "type": "string",
                            "description": "DISCOURAGED: Database name (for postgres/mysql). Only for one-off explicit connections or as override. The specific database to query. Use \"*\" for wildcard mode to query system catalogs (SHOW DATABASES in MySQL, pg_catalog.pg_database in PostgreSQL) or use fully qualified table names. Prefer using saved connections."
                        },
                        "file": {
                            "type": "string",
                            "description": "DISCOURAGED: File path to SQLite/DuckDB database file. Only for one-off sqlite/duckdb explicit connections. Can be relative or absolute path. Example: './app.db', '/var/lib/data.duckdb'. Prefer using saved connections."
                        },
                        "password_env": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: name of the environment variable holding the password. The secret value never passes through Plenum. Combine with engine + host/port/user/database. Mutually exclusive with password_command and keychain_service/keychain_account."
                        },
                        "password_command": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: shell command whose stdout (trimmed) is the password. Mutually exclusive with password_env and keychain_service/keychain_account."
                        },
                        "keychain_service": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: OS keychain service name. Must be paired with keychain_account. Mutually exclusive with password_env and password_command."
                        },
                        "keychain_account": {
                            "type": "string",
                            "description": "Credential reference for one-off explicit connections: OS keychain account name. Must be paired with keychain_service."
                        },
                        "max_rows": {
                            "type": "number",
                            "description": "CRITICAL: Maximum number of rows to return from SELECT queries. Due to MCP's 25k token response limit, this parameter is effectively REQUIRED for all queries against tables of unknown size. Omitting this will cause tool failure on large tables. Start small and increase if needed: Use 10 for initial exploration/preview, 50-100 for small known tables, 100-500 for medium tables (only after confirming size with COUNT(*) query). Even with columnar format (30-50% token reduction), a 100-row result with 10+ columns can approach token limits. Always prefer smaller limits initially."
                        },
                        "max_bytes": {
                            "type": "number",
                            "description": "Optional: Maximum serialized byte size of the rows array. Truncates at row boundaries so partial rows are never returned. When triggered, the response includes rows_truncated:true and truncated_by:'bytes' in the meta section. Useful for tables with wide columns (BLOBs, large JSON) where max_rows alone may not bound the response size. Example: 50000 (50 KB)."
                        },
                        "timeout_ms": {
                            "type": "number",
                            "description": "Optional: Query execution timeout in milliseconds. Recommended for potentially expensive queries to prevent long-running operations. Example: 5000 (5 seconds). No timeout if omitted."
                        },
                        "target_database": {
                            "type": "string",
                            "description": "Optional modifier: Switch to different database before executing query. Reconnects with different DB. Postgres/MySQL only (SQLite uses different files). Example: query 'production' DB while default connection points to 'staging'. This parameter overrides the database specified in the connection config."
                        },
                        "time_only": {
                            "type": "boolean",
                            "description": "Optional: Return only execution timing information (excludes result data). Useful for benchmarking queries without consuming MCP response tokens. The query still executes fully to measure realistic performance. Returns execution_ms and rows_matched count instead of full result set. Default: false."
                        },
                        "explain_format": {
                            "type": "string",
                            "enum": ["native", "structured"],
                            "description": "Optional: EXPLAIN output format. 'native' (default) returns raw engine rows unchanged. 'structured' requires the SQL to be an EXPLAIN statement and returns data.plan — a normalized, engine-stable JSON tree with node_type, relation, estimated_rows, estimated_cost, and children. Engine-absent fields are explicit null. Non-EXPLAIN queries with 'structured' are rejected with INVALID_INPUT."
                        }
                    },
                    "required": ["sql"]
                }
            },
            {
                "name": "connect",
                "description": "Test a database connection OR save a connection config by reference. TWO MODES: (A) TEST (default, no 'save'): opens a connection, verifies liveness, returns ConnectionInfo, then disconnects. Stateless — no config mutated. (B) SAVE ('save': \"local\"|\"global\"): persists the connection config BY REFERENCE to .plenum/config.json (local) or ~/.config/plenum/connections.json (global), then returns a saved confirmation. No live connection required — provisions connections offline. CREDENTIAL SAFETY (non-negotiable): inline plaintext 'password' is REJECTED with CAPABILITY_VIOLATION. Source secrets ONLY by reference: 'password_env' (env var name), 'password_command' (shell command), or 'keychain_service'+'keychain_account' (OS keychain). The plaintext secret NEVER passes through Plenum — only the reference string is stored. Use TEST mode to health-check a saved connection; use SAVE mode to register a new one. Possible error codes: CAPABILITY_VIOLATION (inline plaintext password), CONNECTION_FAILED (unreachable host, bad credentials, missing file), INVALID_INPUT (missing required params), CONFIG_ERROR (no saved connection found).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection": {
                            "type": "string",
                            "description": "RECOMMENDED: Name of saved connection to test. Loads from .plenum/config.json (local) or ~/.config/plenum/connections.json (global). If omitted along with 'engine', auto-resolves project's default connection (BEST PRACTICE)."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite", "duckdb"],
                            "description": "DISCOURAGED: Database engine type for explicit one-off connection tests. Only use if no saved connection exists."
                        },
                        "host": {
                            "type": "string",
                            "description": "DISCOURAGED: Database host (postgres/mysql). Only for explicit one-off tests."
                        },
                        "port": {
                            "type": "number",
                            "description": "DISCOURAGED: Database port (postgres/mysql). Defaults: postgres=5432, mysql=3306."
                        },
                        "user": {
                            "type": "string",
                            "description": "DISCOURAGED: Database username (postgres/mysql). Only for explicit one-off tests."
                        },
                        "password": {
                            "type": "string",
                            "description": "DISCOURAGED: Database password (postgres/mysql). Only for explicit one-off tests."
                        },
                        "database": {
                            "type": "string",
                            "description": "DISCOURAGED: Database name (postgres/mysql). Only for explicit one-off tests."
                        },
                        "file": {
                            "type": "string",
                            "description": "DISCOURAGED: SQLite/DuckDB database file path. Only for explicit one-off tests."
                        },
                        "password_env": {
                            "type": "string",
                            "description": "Credential reference: name of the environment variable that holds the password (e.g. \"DB_PASSWORD\"). The reference string is stored/used, never the secret value. Mutually exclusive with password_command and keychain_service/keychain_account. Use this instead of inline 'password' (which is rejected)."
                        },
                        "password_command": {
                            "type": "string",
                            "description": "Credential reference: shell command whose stdout (trimmed) is the password (e.g. \"op read op://vault/db/password\"). The command string is stored/used, never the secret value. Mutually exclusive with password_env and keychain_service/keychain_account."
                        },
                        "keychain_service": {
                            "type": "string",
                            "description": "Credential reference: OS keychain service name. Must be paired with keychain_account. The password is looked up from the platform keychain at connection time. Mutually exclusive with password_env and password_command."
                        },
                        "keychain_account": {
                            "type": "string",
                            "description": "Credential reference: OS keychain account name. Must be paired with keychain_service."
                        },
                        "save": {
                            "type": "string",
                            "enum": ["local", "global"],
                            "description": "SAVE MODE: persist this connection config by reference. \"local\" writes .plenum/config.json (team-shareable, per-project); \"global\" writes ~/.config/plenum/connections.json (per-user). Requires explicit 'engine' + connection params. Credentials are stored only as references (password_env/password_command/keychain_*) — never plaintext. Omit to run in TEST mode instead."
                        },
                        "project_path": {
                            "type": "string",
                            "description": "SAVE MODE modifier: project path to key the saved connection under. Defaults to the server's bound --project-path, or the current working directory. Local saves are written to <project_path>/.plenum/config.json."
                        },
                        "name": {
                            "type": "string",
                            "description": "SAVE MODE modifier: name to store the connection under (e.g. \"prod\", \"staging\"). Defaults to \"default\". The first connection saved for a project becomes its default."
                        }
                    }
                }
            }
        ]
    }))
}

/// Handle tools/call request
///
/// Routes the tool call to the appropriate tool implementation.
async fn handle_call_tool(params: Option<Value>, binding: &McpBinding) -> Result<Value> {
    let params = params.ok_or_else(|| anyhow!("Missing params"))?;
    let name = params["name"].as_str().ok_or_else(|| anyhow!("Missing tool name"))?;
    let arguments = &params["arguments"];

    match name {
        "connect" => tool_connect(arguments, binding).await,
        "introspect" => tool_introspect(arguments, binding).await,
        "query" => tool_query(arguments, binding).await,
        _ => Err(anyhow!("Unknown tool: {name}")),
    }
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// MCP Tool: connect
///
/// Two modes:
/// - Without `save`: tests a database connection and returns server metadata.
///   Stateless — no config is mutated.
/// - With `save` (`"local"` | `"global"`): persists the connection config **by
///   reference** to `.plenum/config.json` or the global registry, then returns a
///   saved confirmation. No live connection is required, so agents can provision
///   connections offline.
///
/// Inline plaintext passwords are rejected unconditionally: callers must source
/// secrets via `password_env`, `password_command`, or `keychain_service` +
/// `keychain_account`. This guarantees the plaintext secret never passes through
/// Plenum.
async fn tool_connect(args: &Value, binding: &McpBinding) -> Result<Value> {
    // Capability boundary: the MCP connect tool never handles plaintext secrets.
    if args.get("password").and_then(|v| v.as_str()).is_some() {
        return Err(anyhow!(
            "CAPABILITY_VIOLATION: inline plaintext 'password' is not permitted on the MCP \
             connect tool. Reference the secret instead via 'password_env', 'password_command', \
             or 'keychain_service' + 'keychain_account'."
        ));
    }

    // Save mode: persist by reference and return without opening a connection.
    if let Some(save_raw) = args.get("save") {
        return tool_connect_save(args, binding, save_raw);
    }

    let (config, _is_readonly) = resolve_connection_from_args(args, binding)?;

    let connection_info = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::validate_connection(&config)
            .await
            .map_err(|e| anyhow!("SQLite connection test failed: {e}"))?,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            return Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"));
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::validate_connection(&config)
            .await
            .map_err(|e| anyhow!("PostgreSQL connection test failed: {e}"))?,
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            return Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"));
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::validate_connection(&config)
            .await
            .map_err(|e| anyhow!("MySQL connection test failed: {e}"))?,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            return Err(anyhow!("MySQL engine not enabled. Build with --features mysql"));
        }

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::validate_connection(&config)
            .await
            .map_err(|e| anyhow!("DuckDB connection test failed: {e}"))?,
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => {
            return Err(anyhow!("DuckDB engine not enabled. Build with --features duckdb"));
        }
    };

    CallToolResult::success(connection_info)
}

/// Persist a connection config by reference for the `connect` tool's save mode.
///
/// The stored config carries no plaintext password (inline `password` was already
/// rejected by the caller); credentials are recorded only as references.
fn tool_connect_save(args: &Value, binding: &McpBinding, save_raw: &Value) -> Result<Value> {
    let location = match save_raw.as_str() {
        Some("local") => ConfigLocation::Local,
        Some("global") => ConfigLocation::Global,
        _ => {
            return Err(anyhow!("Invalid 'save' value. Must be \"local\" or \"global\""));
        }
    };

    // Saving requires explicit connection parameters — you cannot re-save a
    // connection selected purely by name, and a DSN would carry inline secrets.
    let engine_str = args.get("engine").and_then(|v| v.as_str()).ok_or_else(|| {
        anyhow!("'save' requires explicit connection params, starting with 'engine'")
    })?;

    let (password_env, password_command, keychain_entry) = parse_credential_refs(args)?;
    let has_ref = password_env.is_some() || password_command.is_some() || keychain_entry.is_some();

    // Build the config to store. `allow_missing_password` keeps the config free of
    // any inline secret — the reference is the sole password authority.
    let config = build_connection_config_from_args(args, engine_str, true)?;

    // Project path: explicit arg wins, else the server-registration binding, else cwd.
    let project_path = match args.get("project_path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => match &binding.project_path {
            Some(p) => p.clone(),
            None => {
                crate::config::get_current_project_path().map_err(|e| anyhow!("{}", e.message()))?
            }
        },
    };

    let name = args.get("name").and_then(|v| v.as_str()).map(String::from);

    crate::config::save_connection_in_project(
        &project_path,
        name.clone(),
        config,
        password_env.clone(),
        password_command.clone(),
        keychain_entry.clone(),
        location,
    )
    .map_err(|e| anyhow!("Failed to save connection: {}", e.message()))?;

    let credential_source = if password_env.is_some() {
        "password_env"
    } else if password_command.is_some() {
        "password_command"
    } else if keychain_entry.is_some() {
        "keychain"
    } else {
        "none"
    };

    CallToolResult::success(serde_json::json!({
        "saved": true,
        "location": match location {
            ConfigLocation::Local => "local",
            ConfigLocation::Global => "global",
        },
        "name": name.unwrap_or_else(|| "default".to_string()),
        "project_path": project_path,
        "credential_source": credential_source,
        "has_reference": has_ref,
    }))
}

/// MCP Tool: introspect
///
/// Introspects database schema and returns table/column information.
/// When `diff_against` is provided, computes a structural schema diff instead.
async fn tool_introspect(args: &Value, binding: &McpBinding) -> Result<Value> {
    // Resolve base connection config
    let (config, _is_readonly) = resolve_connection_from_args(args, binding)?;

    // Get optional database and schema modifiers (shared by both paths)
    let database = args.get("target_database").and_then(|v| v.as_str());
    let schema = args.get("schema").and_then(|v| v.as_str());

    // ── diff-against path ──────────────────────────────────────────────────────
    if let Some(target_name) = args.get("diff_against").and_then(|v| v.as_str()) {
        let target_proj = args.get("diff_against_project_path").and_then(|v| v.as_str());
        let (target_config, _) = crate::resolve_connection(target_proj, Some(target_name))
            .map_err(|e| {
                anyhow!("Failed to resolve diff-against connection '{target_name}': {e}")
            })?;

        let diff = crate::diff::compute_schema_diff(&config, &target_config, database, schema)
            .await
            .map_err(|e| anyhow!("Schema diff failed: {e}"))?;

        return CallToolResult::success(serde_json::json!({ "diff": diff }));
    }

    // ── standard introspect path ───────────────────────────────────────────────
    let operation = parse_introspect_operation(args)?;

    // Call engine's introspect method (opens and closes connection)
    let result = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::introspect(&config, &operation, database, schema)
            .await
            .map_err(|e| anyhow!("SQLite introspection failed: {e}"))?,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            return Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"));
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::introspect(&config, &operation, database, schema)
            .await
            .map_err(|e| anyhow!("PostgreSQL introspection failed: {e}"))?,
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            return Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"));
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::introspect(&config, &operation, database, schema)
            .await
            .map_err(|e| anyhow!("MySQL introspection failed: {e}"))?,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            return Err(anyhow!("MySQL engine not enabled. Build with --features mysql"));
        }

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::introspect(&config, &operation, database, schema)
            .await
            .map_err(|e| anyhow!("DuckDB introspection failed: {e}"))?,
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => {
            return Err(anyhow!("DuckDB engine not enabled. Build with --features duckdb"));
        }
    };

    CallToolResult::success(result)
}

/// Parse introspect operation from MCP arguments.
/// Called only on the standard path; `diff_against` is handled before this in `tool_introspect`.
fn parse_introspect_operation(args: &Value) -> Result<crate::engine::IntrospectOperation> {
    use crate::engine::{IntrospectOperation, TableFields};

    // Check which operation is requested (mutually exclusive)
    let is_list_databases = args.get("list_databases").and_then(Value::as_bool).unwrap_or(false);
    let is_list_schemas = args.get("list_schemas").and_then(Value::as_bool).unwrap_or(false);
    let is_list_tables = args.get("list_tables").and_then(Value::as_bool).unwrap_or(false);
    let is_list_views = args.get("list_views").and_then(Value::as_bool).unwrap_or(false);
    let is_list_indexes = args.get("list_indexes").is_some();
    let table_name = args.get("table").and_then(|v| v.as_str());
    let view_name = args.get("view").and_then(|v| v.as_str());

    // Count how many operations were specified
    let op_count = [
        is_list_databases,
        is_list_schemas,
        is_list_tables,
        is_list_views,
        is_list_indexes,
        table_name.is_some(),
        view_name.is_some(),
    ]
    .iter()
    .filter(|&&x| x)
    .count();

    if op_count == 0 {
        return Err(anyhow!(
            "No introspect operation specified. Must provide one of: \
             list_databases, list_schemas, list_tables, list_views, list_indexes, table, view, \
             or diff_against"
        ));
    }

    if op_count > 1 {
        return Err(anyhow!(
            "Multiple introspect operations specified. Only one operation allowed per call."
        ));
    }

    // Build the operation
    if is_list_databases {
        return Ok(IntrospectOperation::ListDatabases);
    }

    if is_list_schemas {
        return Ok(IntrospectOperation::ListSchemas);
    }

    if is_list_tables {
        return Ok(IntrospectOperation::ListTables);
    }

    if is_list_views {
        return Ok(IntrospectOperation::ListViews);
    }

    if is_list_indexes {
        let table_filter = args.get("list_indexes").and_then(|v| v.as_str()).map(String::from);
        return Ok(IntrospectOperation::ListIndexes { table: table_filter });
    }

    if let Some(name) = table_name {
        // Parse table field selectors
        let fields = TableFields {
            columns: args.get("columns").and_then(Value::as_bool).unwrap_or(true),
            primary_key: args.get("primary_key").and_then(Value::as_bool).unwrap_or(true),
            foreign_keys: args.get("foreign_keys").and_then(Value::as_bool).unwrap_or(true),
            indexes: args.get("indexes").and_then(Value::as_bool).unwrap_or(true),
        };

        return Ok(IntrospectOperation::TableDetails { name: name.to_string(), fields });
    }

    if let Some(name) = view_name {
        return Ok(IntrospectOperation::ViewDetails { name: name.to_string() });
    }

    Err(anyhow!("Failed to parse introspect operation"))
}

/// MCP Tool: query
///
/// Executes a READ-ONLY SQL query.
async fn tool_query(args: &Value, binding: &McpBinding) -> Result<Value> {
    // Extract SQL
    let sql = args["sql"].as_str().ok_or_else(|| anyhow!("Missing required field: sql"))?;

    // Resolve connection config
    let (mut config, _is_readonly) = resolve_connection_from_args(args, binding)?;

    // Apply target_database override if provided
    if let Some(target_db) = args.get("target_database").and_then(|v| v.as_str()) {
        config.database = Some(target_db.to_string());
    }

    // Extract safety parameters from args
    let max_rows = args.get("max_rows").and_then(serde_json::Value::as_u64).map(|n| n as usize);
    let max_bytes = args.get("max_bytes").and_then(serde_json::Value::as_u64).map(|n| n as usize);
    let timeout_ms = args.get("timeout_ms").and_then(serde_json::Value::as_u64);
    let time_only = args.get("time_only").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let check_only = args.get("check_only").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let explain_format = match args.get("explain_format").and_then(serde_json::Value::as_str) {
        None | Some("native") => None,
        Some("structured") => Some(crate::engine::ExplainFormat::Structured),
        Some(other) => {
            return Err(anyhow!(
                "Invalid explain_format '{other}'. Valid values: native, structured"
            ));
        }
    };

    // Build capabilities (read-only only; max_bytes is post-processed below)
    let capabilities =
        Capabilities { max_rows, max_bytes: None, timeout_ms, offset: None, explain_format };

    // Validate query is read-only (pre-execution check)
    crate::validate_query(sql, &capabilities, config.engine).map_err(|e| anyhow!("{e}"))?;

    // check_only: return verdict without opening a database connection
    if check_only {
        return CallToolResult::success(
            serde_json::json!({ "would_execute": true, "category": "read" }),
        );
    }

    // Execute query (opens and closes connection)
    let mut query_result = execute_query(&config, sql, &capabilities).await?;

    // Apply byte budget post-engine (row-boundary truncation)
    if let Some(max_b) = max_bytes {
        crate::engine::apply_byte_budget(&mut query_result, max_b);
    }

    // Return time-only result if requested (for benchmarking)
    if time_only {
        let time_only_result = crate::TimeOnlyResult {
            execution_ms: query_result.execution_ms,
            rows_matched: query_result.rows.len(),
        };
        CallToolResult::success(time_only_result)
    } else {
        CallToolResult::success(query_result)
    }
}

// ============================================================================
// Helper Functions (Stateless)
// ============================================================================

/// Extract credential-reference arguments from a tool call.
///
/// Recognizes `password_env`, `password_command`, and the keychain pair
/// `keychain_service` + `keychain_account`. These name where a secret lives;
/// the plaintext secret itself never passes through Plenum.
///
/// Enforces the same invariants the config layer does:
/// - at most one credential source may be specified;
/// - `keychain_service` and `keychain_account` must be supplied together.
///
/// Returns `(password_env, password_command, keychain_entry)` — all `None` when
/// no reference was provided.
fn parse_credential_refs(
    args: &Value,
) -> Result<(Option<String>, Option<String>, Option<KeychainEntry>)> {
    let password_env = args.get("password_env").and_then(|v| v.as_str()).map(String::from);
    let password_command = args.get("password_command").and_then(|v| v.as_str()).map(String::from);
    let service = args.get("keychain_service").and_then(|v| v.as_str()).map(String::from);
    let account = args.get("keychain_account").and_then(|v| v.as_str()).map(String::from);

    let keychain_entry = match (service, account) {
        (Some(service), Some(account)) => Some(KeychainEntry { service, account }),
        (None, None) => None,
        _ => {
            return Err(anyhow!(
                "'keychain_service' and 'keychain_account' must be provided together"
            ));
        }
    };

    let source_count =
        [password_env.is_some(), password_command.is_some(), keychain_entry.is_some()]
            .iter()
            .filter(|&&b| b)
            .count();

    if source_count > 1 {
        return Err(anyhow!(
            "Only one credential reference is allowed: \
             password_env, password_command, or keychain_service/keychain_account"
        ));
    }

    Ok((password_env, password_command, keychain_entry))
}

/// Build `ConnectionConfig` from JSON arguments.
///
/// When `allow_missing_password` is true the inline `password` field is
/// optional — the caller is expected to supply a credential reference
/// (`password_env` / `password_command` / `keychain_service`+`keychain_account`)
/// that resolves the secret at connection time. The resulting config carries
/// `password: None` so the reference is the sole authority.
fn build_connection_config_from_args(
    args: &Value,
    engine_str: &str,
    allow_missing_password: bool,
) -> Result<ConnectionConfig> {
    let engine_type = match engine_str {
        "postgres" => DatabaseType::Postgres,
        "mysql" => DatabaseType::MySQL,
        "sqlite" => DatabaseType::SQLite,
        "duckdb" => DatabaseType::DuckDB,
        _ => return Err(anyhow!("Invalid engine. Must be postgres, mysql, sqlite, or duckdb")),
    };

    match engine_type {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host = args["host"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: host"))?
                .to_string();
            let port = args["port"]
                .as_u64()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: port"))?
                as u16;
            let user = args["user"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: user"))?
                .to_string();
            let password = if allow_missing_password {
                // Reference-sourced: no inline password is expected here.
                args["password"].as_str().unwrap_or_default().to_string()
            } else {
                args["password"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Missing required field for {engine_str}: password"))?
                    .to_string()
            };
            let database = args["database"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: database"))?
                .to_string();

            let mut config = if engine_type == DatabaseType::Postgres {
                ConnectionConfig::postgres(host, port, user, password, database)
            } else {
                ConnectionConfig::mysql(host, port, user, password, database)
            };
            // With a credential reference, the reference is the sole authority.
            if allow_missing_password && args["password"].as_str().is_none() {
                config.password = None;
            }
            Ok(config)
        }
        DatabaseType::SQLite => {
            let file_str = args["file"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for sqlite: file"))?;
            Ok(ConnectionConfig::sqlite(PathBuf::from(file_str)))
        }
        DatabaseType::DuckDB => {
            let file_str = args["file"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for duckdb: file"))?;
            Ok(ConnectionConfig::duckdb(PathBuf::from(file_str)))
        }
    }
}

/// Resolve connection config from JSON arguments, falling back to the
/// server-registration `binding` when the call omits connection selectors.
///
/// Resolution order:
/// 1. DSN string: one-off URL, bypasses saved config; mutually exclusive with connection/engine
/// 2. Named connection: loads saved connection (within the bound project path), with overrides
/// 3. Explicit parameters: requires engine and all connection details
/// 4. Binding fallback: `--dsn-env`, or `--project-path` / `--name`, or the
///    cwd default — with a structured error when nothing resolves.
///
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
fn resolve_connection_from_args(
    args: &Value,
    binding: &McpBinding,
) -> Result<(ConnectionConfig, bool)> {
    // Scenario 0: DSN one-off URL (mutually exclusive with connection and engine)
    if let Some(dsn_str) = args.get("dsn").and_then(|v| v.as_str()) {
        if args.get("connection").and_then(|v| v.as_str()).is_some() {
            return Err(anyhow!("'dsn' and 'connection' are mutually exclusive"));
        }
        if args.get("engine").and_then(|v| v.as_str()).is_some() {
            return Err(anyhow!("'dsn' and 'engine' are mutually exclusive"));
        }
        let config = parse_dsn(dsn_str)
            .map_err(|e| anyhow!("{} (DSN: {})", e.message(), redact_dsn(dsn_str)))?;
        return Ok((config, false));
    }

    let has_connection = args.get("connection").and_then(|v| v.as_str()).is_some();
    let has_engine = args.get("engine").and_then(|v| v.as_str()).is_some();

    // Scenario 1: Named connection (with optional overrides)
    if has_connection {
        let connection = args["connection"].as_str().unwrap();

        // When --project-path is bound, look up the named connection inside that
        // project's local config (not CWD).  Without a bound path, fall back to CWD.
        let (mut config, is_readonly) = if let Some(path) = &binding.project_path {
            crate::config::resolve_connection_in_project(path, Some(connection))
        } else {
            crate::resolve_connection(None, Some(connection))
        }
        .map_err(|e| anyhow!("Failed to resolve connection '{connection}': {e}"))?;

        // Apply overrides
        if let Some(eng) = args.get("engine").and_then(|v| v.as_str()) {
            config.engine = match eng {
                "postgres" => DatabaseType::Postgres,
                "mysql" => DatabaseType::MySQL,
                "sqlite" => DatabaseType::SQLite,
                "duckdb" => DatabaseType::DuckDB,
                _ => return Err(anyhow!("Invalid engine: {eng}")),
            };
        }
        if let Some(h) = args.get("host").and_then(|v| v.as_str()) {
            config.host = Some(h.to_string());
        }
        if let Some(p) = args.get("port").and_then(serde_json::Value::as_u64) {
            config.port = Some(p as u16);
        }
        if let Some(u) = args.get("user").and_then(|v| v.as_str()) {
            config.user = Some(u.to_string());
        }
        if let Some(pw) = args.get("password").and_then(|v| v.as_str()) {
            config.password = Some(pw.to_string());
        }
        if let Some(db) = args.get("database").and_then(|v| v.as_str()) {
            config.database = Some(db.to_string());
        }
        if let Some(f) = args.get("file").and_then(|v| v.as_str()) {
            config.file = Some(PathBuf::from(f));
        }

        return Ok((config, is_readonly));
    }

    // Scenario 2: Explicit connection parameters
    if has_engine {
        let engine_str = args["engine"].as_str().unwrap();
        let (password_env, password_command, keychain_entry) = parse_credential_refs(args)?;
        let has_ref =
            password_env.is_some() || password_command.is_some() || keychain_entry.is_some();

        let config = build_connection_config_from_args(args, engine_str, has_ref)?;

        // A credential reference sources the secret at connection time; resolve it
        // now so the one-off connection carries the real password. The plaintext
        // never came through Plenum — only the reference string did.
        if has_ref {
            let stored = StoredConnection {
                config,
                password_env,
                password_command,
                keychain_entry,
                readonly: None,
            };
            let (resolved, is_readonly) =
                stored.resolve().map_err(|e| anyhow!("{}", e.message()))?;
            return Ok((resolved, is_readonly));
        }

        return Ok((config, false)); // Explicit connections are never readonly
    }

    // Scenario 3: Fall back to the server-registration binding.
    resolve_from_binding(binding)
}

/// Resolve a connection from the server-registration binding alone (no per-call
/// connection selectors were provided).
///
/// Priority: `--dsn-env` (explicit env-sourced DSN) → `--project-path` / `--name`
/// (or the cwd default when neither is bound). When nothing resolves and no
/// binding flags were given, returns a structured error naming exactly which
/// flags to add — never a silent fallback.
fn resolve_from_binding(binding: &McpBinding) -> Result<(ConnectionConfig, bool)> {
    // `--dsn-env`: read the DSN from ONLY the named variable. Plenum never reads
    // an ambient credential source (DATABASE_URL, PGPASSWORD, ...) that the
    // caller did not explicitly name.
    if let Some(var) = &binding.dsn_env {
        let dsn = std::env::var(var).map_err(|_| {
            anyhow!(
                "--dsn-env variable '{var}' is not set in the environment. \
                 Plenum reads only the named variable and never falls back to \
                 other environment variables."
            )
        })?;
        let config =
            parse_dsn(&dsn).map_err(|e| anyhow!("{} (from --dsn-env {})", e.message(), var))?;
        return Ok((config, false));
    }

    // `--project-path`: load the local config from the project directory itself,
    // not from the launcher's cwd — that's the whole point of the flag.
    if let Some(path) = &binding.project_path {
        return crate::config::resolve_connection_in_project(path, binding.name.as_deref())
            .map_err(|e| anyhow!("Failed to resolve bound connection: {e}"));
    }

    // `--name` only (no --project-path): use cwd as project, pick the named connection.
    // No flags at all: use cwd + project default.
    crate::resolve_connection(None, binding.name.as_deref()).map_err(|e| {
        if binding.name.is_none() {
            // Nothing was bound and cwd has no usable config — name the flags exactly.
            anyhow!(
                "No connection could be resolved: no connection selectors were passed to this \
                 tool call and 'plenum mcp' was started without a connection binding. \
                 Restart the server with one of: '--project-path <path>' to pin the project, \
                 '--name <connection>' to select a saved connection, or \
                 '--dsn-env <ENV_VAR>' to read a DSN from a named environment variable. \
                 Underlying error: {e}"
            )
        } else {
            anyhow!("Failed to resolve bound connection: {e}")
        }
    })
}

/// Validate database connection
///
/// Opens a connection, validates it, and immediately closes it.
/// This function is stateless - no connection persists after it returns.
async fn validate_connection(config: &ConnectionConfig) -> Result<crate::ConnectionInfo> {
    match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("SQLite connection failed: {e}")),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("PostgreSQL connection failed: {e}")),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("MySQL connection failed: {e}")),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(anyhow!("MySQL engine not enabled. Build with --features mysql"))
        }

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("DuckDB connection failed: {e}")),
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => {
            Err(anyhow!("DuckDB engine not enabled. Build with --features duckdb"))
        }
    }
}

/// Execute query
///
/// Opens a connection, executes query, and immediately closes it.
/// This function is stateless - no connection persists after it returns.
async fn execute_query(
    config: &ConnectionConfig,
    sql: &str,
    capabilities: &Capabilities,
) -> Result<crate::QueryResult> {
    match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::execute(config, sql, &[], capabilities)
            .await
            .map_err(|e| anyhow!("SQLite query failed: {e}")),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::execute(config, sql, &[], capabilities)
            .await
            .map_err(|e| anyhow!("PostgreSQL query failed: {e}")),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::execute(config, sql, &[], capabilities)
            .await
            .map_err(|e| anyhow!("MySQL query failed: {e}")),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(anyhow!("MySQL engine not enabled. Build with --features mysql"))
        }

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::execute(config, sql, &[], capabilities)
            .await
            .map_err(|e| anyhow!("DuckDB query failed: {e}")),
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => {
            Err(anyhow!("DuckDB engine not enabled. Build with --features duckdb"))
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── McpBinding construction / conflict rules ────────────────────────────

    #[test]
    fn binding_rejects_dsn_env_with_project_path() {
        let err = McpBinding::new(Some("/p".to_string()), None, Some("PLENUM_DSN".to_string()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--dsn-env"), "error should name --dsn-env: {err}");
        assert!(err.contains("--project-path"), "error should name --project-path: {err}");
    }

    #[test]
    fn binding_rejects_dsn_env_with_name() {
        let err = McpBinding::new(None, Some("prod".to_string()), Some("PLENUM_DSN".to_string()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--dsn-env"), "error should name --dsn-env: {err}");
        assert!(err.contains("--name"), "error should name --name: {err}");
    }

    #[test]
    fn binding_allows_project_path_and_name_together() {
        let binding =
            McpBinding::new(Some("/p".to_string()), Some("prod".to_string()), None).unwrap();
        assert_eq!(binding.project_path.as_deref(), Some("/p"));
        assert_eq!(binding.name.as_deref(), Some("prod"));
    }

    #[test]
    fn binding_allows_dsn_env_alone_and_empty() {
        assert!(McpBinding::new(None, None, Some("PLENUM_DSN".to_string())).is_ok());
        assert!(McpBinding::default().dsn_env.is_none());
    }

    // ── --dsn-env resolution (credential-sourcing safety) ───────────────────

    #[test]
    fn dsn_env_reads_only_the_named_variable() {
        // A named var holding a valid DSN resolves; nothing else is consulted.
        std::env::set_var("PLENUM_TEST_DSN_ENV_OK", "sqlite::memory:");
        let binding =
            McpBinding::new(None, None, Some("PLENUM_TEST_DSN_ENV_OK".to_string())).unwrap();
        let (config, is_readonly) = resolve_from_binding(&binding).unwrap();
        assert_eq!(config.engine, DatabaseType::SQLite);
        assert!(!is_readonly);
        std::env::remove_var("PLENUM_TEST_DSN_ENV_OK");
    }

    #[test]
    fn dsn_env_never_falls_back_to_ambient_credentials() {
        // Set a plausible ambient credential source, then point --dsn-env at a
        // DIFFERENT, unset variable. Plenum must NOT pick up the ambient one.
        std::env::set_var("PLENUM_TEST_AMBIENT_DSN", "postgres://u:p@host:5432/db");
        std::env::remove_var("PLENUM_TEST_DSN_ENV_UNSET");

        let binding =
            McpBinding::new(None, None, Some("PLENUM_TEST_DSN_ENV_UNSET".to_string())).unwrap();
        let err = resolve_from_binding(&binding).unwrap_err().to_string();

        assert!(
            err.contains("PLENUM_TEST_DSN_ENV_UNSET"),
            "error should name the missing var: {err}"
        );
        assert!(
            !err.contains("PLENUM_TEST_AMBIENT_DSN"),
            "must not reference the ambient var: {err}"
        );
        assert!(!err.contains("host"), "must not leak the ambient DSN value: {err}");
        std::env::remove_var("PLENUM_TEST_AMBIENT_DSN");
    }

    #[test]
    fn dsn_env_with_invalid_dsn_is_rejected() {
        std::env::set_var("PLENUM_TEST_DSN_ENV_BAD", "not-a-valid-dsn");
        let binding =
            McpBinding::new(None, None, Some("PLENUM_TEST_DSN_ENV_BAD".to_string())).unwrap();
        let err = resolve_from_binding(&binding).unwrap_err().to_string();
        assert!(err.contains("PLENUM_TEST_DSN_ENV_BAD"), "error names the var: {err}");
        std::env::remove_var("PLENUM_TEST_DSN_ENV_BAD");
    }

    // ── Per-call args still take precedence over the binding ────────────────

    #[test]
    fn per_call_dsn_overrides_binding_and_is_used_directly() {
        // Even with a --dsn-env binding, an explicit per-call `dsn` wins.
        std::env::set_var("PLENUM_TEST_DSN_ENV_BINDING", "sqlite::memory:");
        let binding =
            McpBinding::new(None, None, Some("PLENUM_TEST_DSN_ENV_BINDING".to_string())).unwrap();
        let args = serde_json::json!({ "dsn": "sqlite:/tmp/explicit.db" });
        let (config, _) = resolve_connection_from_args(&args, &binding).unwrap();
        assert_eq!(config.engine, DatabaseType::SQLite);
        assert_eq!(config.file.as_deref(), Some(std::path::Path::new("/tmp/explicit.db")));
        std::env::remove_var("PLENUM_TEST_DSN_ENV_BINDING");
    }

    #[test]
    fn per_call_dsn_and_connection_are_mutually_exclusive() {
        let args = serde_json::json!({ "dsn": "sqlite::memory:", "connection": "prod" });
        let err =
            resolve_connection_from_args(&args, &McpBinding::default()).unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    // ── Credential-reference parsing (REF-298) ──────────────────────────────

    #[test]
    fn parse_credential_refs_none_when_absent() {
        let (env, cmd, kc) = parse_credential_refs(&serde_json::json!({})).unwrap();
        assert!(env.is_none() && cmd.is_none() && kc.is_none());
    }

    #[test]
    fn parse_credential_refs_env() {
        let (env, cmd, kc) =
            parse_credential_refs(&serde_json::json!({ "password_env": "DB_PASSWORD" })).unwrap();
        assert_eq!(env.as_deref(), Some("DB_PASSWORD"));
        assert!(cmd.is_none() && kc.is_none());
    }

    #[test]
    fn parse_credential_refs_keychain_requires_both() {
        let err = parse_credential_refs(&serde_json::json!({ "keychain_service": "svc" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("keychain_service") && err.contains("keychain_account"), "got: {err}");
    }

    #[test]
    fn parse_credential_refs_rejects_multiple_sources() {
        let args = serde_json::json!({ "password_env": "A", "password_command": "echo b" });
        let err = parse_credential_refs(&args).unwrap_err().to_string();
        assert!(err.contains("Only one credential reference"), "got: {err}");
    }

    // ── One-off explicit connection sourced from a credential reference ──────

    #[test]
    fn one_off_explicit_resolves_password_from_env_reference() {
        // The reference string is passed; Plenum reads the secret from the env
        // var at resolution time. No plaintext ever appeared in the args.
        std::env::set_var("PLENUM_TEST_ONEOFF_PWD", "s3cret-from-env");
        let args = serde_json::json!({
            "engine": "postgres",
            "host": "db.example.com",
            "port": 5432,
            "user": "agent",
            "database": "app",
            "password_env": "PLENUM_TEST_ONEOFF_PWD"
        });
        let (config, _ro) = resolve_connection_from_args(&args, &McpBinding::default()).unwrap();
        assert_eq!(config.engine, DatabaseType::Postgres);
        assert_eq!(config.password.as_deref(), Some("s3cret-from-env"));
        std::env::remove_var("PLENUM_TEST_ONEOFF_PWD");
    }

    #[test]
    fn one_off_explicit_missing_env_reference_is_error() {
        std::env::remove_var("PLENUM_TEST_ONEOFF_MISSING");
        let args = serde_json::json!({
            "engine": "postgres",
            "host": "h", "port": 5432, "user": "u", "database": "d",
            "password_env": "PLENUM_TEST_ONEOFF_MISSING"
        });
        let err =
            resolve_connection_from_args(&args, &McpBinding::default()).unwrap_err().to_string();
        assert!(err.contains("PLENUM_TEST_ONEOFF_MISSING"), "names missing var: {err}");
    }

    // ── connect save mode persists by reference (no plaintext) ──────────────

    #[test]
    fn connect_save_local_writes_reference_no_plaintext() {
        let project =
            std::env::temp_dir().join(format!("plenum-mcp-save-{}-{:p}", std::process::id(), &0u8));
        let _ = std::fs::remove_dir_all(&project);
        std::fs::create_dir_all(&project).unwrap();

        let args = serde_json::json!({
            "engine": "postgres",
            "host": "db.example.com",
            "port": 5432,
            "user": "agent",
            "database": "app",
            "password_env": "DB_PASSWORD",
            "save": "local",
            "name": "prod",
            "project_path": project.to_str().unwrap(),
        });

        let result = tool_connect_save(&args, &McpBinding::default(), &args["save"]).unwrap();
        // Tool result envelope wraps a JSON text block; assert the confirmation shape.
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"saved\": true"), "confirmation: {text}");
        assert!(text.contains("\"location\": \"local\""), "location: {text}");

        let written = std::fs::read_to_string(project.join(".plenum").join("config.json")).unwrap();
        assert!(written.contains("password_env"), "reference stored: {written}");
        assert!(written.contains("DB_PASSWORD"));
        assert!(!written.contains("\"password\":"), "no plaintext: {written}");
        assert!(written.contains("prod"));

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn connect_save_rejects_missing_engine() {
        let args = serde_json::json!({ "save": "local", "connection": "prod" });
        let err = tool_connect_save(&args, &McpBinding::default(), &args["save"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("engine"), "got: {err}");
    }

    #[test]
    fn connect_save_rejects_invalid_location() {
        let args = serde_json::json!({ "save": "cloud", "engine": "sqlite", "file": "/tmp/x.db" });
        let err = tool_connect_save(&args, &McpBinding::default(), &args["save"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("local") && err.contains("global"), "got: {err}");
    }
}
