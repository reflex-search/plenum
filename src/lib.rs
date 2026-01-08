//! Plenum - Agent-First Database Control CLI
//!
//! Plenum is a lightweight, agent-first database control CLI designed for autonomous AI coding
//! agents. It provides a deterministic, least-privilege execution surface for database operations.
//!
//! # Core Principles
//! - Agent-first, machine-only interface (JSON-only output)
//! - No query language abstraction (vendor-specific SQL)
//! - Explicit over implicit (no inferred values)
//! - Least privilege by default (read-only mode)
//! - Deterministic behavior (identical inputs → identical outputs)
//!
//! # Architecture
//! This library provides the core functionality for both CLI and MCP interfaces.
//! Both interfaces are thin wrappers that call the same internal library functions.
//!
//! # Module Organization
//! - [`error`] - Error types and handling
//! - [`output`] - JSON output envelope types
//! - [`engine`] - Database engine trait and core types
//! - [`capability`] - Capability validation and SQL categorization
//! - [`config`] - Configuration management
//!
//! # Public API
//! This library exports types and functions for use by both CLI and MCP interfaces:
//! - Core types: [`ConnectionConfig`], [`Capabilities`], [`ConnectionInfo`], etc.
//! - Envelopes: [`SuccessEnvelope`], [`ErrorEnvelope`]
//! - Errors: [`PlenumError`]
//! - Functions: Configuration resolution and validation

// Core modules (Phase 1)
pub mod error;       // Error handling infrastructure (Phase 1.3)
pub mod output;      // JSON output envelopes (Phase 1.2)
pub mod engine;      // Database engine trait and implementations (Phase 1.1, 3-5)
pub mod capability;  // Capability validation and enforcement (Phase 1.4)
pub mod config;      // Configuration management (Phase 1.5)
pub mod mcp;         // MCP server (Phase 7) - Manual JSON-RPC 2.0 implementation

// Re-export commonly used types for convenience
pub use error::{PlenumError, Result};
pub use output::{ErrorEnvelope, ErrorInfo, Metadata, SuccessEnvelope};
pub use engine::{
    Capabilities, ColumnInfo, ConnectionConfig, ConnectionInfo, DatabaseEngine, DatabaseType,
    ForeignKeyInfo, IndexInfo, QueryResult, SchemaInfo, TableInfo,
};
pub use capability::{validate_query, QueryCategory};
pub use config::{
    resolve_connection, save_connection, list_connections,
    ConfigLocation, ConnectionRegistry, StoredConnection,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_api_exports() {
        // Verify that key types are accessible
        let _caps = Capabilities::default();
        let _engine_type = DatabaseType::Postgres;

        // This test ensures the public API is properly exported
        assert!(true, "Public API exports are accessible");
    }
}
