//! Plenum CLI Entry Point
//!
//! This is the main binary entry point for the Plenum CLI.
//! It provides four subcommands:
//! - `connect` - Database connection configuration management
//! - `introspect` - Schema introspection
//! - `query` - Constrained query execution
//! - `mcp` - MCP server mode (hidden, for AI agent integration)
//!
//! All output to stdout is JSON-only. Logs go to stderr.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

use plenum::{
    Capabilities, ConfigLocation, ConnectionConfig, DatabaseEngine, DatabaseType, ErrorEnvelope,
    Metadata, PlenumError, Result, SuccessEnvelope,
};

// Import database engines
#[cfg(feature = "mysql")]
use plenum::engine::mysql::MySqlEngine;
#[cfg(feature = "postgres")]
use plenum::engine::postgres::PostgresEngine;
#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

/// Plenum - Agent-First Database Control CLI
#[derive(Parser)]
#[command(name = "plenum")]
#[command(about = "Agent-first database control CLI with least-privilege execution")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Configure and validate database connections
    Connect {
        /// Connection name (optional, defaults to "default")
        #[arg(long)]
        name: Option<String>,

        /// Project path (optional, defaults to current directory)
        #[arg(long)]
        project_path: Option<String>,

        /// Database engine
        #[arg(long, value_parser = ["postgres", "mysql", "sqlite"])]
        engine: Option<String>,

        /// Host (postgres/mysql)
        #[arg(long)]
        host: Option<String>,

        /// Port (postgres/mysql)
        #[arg(long)]
        port: Option<u16>,

        /// Username (postgres/mysql)
        #[arg(long)]
        user: Option<String>,

        /// Password (postgres/mysql)
        #[arg(long)]
        password: Option<String>,

        /// Password from environment variable
        #[arg(long)]
        password_env: Option<String>,

        /// Database name (postgres/mysql)
        #[arg(long)]
        database: Option<String>,

        /// `SQLite` file path
        #[arg(long)]
        file: Option<PathBuf>,

        /// Save location (local or global)
        #[arg(long, value_parser = ["local", "global"])]
        save: Option<String>,
    },

    /// Introspect database schema
    Introspect {
        /// Connection name (optional, defaults to "default")
        #[arg(long)]
        name: Option<String>,

        /// Project path (optional, defaults to current directory)
        #[arg(long)]
        project_path: Option<String>,

        /// Engine override
        #[arg(long, value_parser = ["postgres", "mysql", "sqlite"])]
        engine: Option<String>,

        /// Host override
        #[arg(long)]
        host: Option<String>,

        /// Port override
        #[arg(long)]
        port: Option<u16>,

        /// Username override
        #[arg(long)]
        user: Option<String>,

        /// Password override
        #[arg(long)]
        password: Option<String>,

        /// Database override
        #[arg(long)]
        database: Option<String>,

        /// `SQLite` file override
        #[arg(long)]
        file: Option<PathBuf>,

        /// Schema filter
        #[arg(long)]
        schema: Option<String>,
    },

    /// Execute constrained SQL queries
    Query {
        /// Connection name (optional, defaults to "default")
        #[arg(long)]
        name: Option<String>,

        /// Project path (optional, defaults to current directory)
        #[arg(long)]
        project_path: Option<String>,

        /// Engine override
        #[arg(long, value_parser = ["postgres", "mysql", "sqlite"])]
        engine: Option<String>,

        /// Host override
        #[arg(long)]
        host: Option<String>,

        /// Port override
        #[arg(long)]
        port: Option<u16>,

        /// Username override
        #[arg(long)]
        user: Option<String>,

        /// Password override
        #[arg(long)]
        password: Option<String>,

        /// Database override
        #[arg(long)]
        database: Option<String>,

        /// `SQLite` file override
        #[arg(long)]
        file: Option<PathBuf>,

        /// SQL query (mutually exclusive with --sql-file)
        #[arg(long, conflicts_with = "sql_file")]
        sql: Option<String>,

        /// SQL file path
        #[arg(long)]
        sql_file: Option<PathBuf>,

        /// Allow write operations
        #[arg(long)]
        allow_write: bool,

        /// Allow DDL operations
        #[arg(long)]
        allow_ddl: bool,

        /// Max rows to return
        #[arg(long)]
        max_rows: Option<usize>,

        /// Query timeout in milliseconds
        #[arg(long)]
        timeout_ms: Option<u64>,
    },

    /// Start MCP server (hidden from help, for AI agent integration)
    #[command(hide = true)]
    Mcp,
}

