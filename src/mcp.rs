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
//! - `connect` - Validate and save database connections
//! - `introspect` - Introspect database schema
//! - `query` - Execute constrained SQL queries
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

use crate::{Capabilities, ConfigLocation, ConnectionConfig, DatabaseEngine, DatabaseType};

// Import database engines
#[cfg(feature = "mysql")]
use crate::engine::mysql::MySqlEngine;
#[cfg(feature = "postgres")]
use crate::engine::postgres::PostgresEngine;
#[cfg(feature = "sqlite")]
use crate::engine::sqlite::SqliteEngine;

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
pub async fn serve() -> Result<()> {
    let stdin = io::stdin();
    let reader = stdin.lock();
    let mut stdout = io::stdout();

    for line in reader.lines() {
        let line = line?;

        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON-RPC request
        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                // Send parse error response
                let error_response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700, // Parse error
                        message: format!("Parse error: {e}"),
                        data: None,
                    }),
                };
                let response_json = serde_json::to_string(&error_response)?;
                writeln!(stdout, "{response_json}")?;
                stdout.flush()?;
                continue;
            }
        };

        // Handle request
        let response = handle_request(request).await;

        // Write response
        let response_json = serde_json::to_string(&response)?;
        writeln!(stdout, "{response_json}")?;
        stdout.flush()?;
    }

    Ok(())
}

