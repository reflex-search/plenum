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
                "description": "Validate and save database connection configuration. Use this tool to: (1) Test that connection parameters are valid, (2) Save connection details for later use by name, (3) Validate existing saved connections. The connection is opened, validated, and immediately closed - no persistent connection is maintained. Supports PostgreSQL, MySQL, and SQLite. Save locations: 'local' (.plenum/config.json, team-shareable), 'global' (~/.config/plenum/connections.json, user-private). Common pattern: save connection once with a name, then reference it by name in introspect/query tools.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine type"
                        },
                        "name": {
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
                            "description": "Username (for postgres/mysql)"
                        },
                        "password": {
                            "type": "string",
                            "description": "Password (for postgres/mysql)"
                        },
                        "database": {
                            "type": "string",
                            "description": "Database name (for postgres/mysql)"
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
                "description": "Introspect database schema and return structured information about tables, columns, data types, primary keys, foreign keys, and indexes. Use this tool before executing queries to understand the database structure. Connection resolution: (1) Use 'name' to reference a saved connection, (2) Provide explicit connection parameters (engine, host, port, etc.), (3) Mix both - use 'name' and override specific fields. The connection is opened, schema is introspected, and connection is immediately closed (stateless). Optional 'schema' filter limits results to a specific schema (PostgreSQL/MySQL only; SQLite ignores this). Returns JSON with complete schema information suitable for query generation.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Name of saved connection to use. Connection must exist in local (.plenum/config.json) or global (~/.config/plenum/connections.json) config. Cannot be used alone with 'engine' - choose one or mix name with overrides."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine type. Required if not using 'name'. Valid values: 'postgres', 'mysql', 'sqlite'."
                        },
                        "host": {
                            "type": "string",
                            "description": "Database host (for postgres/mysql). Required for explicit connections or use as override. Example: 'localhost', 'db.example.com'."
                        },
                        "port": {
                            "type": "number",
                            "description": "Database port (for postgres/mysql). Required for explicit connections or use as override. Defaults: postgres=5432, mysql=3306."
                        },
                        "user": {
                            "type": "string",
                            "description": "Database username (for postgres/mysql). Required for explicit connections or use as override."
                        },
                        "password": {
                            "type": "string",
                            "description": "Database password (for postgres/mysql). Required for explicit connections or use as override. Passed directly - agent responsible for security."
                        },
                        "database": {
                            "type": "string",
                            "description": "Database name (for postgres/mysql). Required for explicit connections or use as override. The specific database to introspect."
                        },
                        "file": {
                            "type": "string",
                            "description": "File path to SQLite database file. Required for sqlite engine. Can be relative or absolute path. Example: './app.db', '/var/lib/data.db'."
                        },
                        "schema": {
                            "type": "string",
                            "description": "Optional: Filter introspection to specific schema. PostgreSQL/MySQL only (SQLite ignores this). Example: 'public', 'analytics'. If omitted, all schemas are introspected."
                        }
                    }
                }
            },
            {
                "name": "query",
                "description": "Execute SQL query with explicit capability constraints. CAPABILITY HIERARCHY (must request appropriate level): (1) READ-ONLY (default, no flags): SELECT queries only, (2) WRITE (requires allow_write=true): enables INSERT, UPDATE, DELETE but NOT DDL, (3) DDL (requires allow_ddl=true): enables CREATE, DROP, ALTER, TRUNCATE; DDL implicitly grants write permissions. IMPORTANT SECURITY: You (the AI agent) are responsible for sanitizing all user inputs before constructing SQL - Plenum does NOT validate SQL safety, only enforces capability constraints. Connection resolution works like introspect: use 'name' for saved connections, explicit parameters, or mix both with overrides. Recommended safety practices: (1) Use max_rows to limit result sets for unknown queries, (2) Use timeout_ms to prevent long-running operations, (3) Start with read-only and only add write/DDL when necessary. Returns JSON with either query results (rows/columns) or rows_affected count. The connection is opened, query is executed, and connection is immediately closed (stateless). Possible error codes: CAPABILITY_VIOLATION (operation not permitted), QUERY_FAILED (SQL error), CONNECTION_FAILED (connection error).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL query to execute. REQUIRED. Must be valid, vendor-specific SQL (PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL). You (the agent) are responsible for sanitizing user inputs before constructing SQL - Plenum does not validate SQL safety."
                        },
                        "name": {
                            "type": "string",
                            "description": "Name of saved connection to use. Connection must exist in local (.plenum/config.json) or global (~/.config/plenum/connections.json) config. Cannot be used alone with 'engine' - choose one or mix name with overrides."
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine type. Required if not using 'name'. Valid values: 'postgres', 'mysql', 'sqlite'."
                        },
                        "host": {
                            "type": "string",
                            "description": "Database host (for postgres/mysql). Required for explicit connections or use as override. Example: 'localhost', 'db.example.com'."
                        },
                        "port": {
                            "type": "number",
                            "description": "Database port (for postgres/mysql). Required for explicit connections or use as override. Defaults: postgres=5432, mysql=3306."
                        },
                        "user": {
                            "type": "string",
                            "description": "Database username (for postgres/mysql). Required for explicit connections or use as override."
                        },
                        "password": {
                            "type": "string",
                            "description": "Database password (for postgres/mysql). Required for explicit connections or use as override. Passed directly - agent responsible for security."
                        },
                        "database": {
                            "type": "string",
                            "description": "Database name (for postgres/mysql). Required for explicit connections or use as override. The specific database to query."
                        },
                        "file": {
                            "type": "string",
                            "description": "File path to SQLite database file. Required for sqlite engine. Can be relative or absolute path. Example: './app.db', '/var/lib/data.db'."
                        },
                        "allow_write": {
                            "type": "boolean",
                            "description": "Enable write operations (INSERT, UPDATE, DELETE) but NOT DDL. Default: false (read-only). Set to true for data modifications. DDL operations (CREATE, DROP, ALTER) still blocked - use allow_ddl for those."
                        },
                        "allow_ddl": {
                            "type": "boolean",
                            "description": "Enable DDL operations (CREATE, DROP, ALTER, TRUNCATE). Default: false. DDL capability implicitly grants write permissions - no need to also set allow_write=true. Use this for schema changes."
                        },
                        "max_rows": {
                            "type": "number",
                            "description": "Optional: Maximum number of rows to return from SELECT queries. Recommended for queries against unknown tables to prevent excessive memory usage. Example: 1000. No limit if omitted."
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

        let conn_name =
            args["name"].as_str().map_or_else(|| "default".to_string(), std::string::ToString::to_string);

        crate::save_connection(conn_name.clone(), config.clone(), location)
            .map_err(|e| anyhow!("Failed to save connection: {e}"))?;

        Ok(serde_json::json!({
            "ok": true,
            "connection_name": conn_name,
            "engine": config.engine.as_str(),
            "saved_to": save_str,
            "connection_info": conn_info,
            "message": format!("Connection '{}' saved successfully", conn_name)
        }))
    } else {
        Ok(serde_json::json!({
            "ok": true,
            "engine": config.engine.as_str(),
            "connection_info": conn_info,
            "message": "Connection validated successfully"
        }))
    }
}