#[tokio::main]
async fn main() {
    // Parse CLI arguments
    let cli = Cli::parse();

    // Set up panic handler to convert panics to error envelopes
    std::panic::set_hook(Box::new(|panic_info| {
        let error_envelope = ErrorEnvelope::new(
            "",
            "unknown",
            plenum::ErrorInfo::new("INTERNAL_ERROR", format!("Internal error: {panic_info}")),
        );
        output_error(&error_envelope);
    }));

    // Route to command handlers
    let result = match cli.command {
        Some(Commands::Connect {
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            password_env,
            database,
            file,
            save,
        }) => {
            handle_connect(
                name,
                project_path,
                engine,
                host,
                port,
                user,
                password,
                password_env,
                database,
                file,
                save,
            )
            .await
        }
        Some(Commands::Introspect {
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            schema,
        }) => {
            handle_introspect(
                name,
                project_path,
                engine,
                host,
                port,
                user,
                password,
                database,
                file,
                schema,
            )
            .await
        }
        Some(Commands::Query {
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            sql,
            sql_file,
            allow_write,
            allow_ddl,
            max_rows,
            timeout_ms,
        }) => {
            handle_query(
                name,
                project_path,
                engine,
                host,
                port,
                user,
                password,
                database,
                file,
                sql,
                sql_file,
                allow_write,
                allow_ddl,
                max_rows,
                timeout_ms,
            )
            .await
        }
        Some(Commands::Mcp) => handle_mcp().await,
        None => {
            // No subcommand provided
            let error_envelope = ErrorEnvelope::new(
                "",
                "unknown",
                plenum::ErrorInfo::new(
                    "NO_SUBCOMMAND",
                    "No subcommand provided. Use --help to see available commands.",
                ),
            );
            output_error(&error_envelope);
            std::process::exit(1);
        }
    };

    // Handle result
    if let Err(exit_code) = result {
        std::process::exit(exit_code);
    }
}

// ============================================================================
// Command Handlers
// ============================================================================

async fn handle_connect(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
    save: Option<String>,
) -> std::result::Result<(), i32> {
    // Start timing
    let start = Instant::now();

    // Determine mode: interactive or non-interactive
    let has_args = engine.is_some()
        || host.is_some()
        || port.is_some()
        || user.is_some()
        || password.is_some()
        || password_env.is_some()
        || database.is_some()
        || file.is_some()
        || save.is_some();

    let result: Result<(String, Option<String>, ConnectionConfig, ConfigLocation)> = if has_args {
        // Non-interactive mode: build from args
        non_interactive_connect(
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            password_env,
            database,
            file,
            save,
        )
        .await
    } else {
        // Interactive mode: show picker
        interactive_connect_picker().await
    };

    match result {
        Ok((conn_name, proj_path, config, location)) => {
            // Save connection
            match plenum::save_connection(proj_path, Some(conn_name.clone()), config.clone(), location) {
                Ok(()) => {
                    // Build success response
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let data = serde_json::json!({
                        "connection_name": conn_name,
                        "engine": config.engine.as_str(),
                        "saved_to": match location {
                            ConfigLocation::Local => "local",
                            ConfigLocation::Global => "global",
                        },
                        "message": format!("Connection '{}' saved successfully", conn_name),
                    });

                    let envelope = SuccessEnvelope::new(
                        config.engine.as_str(),
                        "connect",
                        data,
                        Metadata::new(elapsed_ms),
                    );
                    output_success(&envelope);
                    Ok(())
                }
                Err(e) => {
                    let envelope = ErrorEnvelope::from_error("", "connect", &e);
                    output_error(&envelope);
                    Err(1)
                }
            }
        }
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "connect", &e);
            output_error(&envelope);
            Err(1)
        }
    }
}