/// Handle a JSON-RPC request
///
/// Routes the request to the appropriate handler based on the method name.
async fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    let result = match request.method.as_str() {
        "initialize" => handle_initialize(request.params),
        "tools/list" => handle_list_tools(),
        "tools/call" => handle_call_tool(request.params).await,
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
                "name": "connect",
                "description": "ONE-TIME SETUP: Validate and save database connection configuration. IMPORTANT: (1) NEVER guess or invent credentials (e.g., 'root'/'root'). If the user hasn't provided credentials, ASK the user for them. (2) This tool is for INITIAL SETUP only - do NOT call this repeatedly. (3) After saving a connection once, use its connection name in introspect/query tools. Use cases: initial project setup, adding new connections, validating existing credentials. The connection is opened, validated, and immediately closed - no persistent connection is maintained. Supports PostgreSQL, MySQL, and SQLite. Save locations: 'local' (.plenum/config.json, team-shareable), 'global' (~/.config/plenum/connections.json, user-private). WORKFLOW: (1) Save connection once with this tool, (2) Reference by connection name in all subsequent introspect/query calls, (3) Never pass explicit credentials repeatedly.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine type"
                        },
                        "connection": {
                            "type": "string",
                            "description": "Connection name (optional)"
                        },
                        "host": {
                            "type": "string",
                            "description": "Host (for postgres/mysql)"
                        },
                        "port": {
                            "type": "number",
                            "description": "Port (for postgres/mysql)"
                        },
                        "user": {
                            "type": "string",
                            "description": "Username (for postgres/mysql). NEVER guess or invent - if not provided by user, ASK for it."
                        },
                        "password": {
                            "type": "string",
                            "description": "Password (for postgres/mysql). NEVER guess or invent - if not provided by user, ASK for it."
                        },
                        "database": {
                            "type": "string",
                            "description": "Database name (for postgres/mysql). Use \"*\" for wildcard mode to enable database discovery via SHOW DATABASES (MySQL) or pg_catalog queries (PostgreSQL)."
                        },
                        "file": {
                            "type": "string",
                            "description": "File path (for sqlite)"
                        },
                        "save": {
                            "type": "string",
                            "enum": ["local", "global"],
                            "description": "Optional: Where to save the connection. 'local' saves to .plenum/config.json (team-shareable, project-specific), 'global' saves to ~/.config/plenum/connections.json (user-private, cross-project). If omitted, connection is validated but not saved."
                        }
                    },
                    "required": ["engine"]
                }
            },
            {
                "name": "introspect",
                "description": "Introspect database schema with granular operations. NEVER dumps entire schema - requires explicit operation. IMPORTANT CONNECTION WORKFLOW: (1) RECOMMENDED: Auto-resolve (omit all connection params) - uses project's default saved connection, (2) COMMON: Named connection (use 'connection' param only) - references saved connection by name, (3) DISCOURAGED: Explicit credentials (engine + host/user/password) - ONLY for one-off scenarios, NOT for regular use. DO NOT pass credentials repeatedly - use saved connections instead. Before using explicit credentials, check if a saved connection exists. Operations (EXACTLY ONE required, mutually exclusive): list_databases (list all DBs), list_schemas (Postgres only), list_tables (table names in schema/DB), list_views (view names), list_indexes (all or filtered by table), table (full details for specific table with optional field filtering), view (view definition + columns). Optional modifiers: 'target_database' (switch to different DB before introspecting - Postgres/MySQL only), 'schema' (filter to specific schema - Postgres/MySQL only). Returns typed JSON specific to operation (DatabaseList, SchemaList, TableList, ViewList, IndexList, TableDetails, or ViewDetails). Stateless - connection opened, operation executed, connection closed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "connection": {
                            "type": "string",
                            "description": "RECOMMENDED: Name of saved connection to use. Loads from .plenum/config.json (local) or ~/.config/plenum/connections.json (global). If omitted along with 'engine', auto-resolves project's default connection (BEST PRACTICE)."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
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
                            "description": "DISCOURAGED: SQLite database file path. Only for one-off sqlite explicit connections or as override. Prefer using saved connections."
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
                        "connection": {
                            "type": "string",
                            "description": "RECOMMENDED: Name of saved connection to use. Connection must exist in local (.plenum/config.json) or global (~/.config/plenum/connections.json) config. If omitted along with 'engine', auto-resolves to project's default connection (BEST PRACTICE)."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "DISCOURAGED: Database engine type for explicit one-off connections. Only use if no saved connection exists. Valid values: 'postgres', 'mysql', 'sqlite'. If omitted along with 'connection', auto-resolves project's default connection (RECOMMENDED)."
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
                            "description": "DISCOURAGED: File path to SQLite database file. Only for one-off sqlite explicit connections. Can be relative or absolute path. Example: './app.db', '/var/lib/data.db'. Prefer using saved connections."
                        },
                        "max_rows": {
                            "type": "number",
                            "description": "CRITICAL: Maximum number of rows to return from SELECT queries. Due to MCP's 25k token response limit, this parameter is effectively REQUIRED for all queries against tables of unknown size. Omitting this will cause tool failure on large tables. Start small and increase if needed: Use 10 for initial exploration/preview, 50-100 for small known tables, 100-500 for medium tables (only after confirming size with COUNT(*) query). Even with columnar format (30-50% token reduction), a 100-row result with 10+ columns can approach token limits. Always prefer smaller limits initially."
                        },
                        "timeout_ms": {
                            "type": "number",
                            "description": "Optional: Query execution timeout in milliseconds. Recommended for potentially expensive queries to prevent long-running operations. Example: 5000 (5 seconds). No timeout if omitted."
                        }
                    },
                    "required": ["sql"]
                }
            }
        ]
    }))
}

/// Handle tools/call request
///
/// Routes the tool call to the appropriate tool implementation.
async fn handle_call_tool(params: Option<Value>) -> Result<Value> {
    let params = params.ok_or_else(|| anyhow!("Missing params"))?;
    let name = params["name"].as_str().ok_or_else(|| anyhow!("Missing tool name"))?;
    let arguments = &params["arguments"];

    match name {
        "connect" => tool_connect(arguments).await,
        "introspect" => tool_introspect(arguments).await,
        "query" => tool_query(arguments).await,
        _ => Err(anyhow!("Unknown tool: {name}")),
    }
}

// ============================================================================
// Tool Implementations
// ============================================================================

