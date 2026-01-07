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
    Capabilities, ConfigLocation, ConnectionConfig, DatabaseType, ErrorEnvelope, Metadata,
    PlenumError, Result, SuccessEnvelope,
};

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
        /// Connection name (optional)
        #[arg(long)]
        name: Option<String>,

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

        /// SQLite file path
        #[arg(long)]
        file: Option<PathBuf>,

        /// Save location (local or global)
        #[arg(long, value_parser = ["local", "global"])]
        save: Option<String>,
    },

    /// Introspect database schema
    Introspect {
        /// Named connection
        #[arg(long)]
        name: Option<String>,

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

        /// SQLite file override
        #[arg(long)]
        file: Option<PathBuf>,

        /// Schema filter
        #[arg(long)]
        schema: Option<String>,
    },

    /// Execute constrained SQL queries
    Query {
        /// Named connection
        #[arg(long)]
        name: Option<String>,

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

        /// SQLite file override
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
            plenum::ErrorInfo::new(
                "INTERNAL_ERROR",
                format!("Internal error: {}", panic_info),
            ),
        );
        output_error(&error_envelope);
    }));

    // Route to command handlers
    let result = match cli.command {
        Some(Commands::Connect {
            name,
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
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            schema,
        }) => {
            handle_introspect(name, engine, host, port, user, password, database, file, schema)
                .await
        }
        Some(Commands::Query {
            name,
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

    let result: Result<(String, ConnectionConfig, ConfigLocation)> = if !has_args {
        // Interactive mode: show picker
        interactive_connect_picker().await
    } else {
        // Non-interactive mode: build from args
        non_interactive_connect(
            name,
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
    };

    match result {
        Ok((conn_name, config, location)) => {
            // Save connection
            match plenum::save_connection(conn_name.clone(), config.clone(), location, true) {
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
async fn interactive_connect_picker() -> Result<(String, ConnectionConfig, ConfigLocation)> {
    use dialoguer::Select;

    // Load existing connections
    let connections = plenum::list_connections()?;

    if connections.is_empty() {
        // No existing connections, go straight to wizard
        eprintln!("No existing connections found. Let's create one.");
        return interactive_connect_wizard().await;
    }

    // Build menu
    let mut items: Vec<String> = connections
        .iter()
        .map(|(name, config)| {
            format!(
                "{} ({}://{})",
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
                        format!(
                            "{}",
                            config
                                .file
                                .as_ref()
                                .and_then(|f| f.to_str())
                                .unwrap_or("?")
                        )
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
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {}", e)))?;

    if selection == connections.len() {
        // User selected "Create New"
        interactive_connect_wizard().await
    } else {
        // User selected an existing connection - we'll validate and re-save it
        let (name, config) = &connections[selection];

        // Ask if they want to update it
        eprintln!("Connection '{}' already exists. Re-validating configuration.", name);

        // Ask for save location
        let location = prompt_save_location()?;

        Ok((name.clone(), config.clone(), location))
    }
}

/// Interactive connection wizard
async fn interactive_connect_wizard() -> Result<(String, ConnectionConfig, ConfigLocation)> {
    use dialoguer::{Input, Select};

    eprintln!("\n=== Create New Database Connection ===\n");

    // Prompt for engine
    let engine_choices = vec!["postgres", "mysql", "sqlite"];
    let engine_idx = Select::new()
        .with_prompt("Select database engine")
        .items(&engine_choices)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {}", e)))?;
    let engine = parse_engine(engine_choices[engine_idx])?;

    // Build config based on engine type
    let config = match engine {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host: String = Input::new()
                .with_prompt("Host")
                .default("localhost".to_string())
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

            let port: u16 = Input::new()
                .with_prompt("Port")
                .default(if engine == DatabaseType::Postgres {
                    5432
                } else {
                    3306
                })
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

            let user: String = Input::new()
                .with_prompt("Username")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

            let password: String = Input::new()
                .with_prompt("Password")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

            let database: String = Input::new()
                .with_prompt("Database name")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

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
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

            ConnectionConfig::sqlite(PathBuf::from(file))
        }
    };

    // Prompt for connection name
    let name: String = Input::new()
        .with_prompt("Connection name")
        .default("default".to_string())
        .interact_text()
        .map_err(|e| PlenumError::invalid_input(format!("Input failed: {}", e)))?;

    // Prompt for save location
    let location = prompt_save_location()?;

    Ok((name, config, location))
}

/// Non-interactive connect (with CLI args)
async fn non_interactive_connect(
    name: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    _password_env: Option<String>, // TODO: Implement password_env support
    database: Option<String>,
    file: Option<PathBuf>,
    save: Option<String>,
) -> Result<(String, ConnectionConfig, ConfigLocation)> {
    // Validate required arguments
    let engine_str = engine.ok_or_else(|| {
        PlenumError::invalid_input("--engine is required for non-interactive mode")
    })?;
    let engine_type = parse_engine(&engine_str)?;

    // Build config based on engine
    let config = match engine_type {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host =
                host.ok_or_else(|| PlenumError::invalid_input("--host is required for postgres/mysql"))?;
            let port =
                port.ok_or_else(|| PlenumError::invalid_input("--port is required for postgres/mysql"))?;
            let user =
                user.ok_or_else(|| PlenumError::invalid_input("--user is required for postgres/mysql"))?;
            let password = password
                .ok_or_else(|| PlenumError::invalid_input("--password is required for postgres/mysql"))?;
            let database = database
                .ok_or_else(|| PlenumError::invalid_input("--database is required for postgres/mysql"))?;

            if engine_type == DatabaseType::Postgres {
                ConnectionConfig::postgres(host, port, user, password, database)
            } else {
                ConnectionConfig::mysql(host, port, user, password, database)
            }
        }
        DatabaseType::SQLite => {
            let file = file.ok_or_else(|| PlenumError::invalid_input("--file is required for sqlite"))?;
            ConnectionConfig::sqlite(file)
        }
    };

    // Determine connection name
    let conn_name = name.unwrap_or_else(|| "default".to_string());

    // Parse save location
    let location = match save.as_deref() {
        Some("local") => ConfigLocation::Local,
        Some("global") => ConfigLocation::Global,
        Some(other) => {
            return Err(PlenumError::invalid_input(format!(
                "Invalid save location '{}'. Must be 'local' or 'global'",
                other
            )))
        }
        None => ConfigLocation::Local, // Default to local
    };

    Ok((conn_name, config, location))
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
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {}", e)))?;

    Ok(if selection == 0 {
        ConfigLocation::Local
    } else {
        ConfigLocation::Global
    })
}

async fn handle_introspect(
    name: Option<String>,
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
        name,
        engine,
        host,
        port,
        user,
        password,
        database,
        file,
    );

    let config = match config_result {
        Ok(cfg) => cfg,
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "introspect", &e);
            output_error(&envelope);
            return Err(1);
        }
    };

    // TODO: Phase 3-5: Call DatabaseEngine::introspect() once engines are implemented
    // For now, return a "not implemented" error since engines don't exist yet
    let error_envelope = ErrorEnvelope::new(
        config.engine.as_str(),
        "introspect",
        plenum::ErrorInfo::new(
            "NOT_IMPLEMENTED",
            format!(
                "Database engine '{}' not yet implemented. Introspection will be available in Phase 3-5.",
                config.engine.as_str()
            ),
        ),
    );

    output_error(&error_envelope);
    Err(1)

    // Future implementation (Phase 3-5):
    // match DatabaseEngine::introspect(&config, schema.as_deref()) {
    //     Ok(schema_info) => {
    //         let elapsed_ms = start.elapsed().as_millis() as u64;
    //         let envelope = SuccessEnvelope::new(
    //             config.engine.as_str(),
    //             "introspect",
    //             schema_info,
    //             Metadata::new(elapsed_ms),
    //         );
    //         output_success(&envelope);
    //         Ok(())
    //     }
    //     Err(e) => {
    //         let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "introspect", &e);
    //         output_error(&envelope);
    //         Err(1)
    //     }
    // }
}

async fn handle_query(
    name: Option<String>,
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
                    plenum::ErrorInfo::new("INVALID_INPUT", format!("Could not read SQL file: {}", e)),
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
                plenum::ErrorInfo::new(
                    "INVALID_INPUT",
                    "Cannot specify both --sql and --sql-file",
                ),
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
    let config = match build_connection_config(name, engine, host, port, user, password, database, file) {
        Ok(cfg) => cfg,
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "query", &e);
            output_error(&envelope);
            return Err(1);
        }
    };

    // Build capabilities
    let capabilities = build_capabilities(allow_write, allow_ddl, max_rows, timeout_ms);

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

    // TODO: Phase 3-5: Call DatabaseEngine::execute() once engines are implemented
    // For now, return a "not implemented" error since engines don't exist yet
    let error_envelope = ErrorEnvelope::new(
        config.engine.as_str(),
        "query",
        plenum::ErrorInfo::new(
            "NOT_IMPLEMENTED",
            format!(
                "Database engine '{}' not yet implemented. Query execution will be available in Phase 3-5.",
                config.engine.as_str()
            ),
        ),
    );

    output_error(&error_envelope);
    Err(1)

    // Future implementation (Phase 3-5):
    // match DatabaseEngine::execute(&config, &sql_text, &capabilities) {
    //     Ok(query_result) => {
    //         let elapsed_ms = start.elapsed().as_millis() as u64;
    //         let envelope = SuccessEnvelope::new(
    //             config.engine.as_str(),
    //             "query",
    //             query_result,
    //             Metadata::with_rows(elapsed_ms, query_result.rows.len()),
    //         );
    //         output_success(&envelope);
    //         Ok(())
    //     }
    //     Err(e) => {
    //         let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "query", &e);
    //         output_error(&envelope);
    //         Err(1)
    //     }
    // }
}

async fn handle_mcp() -> std::result::Result<(), i32> {
    // Phase 7: Implement MCP server
    let error_envelope = ErrorEnvelope::new(
        "",
        "mcp",
        plenum::ErrorInfo::new(
            "NOT_IMPLEMENTED",
            "mcp server not yet implemented - Phase 7",
        ),
    );
    output_error(&error_envelope);
    Err(1)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Output a success envelope as JSON to stdout
fn output_success<T: serde::Serialize>(envelope: &SuccessEnvelope<T>) {
    match serde_json::to_string(envelope) {
        Ok(json) => println!("{}", json),
        Err(e) => {
            eprintln!("FATAL: Failed to serialize success envelope: {}", e);
            std::process::exit(2);
        }
    }
}

/// Output an error envelope as JSON to stdout
fn output_error(envelope: &ErrorEnvelope) {
    match serde_json::to_string(envelope) {
        Ok(json) => println!("{}", json),
        Err(e) => {
            eprintln!("FATAL: Failed to serialize error envelope: {}", e);
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
/// Precedence: Named connection → Default connection → CLI arguments only
fn build_connection_config(
    name: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
) -> Result<ConnectionConfig> {
    // Try to resolve from config if name is provided or if no explicit args
    let has_explicit_args = engine.is_some()
        || host.is_some()
        || port.is_some()
        || user.is_some()
        || password.is_some()
        || database.is_some()
        || file.is_some();

    let mut config = if name.is_some() || !has_explicit_args {
        // Try to load from config
        match plenum::resolve_connection(name.as_deref()) {
            Ok(cfg) => Some(cfg),
            Err(_) if has_explicit_args => None, // Ignore error if explicit args provided
            Err(e) => return Err(e),              // Propagate error if no fallback
        }
    } else {
        None
    };

    // Apply CLI overrides
    if let Some(ref mut cfg) = config {
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
        return Ok(cfg.clone());
    }

    // No config found, build from CLI arguments only
    let engine_type = engine
        .ok_or_else(|| PlenumError::invalid_input("--engine is required when not using a named connection"))?;
    let engine = parse_engine(&engine_type)?;

    match engine {
        DatabaseType::Postgres | DatabaseType::MySQL => {
            let host = host.ok_or_else(|| PlenumError::invalid_input("--host is required for postgres/mysql"))?;
            let port = port.ok_or_else(|| PlenumError::invalid_input("--port is required for postgres/mysql"))?;
            let user = user.ok_or_else(|| PlenumError::invalid_input("--user is required for postgres/mysql"))?;
            let password = password.ok_or_else(|| {
                PlenumError::invalid_input("--password is required for postgres/mysql")
            })?;
            let database = database.ok_or_else(|| {
                PlenumError::invalid_input("--database is required for postgres/mysql")
            })?;

            if engine == DatabaseType::Postgres {
                Ok(ConnectionConfig::postgres(host, port, user, password, database))
            } else {
                Ok(ConnectionConfig::mysql(host, port, user, password, database))
            }
        }
        DatabaseType::SQLite => {
            let file = file.ok_or_else(|| PlenumError::invalid_input("--file is required for sqlite"))?;
            Ok(ConnectionConfig::sqlite(file))
        }
    }
}

/// Parse engine string to DatabaseType
fn parse_engine(engine: &str) -> Result<DatabaseType> {
    match engine {
        "postgres" => Ok(DatabaseType::Postgres),
        "mysql" => Ok(DatabaseType::MySQL),
        "sqlite" => Ok(DatabaseType::SQLite),
        _ => Err(PlenumError::invalid_input(format!(
            "Invalid engine '{}'. Must be postgres, mysql, or sqlite",
            engine
        ))),
    }
}

/// Build capabilities from CLI flags
fn build_capabilities(
    allow_write: bool,
    allow_ddl: bool,
    max_rows: Option<usize>,
    timeout_ms: Option<u64>,
) -> Capabilities {
    Capabilities {
        allow_write,
        allow_ddl,
        max_rows,
        timeout_ms,
    }
}