/// Interactive connection picker (when no args provided)
async fn interactive_connect_picker() -> Result<(String, Option<String>, ConnectionConfig, ConfigLocation)> {
    use dialoguer::Select;

    // Get current project path
    let current_project_path = plenum::config::get_current_project_path()?;

    // Load existing connections for the current project only
    let connections = plenum::list_connections_for_project(&current_project_path)?;

    if connections.is_empty() {
        // No existing connections, go straight to wizard
        eprintln!("No existing connections found. Let's create one.");
        return interactive_connect_wizard().await;
    }

    // Build menu (without showing project path since we're already in the project)
    let mut items: Vec<String> = connections
        .iter()
        .map(|(name, config)| {
            format!(
                "{} - {}://{}",
                name,
                config.engine.as_str(),
                match &config.engine {
                    DatabaseType::Postgres | DatabaseType::MySQL => {
                        format!(
                            "{}@{}:{}",
                            config.user.as_ref().unwrap_or(&"?".to_string()),
                            config.host.as_ref().unwrap_or(&"?".to_string()),
                            config.port.unwrap_or(0)
                        )
                    }
                    DatabaseType::SQLite => {
                        config.file.as_ref().and_then(|f| f.to_str()).unwrap_or("?").to_string()
                    }
                }
            )
        })
        .collect();
    items.push("--- Create New Connection ---".to_string());

    let selection = Select::new()
        .with_prompt("Select a connection")
        .items(&items)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {e}")))?;

    if selection == connections.len() {
        // User selected "Create New"
        interactive_connect_wizard().await
    } else {
        // User selected an existing connection - we'll validate and re-save it
        let (name, config) = &connections[selection];

        // Ask if they want to update it
        eprintln!("Connection '{name}' already exists. Re-validating configuration.");

        // Ask for save location
        let location = prompt_save_location()?;

        Ok((name.clone(), None, config.clone(), location))
    }
}

/// Interactive connection wizard
async fn interactive_connect_wizard() -> Result<(String, Option<String>, ConnectionConfig, ConfigLocation)> {
    use dialoguer::{Input, Select};

    eprintln!("\n=== Create New Database Connection ===\n");

    // Prompt for engine
    let engine_choices = vec!["postgres", "mysql", "sqlite"];
    let engine_idx = Select::new()
        .with_prompt("Select database engine")
        .items(&engine_choices)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {e}")))?;
    let engine = parse_engine(engine_choices[engine_idx])?;

    // Build config based on engine type
    let config = match engine {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host: String = Input::new()
                .with_prompt("Host")
                .default("localhost".to_string())
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let port: u16 = Input::new()
                .with_prompt("Port")
                .default(if engine == DatabaseType::Postgres { 5432 } else { 3306 })
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let user: String = Input::new()
                .with_prompt("Username")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let password: String = dialoguer::Password::new()
                .with_prompt("Password")
                .interact()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let database: String = Input::new()
                .with_prompt("Database name")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            if engine == DatabaseType::Postgres {
                ConnectionConfig::postgres(host, port, user, password, database)
            } else {
                ConnectionConfig::mysql(host, port, user, password, database)
            }
        }
        DatabaseType::SQLite => {
            let file: String = Input::new()
                .with_prompt("Database file path")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            ConnectionConfig::sqlite(PathBuf::from(file))
        }
    };

    // Prompt for connection name
    let name: String = Input::new()
        .with_prompt("Connection name (defaults to 'default')")
        .default("default".to_string())
        .interact_text()
        .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

    // Prompt for save location
    let location = prompt_save_location()?;

    // Use None for project_path (will default to current directory)
    Ok((name, None, config, location))
}