/// MCP Tool: connect
///
/// Validates and optionally saves a database connection configuration.
async fn tool_connect(args: &Value) -> Result<Value> {
    // Extract engine
    let engine_str =
        args["engine"].as_str().ok_or_else(|| anyhow!("Missing required field: engine"))?;

    // Build ConnectionConfig
    let config = build_connection_config_from_args(args, engine_str)?;

    // Validate connection (opens and immediately closes)
    let conn_info = validate_connection(&config).await?;

    // Save if requested
    if let Some(save_str) = args.get("save").and_then(|v| v.as_str()) {
        let location = match save_str {
            "local" => ConfigLocation::Local,
            "global" => ConfigLocation::Global,
            _ => return Err(anyhow!("Invalid save location. Must be 'local' or 'global'")),
        };

        let conn_name = args["connection"]
            .as_str()
            .map_or_else(|| "default".to_string(), std::string::ToString::to_string);

        // Use None for project_path (will default to current directory)
        crate::save_connection(None, Some(conn_name.clone()), config.clone(), location)
            .map_err(|e| anyhow!("Failed to save connection: {e}"))?;

        let response = serde_json::json!({
            "ok": true,
            "connection_name": conn_name,
            "engine": config.engine.as_str(),
            "saved_to": save_str,
            "connection_info": conn_info,
            "message": format!("Connection '{}' saved successfully", conn_name)
        });

        CallToolResult::success(response)
    } else {
        let response = serde_json::json!({
            "ok": true,
            "engine": config.engine.as_str(),
            "connection_info": conn_info,
            "message": "Connection validated successfully"
        });

        CallToolResult::success(response)
    }
}

/// MCP Tool: introspect
///
/// Introspects database schema and returns table/column information.
async fn tool_introspect(args: &Value) -> Result<Value> {
    // Resolve connection config
    let (config, _is_readonly) = resolve_connection_from_args(args)?;

    // Parse introspect operation from args
    let operation = parse_introspect_operation(args)?;

    // Get optional database and schema modifiers
    let database = args.get("target_database").and_then(|v| v.as_str());
    let schema = args.get("schema").and_then(|v| v.as_str());

    // Call engine's introspect method (opens and closes connection)
    let result = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => {
            SqliteEngine::introspect(&config, &operation, database, schema)
                .await
                .map_err(|e| anyhow!("SQLite introspection failed: {e}"))?
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            return Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"));
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => {
            PostgresEngine::introspect(&config, &operation, database, schema)
                .await
                .map_err(|e| anyhow!("PostgreSQL introspection failed: {e}"))?
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            return Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"));
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => {
            MySqlEngine::introspect(&config, &operation, database, schema)
                .await
                .map_err(|e| anyhow!("MySQL introspection failed: {e}"))?
        }
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            return Err(anyhow!("MySQL engine not enabled. Build with --features mysql"));
        }
    };

    CallToolResult::success(result)
}

