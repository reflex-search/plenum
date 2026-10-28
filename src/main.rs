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

use plenum::engine::{SslMode, TlsConfig};
use plenum::{
    parse_dsn, redact_dsn, Capabilities, ConfigLocation, ConnectionConfig, DatabaseEngine,
    DatabaseType, ErrorEnvelope, ExplainFormat, KeychainEntry, Metadata, PlenumError, Result,
    SuccessEnvelope,
};

// Import database engines
#[cfg(feature = "duckdb")]
use plenum::engine::duckdb::DuckDbEngine;
#[cfg(feature = "mysql")]
use plenum::engine::mysql::MySqlEngine;
#[cfg(feature = "postgres")]
use plenum::engine::postgres::PostgresEngine;
#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

/// Resolved arguments for `plenum connect`, produced by the interactive picker,
/// interactive wizard, or non-interactive flag path and consumed by the save step.
struct ConnectArgs {
    /// Connection name to save under.
    name: String,
    /// Explicit project path, or `None` to default to the current directory.
    project_path: Option<String>,
    /// Resolved connection configuration.
    config: ConnectionConfig,
    /// Indirect credential source: environment variable name.
    password_env: Option<String>,
    /// Indirect credential source: shell command producing the password.
    password_command: Option<String>,
    /// Indirect credential source: OS keychain entry.
    keychain_entry: Option<KeychainEntry>,
    /// Where to persist the connection (local or global).
    location: ConfigLocation,
}

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
        /// List saved connections for the project as JSON (no secrets emitted)
        #[arg(long, conflicts_with_all = ["name", "engine", "host", "port", "user", "password", "password_env", "password_command", "keychain_service", "keychain_account", "database", "file", "save", "test"])]
        list: bool,

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

        /// Shell command whose stdout is used as the password (run via `sh -c` at connection time)
        #[arg(long)]
        password_command: Option<String>,

        /// OS keychain service name (use with --keychain-account)
        #[arg(long, requires = "keychain_account")]
        keychain_service: Option<String>,

        /// OS keychain account name (use with --keychain-service)
        #[arg(long, requires = "keychain_service")]
        keychain_account: Option<String>,

        /// Database name (postgres/mysql)
        #[arg(long)]
        database: Option<String>,

        /// `SQLite` file path
        #[arg(long)]
        file: Option<PathBuf>,

        /// Save location (local or global)
        #[arg(long, value_parser = ["local", "global"], conflicts_with = "test")]
        save: Option<String>,

        /// TLS/SSL mode (postgres/mysql only): disable, require, verify-ca, or verify-full
        #[arg(long, value_parser = ["disable", "require", "verify-ca", "verify-full"])]
        ssl_mode: Option<String>,

        /// Path to PEM CA certificate for TLS verification (required for verify-ca / verify-full)
        #[arg(long)]
        ssl_ca: Option<PathBuf>,

        /// Path to PEM client certificate for mTLS (must be paired with --ssl-key)
        #[arg(long)]
        ssl_cert: Option<PathBuf>,

        /// Path to PEM client private key for mTLS (must be paired with --ssl-cert)
        #[arg(long)]
        ssl_key: Option<PathBuf>,

        /// Test connection liveness and return server metadata without saving config
        #[arg(long, conflicts_with = "save")]
        test: bool,
    },

    /// Introspect database schema
    Introspect {
        /// One-off connection DSN/URL (mutually exclusive with --name and explicit connection flags).
        /// Accepted schemes: postgres://, postgresql://, mysql://, sqlite:
        #[arg(long, conflicts_with_all = ["name", "engine", "host", "port", "user", "password", "database", "file"])]
        dsn: Option<String>,

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

        /// TLS/SSL mode (postgres/mysql only): disable, require, verify-ca, or verify-full
        #[arg(long, value_parser = ["disable", "require", "verify-ca", "verify-full"])]
        ssl_mode: Option<String>,

        /// Path to PEM CA certificate for TLS verification (required for verify-ca / verify-full)
        #[arg(long)]
        ssl_ca: Option<PathBuf>,

        /// Path to PEM client certificate for mTLS (must be paired with --ssl-key)
        #[arg(long)]
        ssl_cert: Option<PathBuf>,

        /// Path to PEM client private key for mTLS (must be paired with --ssl-cert)
        #[arg(long)]
        ssl_key: Option<PathBuf>,

        // ===== OPERATIONS (mutually exclusive) =====
        /// List all databases (requires wildcard database connection)
        #[arg(long, conflicts_with_all = ["list_schemas", "list_tables", "list_views", "list_indexes", "table", "view", "diff_against"])]
        list_databases: bool,

        /// List all schemas (`PostgreSQL` only)
        #[arg(long, conflicts_with_all = ["list_databases", "list_tables", "list_views", "list_indexes", "table", "view", "diff_against"])]
        list_schemas: bool,

        /// List all table names
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_views", "list_indexes", "table", "view", "diff_against"])]
        list_tables: bool,

        /// List all view names
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_tables", "list_indexes", "table", "view", "diff_against"])]
        list_views: bool,

        /// List all indexes (optionally filtered by table name)
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_tables", "list_views", "table", "view", "diff_against"])]
        list_indexes: Option<String>,

        /// Get full details for a specific table
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_tables", "list_views", "list_indexes", "view", "diff_against"])]
        table: Option<String>,

        /// Get details for a specific view
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_tables", "list_views", "list_indexes", "table", "diff_against"])]
        view: Option<String>,

        /// Compare the current connection against this named connection (structural schema diff).
        /// Mutually exclusive with all other operation flags.
        /// Returns a full structural diff: tables/views added, removed, and changed (columns,
        /// indexes, foreign keys, primary keys).
        #[arg(long, conflicts_with_all = ["list_databases", "list_schemas", "list_tables", "list_views", "list_indexes", "table", "view"])]
        diff_against: Option<String>,

        /// Project path for the --diff-against connection (defaults to the current project path).
        /// Use for cross-project comparison.
        #[arg(long, requires = "diff_against")]
        diff_against_project_path: Option<String>,

        // ===== MODIFIERS =====
        /// Target database (switch to different database before introspecting)
        #[arg(long)]
        target_database: Option<String>,

        /// Schema filter (PostgreSQL/MySQL only)
        #[arg(long)]
        schema: Option<String>,

        // ===== TABLE FIELD SELECTORS (for --table operation) =====
        /// Include columns in table details (default: true)
        #[arg(long, requires = "table")]
        columns: Option<bool>,

        /// Include primary key in table details (default: true)
        #[arg(long, requires = "table")]
        primary_key: Option<bool>,

        /// Include foreign keys in table details (default: true)
        #[arg(long, requires = "table")]
        foreign_keys: Option<bool>,

        /// Include indexes in table details (default: true)
        #[arg(long, requires = "table")]
        indexes: Option<bool>,
    },

    /// Execute constrained SQL queries
    Query {
        /// One-off connection DSN/URL (mutually exclusive with --name and explicit connection flags).
        /// Accepted schemes: postgres://, postgresql://, mysql://, sqlite:
        #[arg(long, conflicts_with_all = ["name", "engine", "host", "port", "user", "password", "database", "file"])]
        dsn: Option<String>,

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

        /// TLS/SSL mode (postgres/mysql only): disable, require, verify-ca, or verify-full
        #[arg(long, value_parser = ["disable", "require", "verify-ca", "verify-full"])]
        ssl_mode: Option<String>,

        /// Path to PEM CA certificate for TLS verification (required for verify-ca / verify-full)
        #[arg(long)]
        ssl_ca: Option<PathBuf>,

        /// Path to PEM client certificate for mTLS (must be paired with --ssl-key)
        #[arg(long)]
        ssl_cert: Option<PathBuf>,

        /// Path to PEM client private key for mTLS (must be paired with --ssl-cert)
        #[arg(long)]
        ssl_key: Option<PathBuf>,

        /// SQL query (mutually exclusive with --sql-file)
        #[arg(long, conflicts_with = "sql_file")]
        sql: Option<String>,

        /// SQL file path
        #[arg(long)]
        sql_file: Option<PathBuf>,

        /// Max rows to return per page
        #[arg(long)]
        max_rows: Option<usize>,

        /// Max serialized byte size of the rows array; truncates at row boundaries and signals `rows_truncated` + `truncated_by=bytes`
        #[arg(long)]
        max_bytes: Option<usize>,

        /// Number of rows to skip before collecting results (for pagination)
        #[arg(long)]
        offset: Option<usize>,

        /// Query timeout in milliseconds
        #[arg(long)]
        timeout_ms: Option<u64>,

        /// Bound query parameters, one per flag invocation.
        /// Parse rules: numeric literals bind as integers or floats, "true"/"false" as
        /// booleans, "null" as NULL, JSON strings as strings, everything else as text.
        /// Use $1/$2/… placeholders for `PostgreSQL`, ? for MySQL/SQLite.
        #[arg(long = "param", action = clap::ArgAction::Append)]
        param: Vec<String>,

        /// Return only timing information (excludes result data for benchmarking)
        #[arg(long)]
        time_only: bool,

        /// Validate SQL without executing: runs capability checks and returns a verdict, no DB call
        #[arg(long)]
        check_only: bool,

        /// EXPLAIN output format: "native" (default) returns raw engine rows unchanged;
        /// "structured" requires an EXPLAIN statement and returns data.plan — a normalized,
        /// engine-stable plan tree. Non-EXPLAIN queries with "structured" are rejected.
        #[arg(long)]
        explain_format: Option<String>,
    },

    /// Start MCP server (hidden from help, for AI agent integration)
    #[command(hide = true)]
    Mcp {
        /// Pin config resolution to this project path (overrides the launcher's cwd)
        #[arg(long)]
        project_path: Option<String>,

        /// Pin to a named connection within the resolved project
        #[arg(long)]
        name: Option<String>,

        /// Read the DSN from this named environment variable at connection time.
        /// Only the named variable is read — never an ambient fallback.
        #[arg(long, conflicts_with_all = ["project_path", "name"])]
        dsn_env: Option<String>,
    },
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
            list,
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            password_env,
            password_command,
            keychain_service,
            keychain_account,
            database,
            file,
            save,
            ssl_mode,
            ssl_ca,
            ssl_cert,
            ssl_key,
            test,
        }) => {
            let tls = build_tls_config(ssl_mode.as_deref(), ssl_ca, ssl_cert, ssl_key);
            let keychain_entry = match (keychain_service, keychain_account) {
                (Some(service), Some(account)) => Some(KeychainEntry { service, account }),
                _ => None,
            };
            if list {
                handle_connect_list(project_path).await
            } else if test {
                handle_connect_test(
                    name,
                    project_path,
                    engine,
                    host,
                    port,
                    user,
                    password,
                    password_env,
                    password_command,
                    keychain_entry,
                    database,
                    file,
                    tls,
                )
                .await
            } else {
                handle_connect(
                    name,
                    project_path,
                    engine,
                    host,
                    port,
                    user,
                    password,
                    password_env,
                    password_command,
                    keychain_entry,
                    database,
                    file,
                    save,
                    tls,
                )
                .await
            }
        }
        Some(Commands::Introspect {
            dsn,
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            ssl_mode,
            ssl_ca,
            ssl_cert,
            ssl_key,
            list_databases,
            list_schemas,
            list_tables,
            list_views,
            list_indexes,
            table,
            view,
            diff_against,
            diff_against_project_path,
            target_database,
            schema,
            columns,
            primary_key,
            foreign_keys,
            indexes,
        }) => {
            let tls = build_tls_config(ssl_mode.as_deref(), ssl_ca, ssl_cert, ssl_key);
            handle_introspect(
                dsn,
                name,
                project_path,
                engine,
                host,
                port,
                user,
                password,
                database,
                file,
                tls,
                list_databases,
                list_schemas,
                list_tables,
                list_views,
                list_indexes,
                table,
                view,
                diff_against,
                diff_against_project_path,
                target_database,
                schema,
                columns,
                primary_key,
                foreign_keys,
                indexes,
            )
            .await
        }
        Some(Commands::Query {
            dsn,
            name,
            project_path,
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            ssl_mode,
            ssl_ca,
            ssl_cert,
            ssl_key,
            sql,
            sql_file,
            max_rows,
            max_bytes,
            offset,
            timeout_ms,
            param,
            time_only,
            check_only,
            explain_format,
        }) => {
            let tls = build_tls_config(ssl_mode.as_deref(), ssl_ca, ssl_cert, ssl_key);
            handle_query(
                dsn,
                name,
                project_path,
                engine,
                host,
                port,
                user,
                password,
                database,
                file,
                tls,
                sql,
                sql_file,
                max_rows,
                max_bytes,
                offset,
                timeout_ms,
                param,
                time_only,
                check_only,
                explain_format,
            )
            .await
        }
        Some(Commands::Mcp { project_path, name, dsn_env }) => {
            handle_mcp(project_path, name, dsn_env).await
        }
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