/// Non-interactive connect (with CLI args)
async fn non_interactive_connect(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    _password_env: Option<String>, // TODO: Implement password_env support
    database: Option<String>,
    file: Option<PathBuf>,
    save: Option<String>,
) -> Result<(String, Option<String>, ConnectionConfig, ConfigLocation)> {
    // Validate required arguments
    let engine_str = engine.ok_or_else(|| {
        PlenumError::invalid_input("--engine is required for non-interactive mode")
    })?;
    let engine_type = parse_engine(&engine_str)?;

    // Build config based on engine
    let config = match engine_type {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host = host.ok_or_else(|| {
                PlenumError::invalid_input("--host is required for postgres/mysql")
            })?;
            let port = port.ok_or_else(|| {
                PlenumError::invalid_input("--port is required for postgres/mysql")
            })?;
            let user = user.ok_or_else(|| {
                PlenumError::invalid_input("--user is required for postgres/mysql")
            })?;
            let password = password.ok_or_else(|| {
                PlenumError::invalid_input("--password is required for postgres/mysql")
            })?;
            let database = database.ok_or_else(|| {
                PlenumError::invalid_input("--database is required for postgres/mysql")
            })?;

            if engine_type == DatabaseType::Postgres {
                ConnectionConfig::postgres(host, port, user, password, database)
            } else {
                ConnectionConfig::mysql(host, port, user, password, database)
            }
        }
        DatabaseType::SQLite => {
            let file =
                file.ok_or_else(|| PlenumError::invalid_input("--file is required for sqlite"))?;
            ConnectionConfig::sqlite(file)
        }
    };

    // Determine connection name (defaults to "default")
    let conn_name = name.unwrap_or_else(|| "default".to_string());

    // Parse save location
    let location = match save.as_deref() {
        Some("local") | None => ConfigLocation::Local, // Default to local
        Some("global") => ConfigLocation::Global,
        Some(other) => {
            return Err(PlenumError::invalid_input(format!(
                "Invalid save location '{other}'. Must be 'local' or 'global'"
            )))
        }
    };

    Ok((conn_name, project_path, config, location))
}

/// Prompt user for save location
fn prompt_save_location() -> Result<ConfigLocation> {
    use dialoguer::Select;

    let choices = vec!["local (.plenum/config.json)", "global (~/.config/plenum/connections.json)"];
    let selection = Select::new()
        .with_prompt("Save location")
        .items(&choices)
        .default(0)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {e}")))?;

    Ok(if selection == 0 { ConfigLocation::Local } else { ConfigLocation::Global })
}

async fn handle_introspect(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
    schema: Option<String>,
) -> std::result::Result<(), i32> {
    // Start timing
    let start = Instant::now();

    // Resolve connection config
    let config_result = build_connection_config(
        name.as_deref(),
        project_path.as_deref(),
        engine,
        host,
        port,
        user,
        password,
        database,
        file,
    );

    let (config, _is_readonly) = match config_result {
        Ok(cfg_tuple) => cfg_tuple,
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "introspect", &e);
            output_error(&envelope);
            return Err(1);
        }
    };

    // Call appropriate database engine for introspection
    let introspect_result = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::introspect(&config, schema.as_deref()).await,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(PlenumError::invalid_input(
                "SQLite engine not enabled. Build with --features sqlite to enable SQLite support."
            ))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::introspect(&config, schema.as_deref()).await,
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(PlenumError::invalid_input(
                "PostgreSQL engine not enabled. Build with --features postgres to enable PostgreSQL support."
            ))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::introspect(&config, schema.as_deref()).await,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(PlenumError::invalid_input(
                "MySQL engine not enabled. Build with --features mysql to enable MySQL support."
            ))
        }
    };

    match introspect_result {
        Ok(schema_info) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let envelope = SuccessEnvelope::new(
                config.engine.as_str(),
                "introspect",
                schema_info,
                Metadata::new(elapsed_ms),
            );
            output_success(&envelope);
            Ok(())
        }
        Err(e) => {
            let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "introspect", &e);
            output_error(&envelope);
            Err(1)
        }
    }
}

