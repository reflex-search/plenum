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
        // Connection parameters will be added in Phase 2.2
        #[arg(long)]
        placeholder: Option<String>,
    },

    /// Introspect database schema
    Introspect {
        // Introspection parameters will be added in Phase 2.3
        #[arg(long)]
        placeholder: Option<String>,
    },

    /// Execute constrained SQL queries
    Query {
        // Query parameters will be added in Phase 2.4
        #[arg(long)]
        placeholder: Option<String>,
    },

    /// Start MCP server (hidden from help, for AI agent integration)
    #[command(hide = true)]
    Mcp {
        // MCP server will be implemented in Phase 7
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Phase 0: Placeholder implementation
    // Actual command routing will be implemented in Phase 2
    match cli.command {
        Some(Commands::Connect { .. }) => {
            println!(
                r#"{{"ok": false, "error": {{"code": "NOT_IMPLEMENTED", "message": "connect command not yet implemented - Phase 2.2"}}}}"#
            );
        }
        Some(Commands::Introspect { .. }) => {
            println!(
                r#"{{"ok": false, "error": {{"code": "NOT_IMPLEMENTED", "message": "introspect command not yet implemented - Phase 2.3"}}}}"#
            );
        }
        Some(Commands::Query { .. }) => {
            println!(
                r#"{{"ok": false, "error": {{"code": "NOT_IMPLEMENTED", "message": "query command not yet implemented - Phase 2.4"}}}}"#
            );
        }
        Some(Commands::Mcp { .. }) => {
            println!(
                r#"{{"ok": false, "error": {{"code": "NOT_IMPLEMENTED", "message": "mcp server not yet implemented - Phase 7"}}}}"#
            );
        }
        None => {
            // No subcommand provided, show help
            println!(
                r#"{{"ok": false, "error": {{"code": "NO_SUBCOMMAND", "message": "No subcommand provided. Use --help to see available commands."}}}}"#
            );
        }
    }
}