/// MCP Tool: introspect
///
/// Introspects database schema and returns table/column information.
async fn tool_introspect(args: &Value) -> Result<Value> {
    // Resolve connection config
    let config = resolve_connection_from_args(args)?;

    // Get schema filter
    let schema_filter = args.get("schema").and_then(|v| v.as_str());

    // Call introspect (opens and closes connection)
    let schema_info = introspect_schema(&config, schema_filter).await?;

    Ok(serde_json::to_value(schema_info)?)
}

/// MCP Tool: query
///
/// Executes a SQL query with capability constraints.
async fn tool_query(args: &Value) -> Result<Value> {
    // Extract SQL
    let sql = args["sql"].as_str().ok_or_else(|| anyhow!("Missing required field: sql"))?;

    // Resolve connection config
    let config = resolve_connection_from_args(args)?;

    // Build capabilities
    let capabilities = Capabilities {
        allow_write: args.get("allow_write").and_then(serde_json::Value::as_bool).unwrap_or(false),
        allow_ddl: args.get("allow_ddl").and_then(serde_json::Value::as_bool).unwrap_or(false),
        max_rows: args.get("max_rows").and_then(serde_json::Value::as_u64).map(|n| n as usize),
        timeout_ms: args.get("timeout_ms").and_then(serde_json::Value::as_u64),
    };

    // Validate query against capabilities (pre-execution check)
    crate::validate_query(sql, &capabilities, config.engine)
        .map_err(|e| anyhow!("Capability validation failed: {e}"))?;

    // Execute query (opens and closes connection)
    let query_result = execute_query(&config, sql, &capabilities).await?;

    Ok(serde_json::to_value(query_result)?)
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
/// This handles both named connections and explicit connection parameters.
fn resolve_connection_from_args(args: &Value) -> Result<ConnectionConfig> {
    // Try named connection first
    if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
        let mut config = crate::resolve_connection(name)
            .map_err(|e| anyhow!("Failed to resolve connection '{name}': {e}"))?;

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

        return Ok(config);
    }

    // No named connection - must have explicit engine
    let engine_str =
        args["engine"].as_str().ok_or_else(|| anyhow!("Must provide either 'name' or 'engine'"))?;

    build_connection_config_from_args(args, engine_str)
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

/// Introspect database schema
///
/// Opens a connection, introspects schema, and immediately closes it.
/// This function is stateless - no connection persists after it returns.
async fn introspect_schema(
    config: &ConnectionConfig,
    schema_filter: Option<&str>,
) -> Result<crate::SchemaInfo> {
    match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::introspect(config, schema_filter)
            .await
            .map_err(|e| anyhow!("SQLite introspection failed: {e}")),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(anyhow!("SQLite engine not enabled. Build with --features sqlite"))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::introspect(config, schema_filter)
            .await
            .map_err(|e| anyhow!("PostgreSQL introspection failed: {e}")),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(anyhow!("PostgreSQL engine not enabled. Build with --features postgres"))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::introspect(config, schema_filter)
            .await
            .map_err(|e| anyhow!("MySQL introspection failed: {e}")),
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