async fn handle_query(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
    sql: Option<String>,
    sql_file: Option<PathBuf>,
    allow_write: bool,
    allow_ddl: bool,
    max_rows: Option<usize>,
    timeout_ms: Option<u64>,
) -> std::result::Result<(), i32> {
    // Start timing
    let start = Instant::now();

    // Resolve SQL input
    let sql_text = match (sql, sql_file) {
        (Some(s), None) => s,
        (None, Some(path)) => match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                let envelope = ErrorEnvelope::new(
                    "",
                    "query",
                    plenum::ErrorInfo::new(
                        "INVALID_INPUT",
                        format!("Could not read SQL file: {e}"),
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }
        },
        (Some(_), Some(_)) => {
            // This should be prevented by clap's conflicts_with, but check anyway
            let envelope = ErrorEnvelope::new(
                "",
                "query",
                plenum::ErrorInfo::new("INVALID_INPUT", "Cannot specify both --sql and --sql-file"),
            );
            output_error(&envelope);
            return Err(1);
        }
        (None, None) => {
            let envelope = ErrorEnvelope::new(
                "",
                "query",
                plenum::ErrorInfo::new("INVALID_INPUT", "Either --sql or --sql-file is required"),
            );
            output_error(&envelope);
            return Err(1);
        }
    };

    // Resolve connection config
    let (config, is_readonly) = match build_connection_config(
        name.as_deref(),
        project_path.as_deref(),
        engine,
        host,
        port,
        user,
        password,
        database,
        file,
    ) {
        Ok(cfg_tuple) => cfg_tuple,
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "query", &e);
            output_error(&envelope);
            return Err(1);
        }
    };

    // Enforce readonly mode: if connection is readonly, reject write/DDL operations
    if is_readonly && (allow_write || allow_ddl) {
        let conn_name = name.unwrap_or_else(|| "default".to_string());
        let envelope = ErrorEnvelope::new(
            config.engine.as_str(),
            "query",
            plenum::ErrorInfo::new(
                "READONLY_VIOLATION",
                format!(
                    "Connection '{conn_name}' is configured as readonly. Write and DDL operations are not permitted."
                ),
            ),
        );
        output_error(&envelope);
        return Err(1);
    }

    // Build capabilities (forced to read-only if connection is readonly)
    let capabilities = if is_readonly {
        // Force read-only mode, ignore flags
        Capabilities { allow_write: false, allow_ddl: false, max_rows, timeout_ms }
    } else {
        build_capabilities(allow_write, allow_ddl, max_rows, timeout_ms)
    };

    // Validate query against capabilities
    match plenum::validate_query(&sql_text, &capabilities, config.engine) {
        Ok(_category) => {
            // Query is valid according to capabilities
        }
        Err(e) => {
            let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "query", &e);
            output_error(&envelope);
            return Err(1);
        }
    }

    // Call appropriate database engine for query execution
    let execute_result = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::execute(&config, &sql_text, &capabilities).await,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(PlenumError::invalid_input(
                "SQLite engine not enabled. Build with --features sqlite to enable SQLite support."
            ))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::execute(&config, &sql_text, &capabilities).await,
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(PlenumError::invalid_input(
                "PostgreSQL engine not enabled. Build with --features postgres to enable PostgreSQL support."
            ))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::execute(&config, &sql_text, &capabilities).await,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(PlenumError::invalid_input(
                "MySQL engine not enabled. Build with --features mysql to enable MySQL support."
            ))
        }
    };

    match execute_result {
        Ok(query_result) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let row_count = query_result.rows.len();
            let envelope = SuccessEnvelope::new(
                config.engine.as_str(),
                "query",
                query_result,
                Metadata::with_rows(elapsed_ms, row_count),
            );
            output_success(&envelope);
            Ok(())
        }
        Err(e) => {
            let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "query", &e);
            output_error(&envelope);
            Err(1)
        }
    }
}

#[allow(clippy::future_not_send)]
async fn handle_mcp() -> std::result::Result<(), i32> {
    // Phase 7: MCP server using manual JSON-RPC 2.0 implementation
    // Follows the proven pattern from reflex-search (no unstable rmcp dependency)
    match plenum::mcp::serve().await {
        Ok(()) => Ok(()),
        Err(e) => {
            // MCP server errors go to stderr (not stdout, which is for JSON-RPC)
            eprintln!("MCP server error: {e}");
            Err(1)
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Output a success envelope as JSON to stdout
fn output_success<T: serde::Serialize>(envelope: &SuccessEnvelope<T>) {
    match serde_json::to_string(envelope) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("FATAL: Failed to serialize success envelope: {e}");
            std::process::exit(2);
        }
    }
}

/// Output an error envelope as JSON to stdout
fn output_error(envelope: &ErrorEnvelope) {
    match serde_json::to_string(envelope) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("FATAL: Failed to serialize error envelope: {e}");
            std::process::exit(2);
        }
    }
}