/// Parse introspect operation from MCP arguments
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
    let op_count = [is_list_databases, is_list_schemas, is_list_tables, is_list_views, is_list_indexes, table_name.is_some(), view_name.is_some()]
        .iter()
        .filter(|&&x| x)
        .count();

    if op_count == 0 {
        return Err(anyhow!(
            "No introspect operation specified. Must provide one of: \
             list_databases, list_schemas, list_tables, list_views, list_indexes, table, or view"
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

        return Ok(IntrospectOperation::TableDetails {
            name: name.to_string(),
            fields,
        });
    }

    if let Some(name) = view_name {
        return Ok(IntrospectOperation::ViewDetails { name: name.to_string() });
    }

    Err(anyhow!("Failed to parse introspect operation"))
}

/// MCP Tool: query
///
/// Executes a READ-ONLY SQL query.
async fn tool_query(args: &Value) -> Result<Value> {
    // Extract SQL
    let sql = args["sql"].as_str().ok_or_else(|| anyhow!("Missing required field: sql"))?;

    // Resolve connection config
    let (config, _is_readonly) = resolve_connection_from_args(args)?;

    // Extract safety parameters from args
    let max_rows = args.get("max_rows").and_then(serde_json::Value::as_u64).map(|n| n as usize);
    let timeout_ms = args.get("timeout_ms").and_then(serde_json::Value::as_u64);

    // Build capabilities (read-only only)
    let capabilities = Capabilities { max_rows, timeout_ms };

    // Validate query is read-only (pre-execution check)
    crate::validate_query(sql, &capabilities, config.engine).map_err(|e| anyhow!("{e}"))?;

    // Execute query (opens and closes connection)
    let query_result = execute_query(&config, sql, &capabilities).await?;

    CallToolResult::success(query_result)
}

// ============================================================================
// Helper Functions (Stateless)
// ============================================================================

/// Build `ConnectionConfig` from JSON arguments
fn build_connection_config_from_args(args: &Value, engine_str: &str) -> Result<ConnectionConfig> {
    let engine_type = match engine_str {
        "postgres" => DatabaseType::Postgres,
        "mysql" => DatabaseType::MySQL,
        "sqlite" => DatabaseType::SQLite,
        _ => return Err(anyhow!("Invalid engine. Must be postgres, mysql, or sqlite")),
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
            let password = args["password"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: password"))?
                .to_string();
            let database = args["database"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {engine_str}: database"))?
                .to_string();

            if engine_type == DatabaseType::Postgres {
                Ok(ConnectionConfig::postgres(host, port, user, password, database))
            } else {
                Ok(ConnectionConfig::mysql(host, port, user, password, database))
            }
        }
        DatabaseType::SQLite => {
            let file_str = args["file"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for sqlite: file"))?;
            Ok(ConnectionConfig::sqlite(PathBuf::from(file_str)))
        }
    }
}

/// Resolve connection config from JSON arguments
///
/// This handles three scenarios:
/// 1. Named connection: Loads saved connection, optionally with overrides
/// 2. Explicit parameters: Requires engine and all connection details
/// 3. Auto-resolve default: When neither connection nor engine provided, uses current project's default connection
///
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
fn resolve_connection_from_args(args: &Value) -> Result<(ConnectionConfig, bool)> {
    let has_connection = args.get("connection").and_then(|v| v.as_str()).is_some();
    let has_engine = args.get("engine").and_then(|v| v.as_str()).is_some();

    // Scenario 1: Named connection (with optional overrides)
    if has_connection {
        let connection = args["connection"].as_str().unwrap();

        // Use None for project_path (defaults to current directory)
        let (mut config, is_readonly) = crate::resolve_connection(None, Some(connection))
            .map_err(|e| anyhow!("Failed to resolve connection '{connection}': {e}"))?;

        // Apply overrides
        if let Some(eng) = args.get("engine").and_then(|v| v.as_str()) {
            config.engine = match eng {
                "postgres" => DatabaseType::Postgres,
                "mysql" => DatabaseType::MySQL,
                "sqlite" => DatabaseType::SQLite,
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
        let config = build_connection_config_from_args(args, engine_str)?;
        return Ok((config, false)); // Explicit connections are never readonly
    }

    // Scenario 3: Auto-resolve default connection for current project
    // Use None for both project_path (current directory) and connection_name (use default)
    let (config, is_readonly) = crate::resolve_connection(None, None).map_err(|e| {
        anyhow!(
            "No connection or engine specified, and failed to auto-resolve default connection: {e}. \
             Either provide 'connection' (named), 'engine' (explicit), or ensure a default connection exists for this project."
        )
    })?;

    Ok((config, is_readonly))
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
        DatabaseType::SQLite => SqliteEngine::execute(config, sql, capabilities)
            .await
            .map_err(|e| anyhow!("SQLite query failed: {e}")),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::execute(config, sql, capabilities)
            .await
            .map_err(|e| anyhow!("PostgreSQL query failed: {e}")),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::execute(config, sql, capabilities)
            .await
            .map_err(|e| anyhow!("MySQL query failed: {e}")),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(anyhow!("MySQL engine not enabled. Build with --features mysql"))
        }
    }
}