/// Redacted connection entry for `connect --list` JSON output.
/// Never includes plaintext passwords. Shows metadata for indirect sources only:
/// - `password_env`: the env var name
/// - `password_command`: literal `true` (command itself not emitted to avoid leakage)
/// - `keychain_entry`: the service/account reference (no secret)
#[derive(serde::Serialize)]
struct ConnectionListEntry {
    name: String,
    engine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    password_command: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keychain_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keychain_account: Option<String>,
}

async fn handle_connect_list(project_path: Option<String>) -> std::result::Result<(), i32> {
    let start = Instant::now();

    let path = match project_path {
        Some(p) => p,
        None => match plenum::config::get_current_project_path() {
            Ok(p) => p,
            Err(e) => {
                let envelope = ErrorEnvelope::from_error("", "connect", &e);
                output_error(&envelope);
                return Err(1);
            }
        },
    };

    match plenum::list_connections_raw(&path) {
        Ok((connections, default)) => {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let entries: Vec<ConnectionListEntry> = connections
                .into_iter()
                .map(|(name, stored)| ConnectionListEntry {
                    name,
                    engine: stored.config.engine.as_str().to_string(),
                    host: stored.config.host,
                    port: stored.config.port,
                    user: stored.config.user,
                    database: stored.config.database,
                    file: stored.config.file,
                    password_env: stored.password_env,
                    // command text not emitted to avoid leaking secrets in list output
                    password_command: stored.password_command.map(|_| true),
                    keychain_service: stored.keychain_entry.as_ref().map(|e| e.service.clone()),
                    keychain_account: stored.keychain_entry.map(|e| e.account),
                    // inline password intentionally omitted — never emitted
                })
                .collect();

            let data = serde_json::json!({
                "connections": entries,
                "default": default,
            });

            let envelope = SuccessEnvelope::new("", "connect", data, Metadata::new(elapsed_ms));
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

async fn handle_connect(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    password_command: Option<String>,
    keychain_entry: Option<KeychainEntry>,
    database: Option<String>,
    file: Option<PathBuf>,
    save: Option<String>,
    tls: Option<TlsConfig>,
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
        || password_command.is_some()
        || keychain_entry.is_some()
        || database.is_some()
        || file.is_some()
        || save.is_some()
        || tls.is_some();

    let result: Result<ConnectArgs> = if has_args {
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
            password_command,
            keychain_entry,
            database,
            file,
            save,
            tls,
        )
        .await
    } else {
        // Interactive mode: show picker
        interactive_connect_picker().await
    };

    match result {
        Ok(ConnectArgs {
            name: conn_name,
            project_path: proj_path,
            config,
            password_env,
            password_command,
            keychain_entry,
            location,
        }) => {
            // Resolve the effective project path up front so the saved connection
            // and the emitted mcp_stanza reference the exact same path. When
            // proj_path is None, save_connection would fall back to the current
            // directory; resolve it here so both stay in lockstep.
            let effective_project_path = match proj_path {
                Some(p) => p,
                None => match plenum::config::get_current_project_path() {
                    Ok(p) => p,
                    Err(e) => {
                        let envelope = ErrorEnvelope::from_error("", "connect", &e);
                        output_error(&envelope);
                        return Err(1);
                    }
                },
            };

            // Save connection. Use the project-scoped writer so `--save local`
            // targets `<project_path>/.plenum/config.json` rather than the
            // current working directory. The local config format is
            // single-project and keyed by its own directory, so writing under
            // cwd while keying the entry under an explicit `--project-path`
            // would silently drop the connection.
            match plenum::save_connection_in_project(
                &effective_project_path,
                Some(conn_name.clone()),
                config.clone(),
                password_env,
                password_command,
                keychain_entry,
                location,
            ) {
                Ok(()) => {
                    // Build success response
                    let elapsed_ms = start.elapsed().as_millis() as u64;
                    let mcp_stanza = plenum::McpStanza::for_saved_connection(
                        &effective_project_path,
                        &conn_name,
                    );
                    let data = plenum::ConnectSaveResult {
                        message: format!("Connection '{conn_name}' saved successfully"),
                        connection_name: conn_name,
                        engine: config.engine.as_str().to_string(),
                        saved_to: match location {
                            ConfigLocation::Local => "local".to_string(),
                            ConfigLocation::Global => "global".to_string(),
                        },
                        mcp_stanza,
                    };

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

/// Test a connection: open, validate, return `ConnectionInfo`, then disconnect.
/// No config is saved. On failure, returns a `CONNECTION_FAILED` envelope with no credentials.
#[allow(clippy::too_many_arguments)]
async fn handle_connect_test(
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    mut password: Option<String>,
    password_env: Option<String>,
    password_command: Option<String>,
    keychain_entry: Option<KeychainEntry>,
    database: Option<String>,
    file: Option<PathBuf>,
    tls: Option<TlsConfig>,
) -> std::result::Result<(), i32> {
    let start = Instant::now();

    // Enforce: at most one indirect credential source in test mode
    let source_count =
        [password_env.is_some(), password_command.is_some(), keychain_entry.is_some()]
            .iter()
            .filter(|&&b| b)
            .count();
    if source_count > 1 {
        let envelope = ErrorEnvelope::new(
            "",
            "connect",
            plenum::ErrorInfo::new(
                "INVALID_INPUT",
                "Only one of --password-env, --password-command, or --keychain-service/--keychain-account may be used",
            ),
        );
        output_error(&envelope);
        return Err(1);
    }

    // Resolve indirect credential sources before building the config
    if let Some(env_var) = &password_env {
        match std::env::var(env_var) {
            Ok(val) if !val.is_empty() => password = Some(val),
            Ok(_) => {
                let envelope = ErrorEnvelope::new(
                    "",
                    "connect",
                    plenum::ErrorInfo::new(
                        "INVALID_INPUT",
                        format!("Environment variable {env_var} is set but empty"),
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }
            Err(_) => {
                let envelope = ErrorEnvelope::new(
                    "",
                    "connect",
                    plenum::ErrorInfo::new(
                        "INVALID_INPUT",
                        format!("Environment variable {env_var} is not set"),
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }
        }
    } else if let Some(cmd) = &password_command {
        match plenum::config::run_password_command_pub(cmd) {
            Ok(val) => password = Some(val),
            Err(e) => {
                let envelope = ErrorEnvelope::from_error("", "connect", &e);
                output_error(&envelope);
                return Err(1);
            }
        }
    } else if let Some(entry) = &keychain_entry {
        match plenum::config::lookup_keychain_password_pub(&entry.service, &entry.account) {
            Ok(val) => password = Some(val),
            Err(e) => {
                let envelope = ErrorEnvelope::from_error("", "connect", &e);
                output_error(&envelope);
                return Err(1);
            }
        }
    }

    // Resolve connection config from saved config or explicit CLI args
    let (config, _is_readonly) = match build_connection_config(
        name.as_deref(),
        project_path.as_deref(),
        engine,
        host,
        port,
        user,
        password,
        database,
        file,
        tls,
    ) {
        Ok(cfg) => cfg,
        Err(e) => {
            let envelope = ErrorEnvelope::from_error("", "connect", &e);
            output_error(&envelope);
            return Err(1);
        }
    };

    // Open, validate, and immediately close the connection
    let result = match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::validate_connection(&config).await,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => Err(PlenumError::invalid_input(
            "SQLite engine not enabled. Build with --features sqlite to enable SQLite support.",
        )),

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => PostgresEngine::validate_connection(&config).await,
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => Err(PlenumError::invalid_input(
            "PostgreSQL engine not enabled. Build with --features postgres to enable PostgreSQL support.",
        )),

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::validate_connection(&config).await,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => Err(PlenumError::invalid_input(
            "MySQL engine not enabled. Build with --features mysql to enable MySQL support.",
        )),

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::validate_connection(&config).await,
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => Err(PlenumError::invalid_input(
            "DuckDB engine not enabled. Build with --features duckdb to enable DuckDB support.",
        )),
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(connection_info) => {
            let envelope = SuccessEnvelope::new(
                config.engine.as_str(),
                "connect",
                connection_info,
                Metadata::new(elapsed_ms),
            );
            output_success(&envelope);
            Ok(())
        }
        Err(e) => {
            let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "connect", &e);
            output_error(&envelope);
            Err(1)
        }
    }
}

/// Interactive connection picker (when no args provided)
async fn interactive_connect_picker() -> Result<ConnectArgs> {
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
                    DatabaseType::SQLite | DatabaseType::DuckDB => {
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

        // Re-saving an existing connection preserves its stored credential
        // reference as-is; the picker never re-prompts for or scans credentials.
        Ok(ConnectArgs {
            name: name.clone(),
            project_path: None,
            config: config.clone(),
            password_env: None,
            password_command: None,
            keychain_entry: None,
            location,
        })
    }
}

/// A credential source chosen in the interactive wizard.
///
/// Each variant carries only what the user explicitly typed — an env var name,
/// a command string, or a keychain service/account. The wizard never scans the
/// environment, keychain, or filesystem to populate these; there is no ambient
/// pickup or discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WizardCredential {
    /// Plaintext password typed directly.
    Plaintext(String),
    /// Name of an environment variable to read at connection time.
    Env(String),
    /// Shell command whose stdout yields the password at connection time.
    Command(String),
    /// OS keychain service + account reference.
    Keychain { service: String, account: String },
}

/// Split a wizard credential choice into the persisted fields
/// `(password, password_env, password_command, keychain_entry)`.
///
/// Only the user-typed reference is propagated. For indirect sources the inline
/// `password` is `None` (resolved at use time); nothing here reads the
/// environment, keychain, or filesystem.
fn wizard_credential_fields(
    cred: WizardCredential,
) -> (Option<String>, Option<String>, Option<String>, Option<KeychainEntry>) {
    match cred {
        WizardCredential::Plaintext(p) => (Some(p), None, None, None),
        WizardCredential::Env(var) => (None, Some(var), None, None),
        WizardCredential::Command(cmd) => (None, None, Some(cmd), None),
        WizardCredential::Keychain { service, account } => {
            (None, None, None, Some(KeychainEntry { service, account }))
        }
    }
}

/// Validate that an indirect credential source resolves now, so the wizard fails
/// fast instead of persisting a broken reference.
///
/// Error messages name the exact field the user must fix and never suggest a
/// discovered alternative (no "did you mean" from a scan of the environment or
/// keychain).
fn validate_wizard_credential(cred: &WizardCredential) -> Result<()> {
    match cred {
        WizardCredential::Plaintext(_) => Ok(()),
        WizardCredential::Env(var) => match std::env::var(var) {
            Ok(v) if v.is_empty() => Err(PlenumError::invalid_input(format!(
                "Environment variable {var} is set but empty — set {var} or choose a different credential source"
            ))),
            Ok(_) => Ok(()),
            Err(_) => Err(PlenumError::invalid_input(format!(
                "Environment variable {var} is not set — set {var} or choose a different credential source"
            ))),
        },
        WizardCredential::Command(cmd) => plenum::config::run_password_command_pub(cmd)
            .map(|_| ())
            .map_err(|e| {
                PlenumError::invalid_input(format!(
                    "password command failed validation: {} — fix the command or choose a different credential source",
                    e.message()
                ))
            }),
        WizardCredential::Keychain { service, account } => {
            plenum::config::lookup_keychain_password_pub(service, account).map(|_| ()).map_err(|e| {
                PlenumError::invalid_input(format!(
                    "keychain lookup for service '{service}' / account '{account}' failed: {} — fix the service/account or choose a different credential source",
                    e.message()
                ))
            })
        }
    }
}

/// Prompt for how the password should be provided. The user always *types* the
/// source reference; the wizard never lists or suggests discovered credentials.
fn prompt_wizard_credential() -> Result<WizardCredential> {
    use dialoguer::{Input, Password, Select};

    let choices = vec![
        "Enter password directly",
        "Environment variable (you type the name)",
        "Shell command (you type the command)",
        "OS keychain (you type service + account)",
    ];
    let idx = Select::new()
        .with_prompt("How should the password be provided?")
        .items(&choices)
        .default(0)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {e}")))?;

    match idx {
        0 => {
            let pw: String = Password::new()
                .with_prompt("Password")
                .interact()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;
            Ok(WizardCredential::Plaintext(pw))
        }
        1 => {
            let var: String = Input::new()
                .with_prompt("Environment variable name (e.g. PGPASSWORD)")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;
            Ok(WizardCredential::Env(var))
        }
        2 => {
            let cmd: String = Input::new()
                .with_prompt("Shell command to produce the password")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;
            Ok(WizardCredential::Command(cmd))
        }
        _ => {
            let service: String = Input::new()
                .with_prompt("Keychain service name")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;
            let account: String = Input::new()
                .with_prompt("Keychain account name")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;
            Ok(WizardCredential::Keychain { service, account })
        }
    }
}

/// Interactive connection wizard
async fn interactive_connect_wizard() -> Result<ConnectArgs> {
    use dialoguer::{Input, Select};

    eprintln!("\n=== Create New Database Connection ===\n");

    // Prompt for engine
    let engine_choices = vec!["postgres", "mysql", "sqlite", "duckdb"];
    let engine_idx = Select::new()
        .with_prompt("Select database engine")
        .items(&engine_choices)
        .interact()
        .map_err(|e| PlenumError::invalid_input(format!("Selection failed: {e}")))?;
    let engine = parse_engine(engine_choices[engine_idx])?;

    // Build config and resolve any indirect credential reference based on engine type.
    let (config, password_env, password_command, keychain_entry) = match engine {
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

            // Offer plaintext or an indirect credential source, validating that
            // the chosen source resolves before we persist the reference.
            let cred = prompt_wizard_credential()?;
            validate_wizard_credential(&cred)?;

            let database: String = Input::new()
                .with_prompt("Database name")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let (password, p_env, p_cmd, keychain) = wizard_credential_fields(cred);

            let config = ConnectionConfig {
                engine,
                host: Some(host),
                port: Some(port),
                user: Some(user),
                // None for indirect sources — resolved at use time, never stored inline.
                password,
                database: Some(database),
                file: None,
                tls: None,
            };
            (config, p_env, p_cmd, keychain)
        }
        DatabaseType::SQLite | DatabaseType::DuckDB => {
            let file: String = Input::new()
                .with_prompt("Database file path")
                .interact_text()
                .map_err(|e| PlenumError::invalid_input(format!("Input failed: {e}")))?;

            let config = if engine == DatabaseType::DuckDB {
                ConnectionConfig::duckdb(PathBuf::from(file))
            } else {
                ConnectionConfig::sqlite(PathBuf::from(file))
            };
            // File-based engines have no authentication credential.
            (config, None, None, None)
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
    Ok(ConnectArgs {
        name,
        project_path: None,
        config,
        password_env,
        password_command,
        keychain_entry,
        location,
    })
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
    password_env: Option<String>,
    password_command: Option<String>,
    keychain_entry: Option<KeychainEntry>,
    database: Option<String>,
    file: Option<PathBuf>,
    save: Option<String>,
    tls: Option<TlsConfig>,
) -> Result<ConnectArgs> {
    // Validate required arguments
    let engine_str = engine.ok_or_else(|| {
        PlenumError::invalid_input("--engine is required for non-interactive mode")
    })?;
    let engine_type = parse_engine(&engine_str)?;

    // Enforce: at most one indirect credential source
    let indirect_count =
        [password_env.is_some(), password_command.is_some(), keychain_entry.is_some()]
            .iter()
            .filter(|&&b| b)
            .count();
    if indirect_count > 1 {
        return Err(PlenumError::invalid_input(
            "Only one of --password-env, --password-command, or --keychain-service/--keychain-account may be used",
        ));
    }

    // --password is mutually exclusive with indirect sources
    if password.is_some() && indirect_count > 0 {
        return Err(PlenumError::invalid_input(
            "--password is mutually exclusive with --password-env, --password-command, and --keychain-service",
        ));
    }

    // If --password-env is provided, validate that the env var resolves at connect time.
    // The literal password value is NOT persisted; only the env var name is stored.
    if let Some(env_var) = password_env.as_deref() {
        match std::env::var(env_var) {
            Ok(value) if value.is_empty() => {
                return Err(PlenumError::invalid_input(format!(
                    "Environment variable {env_var} is set but empty"
                )));
            }
            Ok(_) => {}
            Err(_) => {
                return Err(PlenumError::invalid_input(format!(
                    "Environment variable {env_var} is not set"
                )));
            }
        }
    }

    // If --password-command is provided, do a test run to validate it resolves now.
    if let Some(cmd) = password_command.as_deref() {
        plenum::config::run_password_command_pub(cmd).map_err(|e| {
            PlenumError::invalid_input(format!(
                "--password-command failed validation: {}",
                e.message()
            ))
        })?;
    }

    // If keychain entry provided, validate it resolves now.
    if let Some(ref entry) = keychain_entry {
        plenum::config::lookup_keychain_password_pub(&entry.service, &entry.account).map_err(
            |e| {
                PlenumError::invalid_input(format!(
                    "--keychain-service/--keychain-account failed validation: {}",
                    e.message()
                ))
            },
        )?;
    }

    // Indirect sources are only meaningful for engines that use passwords.
    if matches!(engine_type, DatabaseType::SQLite | DatabaseType::DuckDB)
        && (password_env.is_some() || password_command.is_some() || keychain_entry.is_some())
    {
        return Err(PlenumError::invalid_input(
            "--password-env, --password-command, and --keychain-service are not applicable to file-based engines (no authentication)",
        ));
    }

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
            let database = database.ok_or_else(|| {
                PlenumError::invalid_input("--database is required for postgres/mysql")
            })?;

            // Password is stored directly only when --password is given.
            // Indirect sources (env/command/keychain) are NOT stored inline — resolved at use time.
            let stored_password = match (&password, indirect_count) {
                (Some(p), 0) => Some(p.clone()),
                (None, 1) => None, // indirect source will be resolved at use time
                (None, 0) => {
                    return Err(PlenumError::invalid_input(
                        "--password, --password-env, --password-command, or --keychain-service is required for postgres/mysql",
                    ));
                }
                _ => unreachable!("checked above"),
            };

            ConnectionConfig {
                engine: engine_type,
                host: Some(host),
                port: Some(port),
                user: Some(user),
                password: stored_password,
                database: Some(database),
                file: None,
                tls,
            }
        }
        DatabaseType::SQLite | DatabaseType::DuckDB => {
            let file = file.ok_or_else(|| {
                PlenumError::invalid_input(format!("--file is required for {engine_str}"))
            })?;
            let mut cfg = if engine_type == DatabaseType::DuckDB {
                ConnectionConfig::duckdb(file)
            } else {
                ConnectionConfig::sqlite(file)
            };
            cfg.tls = tls;
            cfg
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

    Ok(ConnectArgs {
        name: conn_name,
        project_path,
        config,
        password_env,
        password_command,
        keychain_entry,
        location,
    })
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

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
async fn handle_introspect(
    dsn: Option<String>,
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
    tls: Option<TlsConfig>,
    list_databases: bool,
    list_schemas: bool,
    list_tables: bool,
    list_views: bool,
    list_indexes: Option<String>,
    table: Option<String>,
    view: Option<String>,
    diff_against: Option<String>,
    diff_against_project_path: Option<String>,
    target_database: Option<String>,
    schema: Option<String>,
    columns: Option<bool>,
    primary_key: Option<bool>,
    foreign_keys: Option<bool>,
    indexes: Option<bool>,
) -> std::result::Result<(), i32> {
    use plenum::engine::{IntrospectOperation, TableFields};

    // Start timing
    let start = Instant::now();

    // Resolve base connection config — DSN path bypasses saved config entirely
    let (config, _is_readonly) = if let Some(ref dsn_str) = dsn {
        match parse_dsn(dsn_str) {
            Ok(cfg) => (cfg, false),
            Err(e) => {
                let envelope = ErrorEnvelope::new(
                    "",
                    "introspect",
                    plenum::ErrorInfo::new(
                        e.error_code(),
                        format!("{} (DSN: {})", e.message(), redact_dsn(dsn_str)),
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }
        }
    } else {
        match build_connection_config(
            name.as_deref(),
            project_path.as_deref(),
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            tls,
        ) {
            Ok(cfg_tuple) => cfg_tuple,
            Err(e) => {
                let envelope = ErrorEnvelope::from_error("", "introspect", &e);
                output_error(&envelope);
                return Err(1);
            }
        }
    };

    // ── diff-against path ─────────────────────────────────────────────────────
    if let Some(target_name) = diff_against {
        // Resolve the second (target) connection via the same config precedence rules.
        // --diff-against-project-path overrides; otherwise fall back to the primary project path
        // (or current directory when neither is specified).
        let target_proj = diff_against_project_path.as_deref().or(project_path.as_deref());
        let target_config = match resolve_connection_cli(target_proj, Some(&target_name)) {
            Ok((cfg, _)) => cfg,
            Err(e) => {
                let envelope = ErrorEnvelope::from_error(config.engine.as_str(), "introspect", &e);
                output_error(&envelope);
                return Err(1);
            }
        };

        let diff_result = plenum::diff::compute_schema_diff(
            &config,
            &target_config,
            target_database.as_deref(),
            schema.as_deref(),
        )
        .await;

        let elapsed_ms = start.elapsed().as_millis() as u64;
        match diff_result {
            Ok(diff) => {
                let envelope = SuccessEnvelope::new(
                    config.engine.as_str(),
                    "introspect",
                    serde_json::json!({ "diff": diff }),
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
    } else {
        // ── standard introspect path ──────────────────────────────────────────
        let operation = {
            let ops = [
                list_databases,
                list_schemas,
                list_tables,
                list_views,
                list_indexes.is_some(),
                table.is_some(),
                view.is_some(),
            ];
            let op_count = ops.iter().filter(|&&x| x).count();

            if op_count == 0 {
                let envelope = ErrorEnvelope::new(
                    config.engine.as_str(),
                    "introspect",
                    plenum::ErrorInfo::new(
                        "INVALID_INPUT",
                        "No introspect operation specified. Must provide exactly one of: \
                         --list-databases, --list-schemas, --list-tables, --list-views, \
                         --list-indexes, --table, --view, or --diff-against. \
                         Use --help for more information.",
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }

            if op_count > 1 {
                let envelope = ErrorEnvelope::new(
                    config.engine.as_str(),
                    "introspect",
                    plenum::ErrorInfo::new(
                        "INVALID_INPUT",
                        "Multiple introspect operations specified. Only one operation allowed per invocation.",
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }

            if list_databases {
                IntrospectOperation::ListDatabases
            } else if list_schemas {
                IntrospectOperation::ListSchemas
            } else if list_tables {
                IntrospectOperation::ListTables
            } else if list_views {
                IntrospectOperation::ListViews
            } else if let Some(table_filter) = list_indexes {
                let filter = if table_filter.is_empty() { None } else { Some(table_filter) };
                IntrospectOperation::ListIndexes { table: filter }
            } else if let Some(table_name) = table {
                let fields = TableFields {
                    columns: columns.unwrap_or(true),
                    primary_key: primary_key.unwrap_or(true),
                    foreign_keys: foreign_keys.unwrap_or(true),
                    indexes: indexes.unwrap_or(true),
                };
                IntrospectOperation::TableDetails { name: table_name, fields }
            } else if let Some(view_name) = view {
                IntrospectOperation::ViewDetails { name: view_name }
            } else {
                unreachable!("Operation validation above ensures we have exactly one operation")
            }
        };

        let introspect_result = match config.engine {
            #[cfg(feature = "sqlite")]
            DatabaseType::SQLite => {
                SqliteEngine::introspect(
                    &config,
                    &operation,
                    target_database.as_deref(),
                    schema.as_deref(),
                )
                .await
            }
            #[cfg(not(feature = "sqlite"))]
            DatabaseType::SQLite => Err(PlenumError::invalid_input(
                "SQLite engine not enabled. Build with --features sqlite to enable SQLite support.",
            )),

            #[cfg(feature = "postgres")]
            DatabaseType::Postgres => {
                PostgresEngine::introspect(
                    &config,
                    &operation,
                    target_database.as_deref(),
                    schema.as_deref(),
                )
                .await
            }
            #[cfg(not(feature = "postgres"))]
            DatabaseType::Postgres => Err(PlenumError::invalid_input(
                "PostgreSQL engine not enabled. Build with --features postgres to enable PostgreSQL support.",
            )),

            #[cfg(feature = "mysql")]
            DatabaseType::MySQL => {
                MySqlEngine::introspect(
                    &config,
                    &operation,
                    target_database.as_deref(),
                    schema.as_deref(),
                )
                .await
            }
            #[cfg(not(feature = "mysql"))]
            DatabaseType::MySQL => Err(PlenumError::invalid_input(
                "MySQL engine not enabled. Build with --features mysql to enable MySQL support.",
            )),

            #[cfg(feature = "duckdb")]
            DatabaseType::DuckDB => {
                DuckDbEngine::introspect(
                    &config,
                    &operation,
                    target_database.as_deref(),
                    schema.as_deref(),
                )
                .await
            }
            #[cfg(not(feature = "duckdb"))]
            DatabaseType::DuckDB => Err(PlenumError::invalid_input(
                "DuckDB engine not enabled. Build with --features duckdb to enable DuckDB support.",
            )),
        };

        match introspect_result {
            Ok(introspect_result) => {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                let envelope = SuccessEnvelope::new(
                    config.engine.as_str(),
                    "introspect",
                    introspect_result,
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
}

async fn handle_query(
    dsn: Option<String>,
    name: Option<String>,
    project_path: Option<String>,
    engine: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    file: Option<PathBuf>,
    tls: Option<TlsConfig>,
    sql: Option<String>,
    sql_file: Option<PathBuf>,
    max_rows: Option<usize>,
    max_bytes: Option<usize>,
    offset: Option<usize>,
    timeout_ms: Option<u64>,
    raw_params: Vec<String>,
    time_only: bool,
    check_only: bool,
    explain_format: Option<String>,
) -> std::result::Result<(), i32> {
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

    // Resolve connection config — DSN path bypasses saved config entirely
    let (config, _is_readonly) = if let Some(ref dsn_str) = dsn {
        match parse_dsn(dsn_str) {
            Ok(cfg) => (cfg, false),
            Err(e) => {
                let envelope = ErrorEnvelope::new(
                    "",
                    "query",
                    plenum::ErrorInfo::new(
                        e.error_code(),
                        format!("{} (DSN: {})", e.message(), redact_dsn(dsn_str)),
                    ),
                );
                output_error(&envelope);
                return Err(1);
            }
        }
    } else {
        match build_connection_config(
            name.as_deref(),
            project_path.as_deref(),
            engine,
            host,
            port,
            user,
            password,
            database,
            file,
            tls,
        ) {
            Ok(cfg_tuple) => cfg_tuple,
            Err(e) => {
                let envelope = ErrorEnvelope::from_error("", "query", &e);
                output_error(&envelope);
                return Err(1);
            }
        }
    };

    // Build capabilities (read-only only)
    let explain_format_parsed = explain_format.as_deref().map(|s| match s {
        "structured" | "Structured" => ExplainFormat::Structured,
        _ => ExplainFormat::Native,
    });
    let capabilities = Capabilities {
        max_rows,
        max_bytes: None,
        timeout_ms,
        offset,
        explain_format: explain_format_parsed,
    };
    // max_bytes is applied post-engine as a post-processing step (see apply_byte_budget call below)

    // Parse --param strings into typed JSON values.
    // Rule: try serde_json parsing first so that 5 → Number, true → Bool, null → Null,
    // "text" → String. If parsing fails, treat as a plain text string.
    let params: Vec<serde_json::Value> = raw_params
        .iter()
        .map(|s| serde_json::from_str(s).unwrap_or_else(|_| serde_json::Value::String(s.clone())))
        .collect();

    // Validate query is read-only
    match plenum::validate_query(&sql_text, &capabilities, config.engine) {
        Ok(()) => {
            if check_only {
                let elapsed_ms = start.elapsed().as_millis() as u64;
                let data = serde_json::json!({ "would_execute": true, "category": "read" });
                let envelope = SuccessEnvelope::new(
                    config.engine.as_str(),
                    "query",
                    data,
                    Metadata::new(elapsed_ms),
                );
                output_success(&envelope);
                return Ok(());
            }
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
        DatabaseType::SQLite => {
            SqliteEngine::execute(&config, &sql_text, &params, &capabilities).await
        }
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => {
            Err(PlenumError::invalid_input(
                "SQLite engine not enabled. Build with --features sqlite to enable SQLite support."
            ))
        }

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => {
            PostgresEngine::execute(&config, &sql_text, &params, &capabilities).await
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => {
            Err(PlenumError::invalid_input(
                "PostgreSQL engine not enabled. Build with --features postgres to enable PostgreSQL support."
            ))
        }

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => {
            MySqlEngine::execute(&config, &sql_text, &params, &capabilities).await
        }
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => {
            Err(PlenumError::invalid_input(
                "MySQL engine not enabled. Build with --features mysql to enable MySQL support."
            ))
        }

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => {
            DuckDbEngine::execute(&config, &sql_text, &params, &capabilities).await
        }
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => {
            Err(PlenumError::invalid_input(
                "DuckDB engine not enabled. Build with --features duckdb to enable DuckDB support."
            ))
        }
    };

    match execute_result {
        Ok(mut query_result) => {
            // Apply byte budget post-engine (row-boundary truncation)
            if let Some(max_b) = max_bytes {
                plenum::engine::apply_byte_budget(&mut query_result, max_b);
            }

            let execution_ms = query_result.execution_ms;
            let row_count = query_result.rows.len();
            let rows_truncated = query_result.rows_truncated;
            let truncated_by = query_result.truncated_by.clone();
            let effective_offset = offset.unwrap_or(0);
            let query_meta = Metadata::with_query(
                execution_ms,
                row_count,
                rows_truncated,
                effective_offset,
                truncated_by,
            );

            if time_only {
                // Return only timing information (for benchmarking)
                let time_only_result =
                    plenum::TimeOnlyResult { execution_ms, rows_matched: row_count };
                let envelope = SuccessEnvelope::new(
                    config.engine.as_str(),
                    "query",
                    time_only_result,
                    query_meta,
                );
                output_success(&envelope);
            } else {
                // Return full query results
                let envelope =
                    SuccessEnvelope::new(config.engine.as_str(), "query", query_result, query_meta);
                output_success(&envelope);
            }
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
async fn handle_mcp(
    project_path: Option<String>,
    name: Option<String>,
    dsn_env: Option<String>,
) -> std::result::Result<(), i32> {
    // Phase 7: MCP server using manual JSON-RPC 2.0 implementation
    // Follows the proven pattern from reflex-search (no unstable rmcp dependency)
    //
    // Bind the connection source at server registration so resolution is
    // decoupled from the launcher's cwd (REF-297).
    let binding = match plenum::mcp::McpBinding::new(project_path, name, dsn_env) {
        Ok(binding) => binding,
        Err(e) => {
            eprintln!("MCP server error: {e}");
            return Err(1);
        }
    };

    match plenum::mcp::serve(binding).await {
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

/// Build an `Option<TlsConfig>` from raw CLI SSL args.
/// Returns `None` when no SSL args are provided (default = no TLS).
fn build_tls_config(
    ssl_mode: Option<&str>,
    ssl_ca: Option<PathBuf>,
    ssl_cert: Option<PathBuf>,
    ssl_key: Option<PathBuf>,
) -> Option<TlsConfig> {
    if ssl_mode.is_none() && ssl_ca.is_none() && ssl_cert.is_none() && ssl_key.is_none() {
        return None;
    }
    let mode = match ssl_mode.unwrap_or("disable") {
        "require" => SslMode::Require,
        "verify-ca" => SslMode::VerifyCa,
        "verify-full" => SslMode::VerifyFull,
        _ => SslMode::Disable,
    };
    Some(TlsConfig { sslmode: mode, ca_cert: ssl_ca, client_cert: ssl_cert, client_key: ssl_key })
}

/// Build connection config from CLI arguments
///
/// This helper resolves a connection from config or builds one from CLI arguments.
/// Precedence: Named connection at project path → CLI arguments only
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
/// Resolve a saved connection honoring an explicit `--project-path`.
///
/// When a project path is given, the connection is resolved from that project's
/// own `.plenum/config.json` (merged with the global registry), matching the
/// documented `--project-path /other/project` semantics and the way
/// `plenum mcp --project-path` binds. When no project path is given, resolution
/// falls back to the current working directory's local config.
fn resolve_connection_cli(
    project_path: Option<&str>,
    name: Option<&str>,
) -> Result<(ConnectionConfig, bool)> {
    match project_path {
        Some(path) => plenum::resolve_connection_in_project(path, name),
        None => plenum::resolve_connection(None, name),
    }
}

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
    tls: Option<TlsConfig>,
) -> Result<(ConnectionConfig, bool)> {
    let has_explicit_args = engine.is_some()
        || host.is_some()
        || port.is_some()
        || user.is_some()
        || password.is_some();

    // Try to resolve from config if name or project_path is provided, or if no explicit args
    let should_try_resolve = name.is_some() || project_path.is_some() || !has_explicit_args;

    let mut resolved_connection: Option<(ConnectionConfig, bool)> = if should_try_resolve {
        // Try to load connection from config
        match resolve_connection_cli(project_path, name) {
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
        // TLS override: explicit CLI flags always win over stored config
        if tls.is_some() {
            cfg.tls = tls;
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

    let mut config = match engine {
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
        DatabaseType::DuckDB => {
            let file =
                file.ok_or_else(|| PlenumError::invalid_input("--file is required for duckdb"))?;
            ConnectionConfig::duckdb(file)
        }
    };
    config.tls = tls;

    Ok((config, false)) // CLI-only connections are never readonly
}

/// Parse engine string to `DatabaseType`
fn parse_engine(engine: &str) -> Result<DatabaseType> {
    match engine {
        "postgres" => Ok(DatabaseType::Postgres),
        "mysql" => Ok(DatabaseType::MySQL),
        "sqlite" => Ok(DatabaseType::SQLite),
        "duckdb" => Ok(DatabaseType::DuckDB),
        _ => Err(PlenumError::invalid_input(format!(
            "Invalid engine '{engine}'. Must be postgres, mysql, sqlite, or duckdb"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_env_credential_stores_only_typed_name() {
        // Capability test: the typed env var name is stored verbatim; no inline
        // password value is produced, and nothing is read from the environment.
        let (password, p_env, p_cmd, keychain) =
            wizard_credential_fields(WizardCredential::Env("PGPASSWORD".to_string()));
        assert_eq!(p_env.as_deref(), Some("PGPASSWORD"));
        assert!(password.is_none(), "indirect source must not store an inline password");
        assert!(p_cmd.is_none());
        assert!(keychain.is_none());
    }

    #[test]
    fn wizard_command_credential_stores_only_typed_command() {
        let (password, p_env, p_cmd, keychain) =
            wizard_credential_fields(WizardCredential::Command("op read secret".to_string()));
        assert_eq!(p_cmd.as_deref(), Some("op read secret"));
        assert!(password.is_none());
        assert!(p_env.is_none());
        assert!(keychain.is_none());
    }

    #[test]
    fn wizard_keychain_credential_stores_only_typed_service_account() {
        let (password, p_env, p_cmd, keychain) =
            wizard_credential_fields(WizardCredential::Keychain {
                service: "plenum-db".to_string(),
                account: "app".to_string(),
            });
        let entry = keychain.expect("keychain entry stored");
        assert_eq!(entry.service, "plenum-db");
        assert_eq!(entry.account, "app");
        assert!(password.is_none());
        assert!(p_env.is_none());
        assert!(p_cmd.is_none());
    }

    #[test]
    fn wizard_plaintext_credential_stores_inline_password() {
        let (password, p_env, p_cmd, keychain) =
            wizard_credential_fields(WizardCredential::Plaintext("hunter2".to_string()));
        assert_eq!(password.as_deref(), Some("hunter2"));
        assert!(p_env.is_none());
        assert!(p_cmd.is_none());
        assert!(keychain.is_none());
    }

    #[test]
    fn wizard_validate_env_missing_names_the_field_to_fix() {
        // A missing env var must fail fast and name the exact variable — never
        // suggest a discovered alternative.
        let var = "PLENUM_WIZARD_TEST_MISSING_XYZ";
        // Ensure it is unset for the duration of this assertion.
        assert!(std::env::var(var).is_err(), "test precondition: var must be unset");
        let err = validate_wizard_credential(&WizardCredential::Env(var.to_string()))
            .expect_err("missing env var must fail validation");
        let msg = err.message();
        assert!(msg.contains(var), "error should name the missing variable, got {msg:?}");
        assert!(msg.contains("not set"), "error should explain it is unset, got {msg:?}");
    }

    #[test]
    fn wizard_validate_env_present_passes() {
        let var = "PLENUM_WIZARD_TEST_PRESENT_ABC";
        std::env::set_var(var, "resolved-secret");
        let result = validate_wizard_credential(&WizardCredential::Env(var.to_string()));
        std::env::remove_var(var);
        assert!(result.is_ok(), "present env var should pass validation");
    }

    #[test]
    fn wizard_validate_env_empty_names_the_field_to_fix() {
        let var = "PLENUM_WIZARD_TEST_EMPTY_DEF";
        std::env::set_var(var, "");
        let result = validate_wizard_credential(&WizardCredential::Env(var.to_string()));
        std::env::remove_var(var);
        let err = result.expect_err("empty env var must fail");
        let msg = err.message();
        assert!(msg.contains(var), "error should name the empty variable, got {msg:?}");
        assert!(msg.contains("empty"), "error should explain it is empty, got {msg:?}");
    }
}