/// Measure execution time of a function
fn measure_execution<F, T>(f: F) -> (std::result::Result<T, PlenumError>, u64)
where
    F: FnOnce() -> std::result::Result<T, PlenumError>,
{
    let start = Instant::now();
    let result = f();
    let elapsed_ms = start.elapsed().as_millis() as u64;
    (result, elapsed_ms)
}

/// Build connection config from CLI arguments
///
/// This helper resolves a connection from config or builds one from CLI arguments.
/// Precedence: Named connection at project path → CLI arguments only
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
fn build_connection_config(
    name: Option<&str>,
    project_path: Option<&str>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
) -> Result<(ConnectionConfig, bool)> {
    let has_explicit_args = engine.is_some()
        || host.is_some()
        || port.is_some()
        || user.is_some()
        || password.is_some()
        || database.is_some()
        || file.is_some();

    // Try to resolve from config if name or project_path is provided, or if no explicit args
    let should_try_resolve = name.is_some() || project_path.is_some() || !has_explicit_args;

    let mut resolved_connection: Option<(ConnectionConfig, bool)> = if should_try_resolve {
        // Try to load connection from config
        match plenum::resolve_connection(project_path, name) {
            Ok(cfg_tuple) => Some(cfg_tuple),
            Err(_) if has_explicit_args => None, // Ignore error if explicit args provided as fallback
            Err(e) => return Err(e),             // Propagate error if no fallback
        }
    } else {
        None
    };

    // Apply CLI overrides
    if let Some((ref mut cfg, is_readonly)) = resolved_connection {
        // Override engine if provided
        if let Some(eng) = engine {
            cfg.engine = parse_engine(&eng)?;
        }
        // Override connection parameters
        if host.is_some() {
            cfg.host = host;
        }
        if port.is_some() {
            cfg.port = port;
        }
        if user.is_some() {
            cfg.user = user;
        }
        if password.is_some() {
            cfg.password = password;
        }
        if database.is_some() {
            cfg.database = database;
        }
        if file.is_some() {
            cfg.file = file;
        }
        return Ok((cfg.clone(), is_readonly));
    }

    // No config found, build from CLI arguments only
    // CLI-only connections are never readonly (readonly=false)
    let engine_type = engine.ok_or_else(|| {
        PlenumError::invalid_input(
            "--engine is required when not using a saved connection or explicit connection parameters"
        )
    })?;
    let engine = parse_engine(&engine_type)?;

    let config = match engine {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host = host.ok_or_else(|| {
                PlenumError::invalid_input("--host is required for postgres/mysql")
            })?;
            let port = port.ok_or_else(|| {
                PlenumError::invalid_input("--port is required for postgres/mysql")
            })?;
            let user = user.ok_or_else(|| {
                PlenumError::invalid_input("--user is required for postgres/mysql")
            })?;
            let password = password.ok_or_else(|| {
                PlenumError::invalid_input("--password is required for postgres/mysql")
            })?;
            let database = database.ok_or_else(|| {
                PlenumError::invalid_input("--database is required for postgres/mysql")
            })?;

            if engine == DatabaseType::Postgres {
                ConnectionConfig::postgres(host, port, user, password, database)
            } else {
                ConnectionConfig::mysql(host, port, user, password, database)
            }
        }
        DatabaseType::SQLite => {
            let file =
                file.ok_or_else(|| PlenumError::invalid_input("--file is required for sqlite"))?;
            ConnectionConfig::sqlite(file)
        }
    };

    Ok((config, false)) // CLI-only connections are never readonly
}

/// Parse engine string to `DatabaseType`
fn parse_engine(engine: &str) -> Result<DatabaseType> {
    match engine {
        "postgres" => Ok(DatabaseType::Postgres),
        "mysql" => Ok(DatabaseType::MySQL),
        "sqlite" => Ok(DatabaseType::SQLite),
        _ => Err(PlenumError::invalid_input(format!(
            "Invalid engine '{engine}'. Must be postgres, mysql, or sqlite"
        ))),
    }
}

/// Build capabilities from CLI flags
const fn build_capabilities(
    allow_write: bool,
    allow_ddl: bool,
    max_rows: Option<usize>,
    timeout_ms: Option<u64>,
) -> Capabilities {
    Capabilities { allow_write, allow_ddl, max_rows, timeout_ms }
}
