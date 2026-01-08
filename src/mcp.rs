//! MCP (Model Context Protocol) Server
//!
//! This module implements an MCP server using manual JSON-RPC 2.0 over stdio.
//! We follow the proven pattern from reflex-search rather than using the unstable rmcp crate.
//!
//! # Architecture
//!
//! - **Transport**: JSON-RPC 2.0 over stdio (line-based)
//! - **Dependencies**: Only serde_json and anyhow (no MCP-specific crates)
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

use crate::{
    Capabilities, ConfigLocation, ConnectionConfig, DatabaseEngine, DatabaseType,
};

// Import database engines
#[cfg(feature = "sqlite")]
use crate::engine::sqlite::SqliteEngine;
#[cfg(feature = "postgres")]
use crate::engine::postgres::PostgresEngine;
#[cfg(feature = "mysql")]
use crate::engine::mysql::MySqlEngine;

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
                        message: format!("Parse error: {}", e),
                        data: None,
                    }),
                };
                let response_json = serde_json::to_string(&error_response)?;
                writeln!(stdout, "{}", response_json)?;
                stdout.flush()?;
                continue;
            }
        };

        // Handle request
        let response = handle_request(request).await;

        // Write response
        let response_json = serde_json::to_string(&response)?;
        writeln!(stdout, "{}", response_json)?;
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
                "description": "Validate and save database connection configuration. Supports PostgreSQL, MySQL, and SQLite.",
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
                            "description": "Save location: local (.plenum/config.json) or global (~/.config/plenum/connections.json)"
                        }
                    },
                    "required": ["engine"]
                }
            },
            {
                "name": "introspect",
                "description": "Introspect database schema and return table/column information.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "Named connection to use"
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine (if not using named connection)"
                        },
                        "host": {
                            "type": "string",
                            "description": "Host override"
                        },
                        "port": {
                            "type": "number",
                            "description": "Port override"
                        },
                        "user": {
                            "type": "string",
                            "description": "Username override"
                        },
                        "password": {
                            "type": "string",
                            "description": "Password override"
                        },
                        "database": {
                            "type": "string",
                            "description": "Database override"
                        },
                        "file": {
                            "type": "string",
                            "description": "File path override (for sqlite)"
                        },
                        "schema": {
                            "type": "string",
                            "description": "Schema filter (optional)"
                        }
                    }
                }
            },
            {
                "name": "query",
                "description": "Execute SQL query with capability constraints. Read-only by default; write and DDL require explicit flags.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "sql": {
                            "type": "string",
                            "description": "SQL query to execute (required)"
                        },
                        "name": {
                            "type": "string",
                            "description": "Named connection to use"
                        },
                        "engine": {
                            "type": "string",
                            "enum": ["postgres", "mysql", "sqlite"],
                            "description": "Database engine (if not using named connection)"
                        },
                        "host": {
                            "type": "string",
                            "description": "Host override"
                        },
                        "port": {
                            "type": "number",
                            "description": "Port override"
                        },
                        "user": {
                            "type": "string",
                            "description": "Username override"
                        },
                        "password": {
                            "type": "string",
                            "description": "Password override"
                        },
                        "database": {
                            "type": "string",
                            "description": "Database override"
                        },
                        "file": {
                            "type": "string",
                            "description": "File path override (for sqlite)"
                        },
                        "allow_write": {
                            "type": "boolean",
                            "description": "Enable write operations (INSERT, UPDATE, DELETE). Default: false"
                        },
                        "allow_ddl": {
                            "type": "boolean",
                            "description": "Enable DDL operations (CREATE, DROP, ALTER). Default: false"
                        },
                        "max_rows": {
                            "type": "number",
                            "description": "Maximum number of rows to return"
                        },
                        "timeout_ms": {
                            "type": "number",
                            "description": "Query timeout in milliseconds"
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
    let name = params["name"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing tool name"))?;
    let arguments = &params["arguments"];

    match name {
        "connect" => tool_connect(arguments).await,
        "introspect" => tool_introspect(arguments).await,
        "query" => tool_query(arguments).await,
        _ => Err(anyhow!("Unknown tool: {}", name)),
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
    let engine_str = args["engine"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required field: engine"))?;

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

        let conn_name = args["name"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        crate::save_connection(conn_name.clone(), config.clone(), location)
            .map_err(|e| anyhow!("Failed to save connection: {}", e))?;

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
    let sql = args["sql"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing required field: sql"))?;

    // Resolve connection config
    let config = resolve_connection_from_args(args)?;

    // Build capabilities
    let capabilities = Capabilities {
        allow_write: args.get("allow_write").and_then(|v| v.as_bool()).unwrap_or(false),
        allow_ddl: args.get("allow_ddl").and_then(|v| v.as_bool()).unwrap_or(false),
        max_rows: args.get("max_rows").and_then(|v| v.as_u64()).map(|n| n as usize),
        timeout_ms: args.get("timeout_ms").and_then(|v| v.as_u64()),
    };

    // Validate query against capabilities (pre-execution check)
    crate::validate_query(sql, &capabilities, config.engine)
        .map_err(|e| anyhow!("Capability validation failed: {}", e))?;

    // Execute query (opens and closes connection)
    let query_result = execute_query(&config, sql, &capabilities).await?;

    Ok(serde_json::to_value(query_result)?)
}

// ============================================================================
// Helper Functions (Stateless)
// ============================================================================

/// Build ConnectionConfig from JSON arguments
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
                .ok_or_else(|| anyhow!("Missing required field for {}: host", engine_str))?
                .to_string();
            let port = args["port"]
                .as_u64()
                .ok_or_else(|| anyhow!("Missing required field for {}: port", engine_str))?
                as u16;
            let user = args["user"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {}: user", engine_str))?
                .to_string();
            let password = args["password"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {}: password", engine_str))?
                .to_string();
            let database = args["database"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing required field for {}: database", engine_str))?
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
            .map_err(|e| anyhow!("Failed to resolve connection '{}': {}", name, e))?;

        // Apply overrides
        if let Some(eng) = args.get("engine").and_then(|v| v.as_str()) {
            config.engine = match eng {
                "postgres" => DatabaseType::Postgres,
                "mysql" => DatabaseType::MySQL,
                "sqlite" => DatabaseType::SQLite,
                _ => return Err(anyhow!("Invalid engine: {}", eng)),
            };
        }
        if let Some(h) = args.get("host").and_then(|v| v.as_str()) {
            config.host = Some(h.to_string());
        }
        if let Some(p) = args.get("port").and_then(|v| v.as_u64()) {
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
    let engine_str = args["engine"]
        .as_str()
        .ok_or_else(|| anyhow!("Must provide either 'name' or 'engine'"))?;

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
            .map_err(|e| anyhow!("SQLite connection failed: {}", e)),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => Err(anyhow!(
            "SQLite engine not enabled. Build with --features sqlite"
        )),

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("PostgreSQL connection failed: {}", e)),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => Err(anyhow!(
            "PostgreSQL engine not enabled. Build with --features postgres"
        )),

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::validate_connection(config)
            .await
            .map_err(|e| anyhow!("MySQL connection failed: {}", e)),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => Err(anyhow!(
            "MySQL engine not enabled. Build with --features mysql"
        )),
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
            .map_err(|e| anyhow!("SQLite introspection failed: {}", e)),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => Err(anyhow!(
            "SQLite engine not enabled. Build with --features sqlite"
        )),

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::introspect(config, schema_filter)
            .await
            .map_err(|e| anyhow!("PostgreSQL introspection failed: {}", e)),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => Err(anyhow!(
            "PostgreSQL engine not enabled. Build with --features postgres"
        )),

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::introspect(config, schema_filter)
            .await
            .map_err(|e| anyhow!("MySQL introspection failed: {}", e)),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => Err(anyhow!(
            "MySQL engine not enabled. Build with --features mysql"
        )),
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
            .map_err(|e| anyhow!("SQLite query failed: {}", e)),
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => Err(anyhow!(
            "SQLite engine not enabled. Build with --features sqlite"
        )),

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::execute(config, sql, capabilities)
            .await
            .map_err(|e| anyhow!("PostgreSQL query failed: {}", e)),
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => Err(anyhow!(
            "PostgreSQL engine not enabled. Build with --features postgres"
        )),

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::execute(config, sql, capabilities)
            .await
            .map_err(|e| anyhow!("MySQL query failed: {}", e)),
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => Err(anyhow!(
            "MySQL engine not enabled. Build with --features mysql"
        )),
    }
}
