//! Database Engine Traits and Core Types
//!
//! This module defines the core abstractions for database engines.
//! Each engine (`PostgreSQL`, `MySQL`, `SQLite`) implements the `DatabaseEngine` trait.
//!
//! # Stateless Design
//! All trait methods are stateless and take `&ConnectionConfig` as input.
//! Connections are opened, used, and closed within each method call.
//!
//! # Engine Isolation
//! Each engine implementation is completely independent.
//! No shared SQL helpers or cross-engine abstractions.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::Result;

// Engine-specific implementations
#[cfg(feature = "sqlite")]
pub mod sqlite; // Phase 3 ✅

#[cfg(feature = "postgres")]
pub mod postgres; // Phase 4 (in progress)

// MySQL engine (Phase 5)
#[cfg(feature = "mysql")]
pub mod mysql;

/// Supported database engine types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseType {
    /// `PostgreSQL` database
    Postgres,
    /// `MySQL` database (includes `MariaDB`)
    MySQL,
    /// `SQLite` database
    SQLite,
}

impl DatabaseType {
    /// Get the engine name as a string
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::MySQL => "mysql",
            Self::SQLite => "sqlite",
        }
    }
}

impl std::fmt::Display for DatabaseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Connection configuration for database engines
///
/// This struct contains all parameters needed to establish a database connection.
/// Fields are engine-specific (e.g., `file` only applies to `SQLite`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Database engine type
    pub engine: DatabaseType,

    /// Hostname (for postgres/mysql)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Port number (for postgres/mysql)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Username (for postgres/mysql)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Password (for postgres/mysql)
    /// WARNING: Sensitive data, do not log or include in error messages
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Database name (for postgres/mysql)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Database file path (for sqlite)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl ConnectionConfig {
    /// Create a new `PostgreSQL` connection config
    #[must_use]
    pub const fn postgres(
        host: String,
        port: u16,
        user: String,
        password: String,
        database: String,
    ) -> Self {
        Self {
            engine: DatabaseType::Postgres,
            host: Some(host),
            port: Some(port),
            user: Some(user),
            password: Some(password),
            database: Some(database),
            file: None,
        }
    }

    /// Create a new `MySQL` connection config
    #[must_use]
    pub const fn mysql(
        host: String,
        port: u16,
        user: String,
        password: String,
        database: String,
    ) -> Self {
        Self {
            engine: DatabaseType::MySQL,
            host: Some(host),
            port: Some(port),
            user: Some(user),
            password: Some(password),
            database: Some(database),
            file: None,
        }
    }

    /// Create a new `SQLite` connection config
    #[must_use]
    pub const fn sqlite(file: PathBuf) -> Self {
        Self {
            engine: DatabaseType::SQLite,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
            file: Some(file),
        }
    }
}

/// Connection information returned after successful connection validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    /// Database server version string
    pub database_version: String,

    /// Server information (implementation-specific)
    pub server_info: String,

    /// Name of the connected database
    pub connected_database: String,

    /// Connected user name
    pub user: String,
}

/// Query execution capabilities
///
/// Capabilities define constraints for query execution.
/// Plenum is strictly read-only - all write and DDL operations are rejected.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Maximum number of rows to return (enforced by engine)
    /// None means no limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<usize>,

    /// Query timeout in milliseconds
    /// None means no timeout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl Capabilities {
    /// Create new capabilities with optional constraints
    #[must_use]
    pub const fn new(max_rows: Option<usize>, timeout_ms: Option<u64>) -> Self {
        Self { max_rows, timeout_ms }
    }
}

/// Schema introspection result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaInfo {
    /// List of tables in the schema
    pub tables: Vec<TableInfo>,
}

/// Table information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    /// Table name
    pub name: String,

    /// Schema name (for engines that support schemas)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Table columns
    pub columns: Vec<ColumnInfo>,

    /// Primary key columns
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<Vec<String>>,

    /// Foreign keys
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKeyInfo>,

    /// Indexes
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<IndexInfo>,
}

/// Column information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    /// Column name
    pub name: String,

    /// Column data type (engine-specific)
    pub data_type: String,

    /// Whether column allows NULL values
    pub nullable: bool,

    /// Default value (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Foreign key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyInfo {
    /// Foreign key constraint name
    pub name: String,

    /// Column names in this table
    pub columns: Vec<String>,

    /// Referenced table name
    pub referenced_table: String,

    /// Referenced column names
    pub referenced_columns: Vec<String>,
}

/// Index information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    /// Index name
    pub name: String,

    /// Column names included in the index
    pub columns: Vec<String>,

    /// Whether this is a unique index
    pub unique: bool,
}

/// Query execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Column names in result set
    pub columns: Vec<String>,

    /// Result rows (each row is an array of values in column order)
    pub rows: Vec<Vec<serde_json::Value>>,

    /// Number of rows affected (for INSERT/UPDATE/DELETE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_affected: Option<u64>,

    /// Query execution time in milliseconds
    pub execution_ms: u64,
}

/// Introspection operation types
///
/// Defines the type of introspection operation to perform.
/// Each operation returns different data (see `IntrospectResult`).
#[derive(Debug, Clone)]
pub enum IntrospectOperation {
    /// List all databases (requires wildcard connection)
    ListDatabases,

    /// List all schemas (Postgres only - `MySQL` uses database=schema, `SQLite` has none)
    ListSchemas,

    /// List all table names
    ListTables,

    /// List all view names
    ListViews,

    /// List all indexes (optionally filtered to a specific table)
    ListIndexes {
        /// Optional table name to filter indexes
        table: Option<String>,
    },

    /// Get full details for a specific table
    TableDetails {
        /// Table name to introspect
        name: String,
        /// Which fields to include in the result
        fields: TableFields,
    },

    /// Get details for a specific view
    ViewDetails {
        /// View name to introspect
        name: String,
    },
}

/// Table detail field selectors
///
/// Controls which fields are included when introspecting a specific table.
/// Default (all true) returns complete table information.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct TableFields {
    /// Include column information
    pub columns: bool,
    /// Include primary key
    pub primary_key: bool,
    /// Include foreign keys
    pub foreign_keys: bool,
    /// Include indexes
    pub indexes: bool,
}

impl Default for TableFields {
    fn default() -> Self {
        Self { columns: true, primary_key: true, foreign_keys: true, indexes: true }
    }
}

impl TableFields {
    /// Create field selectors with all fields enabled
    #[must_use]
    pub const fn all() -> Self {
        Self { columns: true, primary_key: true, foreign_keys: true, indexes: true }
    }

    /// Create field selectors with specific fields enabled
    #[must_use]
    #[allow(clippy::fn_params_excessive_bools)]
    pub const fn new(columns: bool, primary_key: bool, foreign_keys: bool, indexes: bool) -> Self {
        Self { columns, primary_key, foreign_keys, indexes }
    }
}

/// Introspection result
///
/// The result type depends on which `IntrospectOperation` was requested.
/// Only the relevant variant will be populated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntrospectResult {
    /// List of database names
    DatabaseList {
        /// Database names
        databases: Vec<String>,
    },

    /// List of schema names
    SchemaList {
        /// Schema names
        schemas: Vec<String>,
    },

    /// List of table names
    TableList {
        /// Table names
        tables: Vec<String>,
    },

    /// List of view names
    ViewList {
        /// View names
        views: Vec<String>,
    },

    /// List of indexes
    IndexList {
        /// Index summaries
        indexes: Vec<IndexSummary>,
    },

    /// Full table details
    TableDetails {
        /// Table information
        table: TableInfo,
    },

    /// View details
    ViewDetails {
        /// View information
        view: ViewInfo,
    },
}

/// Index summary (used in `ListIndexes` operation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSummary {
    /// Index name
    pub name: String,

    /// Table the index belongs to
    pub table: String,

    /// Whether this is a unique index
    pub unique: bool,

    /// Column names included in the index
    pub columns: Vec<String>,
}

/// View information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewInfo {
    /// View name
    pub name: String,

    /// Schema name (for engines that support schemas)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// View definition (SQL source, if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,

    /// View columns
    pub columns: Vec<ColumnInfo>,
}

/// Database engine trait
///
/// All database engines implement this trait.
/// Each method is stateless and takes a connection config as input.
pub trait DatabaseEngine {
    /// Validate connection and return connection information
    ///
    /// This method:
    /// 1. Opens a connection using the provided config
    /// 2. Retrieves server version and connection metadata
    /// 3. Closes the connection
    /// 4. Returns connection info or error
    ///
    /// No persistent connection is maintained.
    fn validate_connection(
        config: &ConnectionConfig,
    ) -> impl std::future::Future<Output = Result<ConnectionInfo>> + Send;

    /// Introspect database schema
    ///
    /// This method:
    /// 1. Opens a connection
    /// 2. Performs the requested introspection operation
    /// 3. Closes the connection
    /// 4. Returns operation-specific results or error
    ///
    /// # Parameters
    /// - `config`: Database connection configuration
    /// - `operation`: The type of introspection to perform
    /// - `database`: Optional database override (reconnects with different database)
    /// - `schema`: Optional schema filter (Postgres/MySQL only, ignored by `SQLite`)
    ///
    /// # Errors
    /// - Operation not supported by engine (e.g., `ListSchemas` on MySQL/SQLite)
    /// - Connection failure
    /// - Table/view not found (for detail operations)
    /// - Query execution failure
    fn introspect(
        config: &ConnectionConfig,
        operation: &IntrospectOperation,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> impl std::future::Future<Output = Result<IntrospectResult>> + Send;

    /// Execute a query with capability constraints
    ///
    /// This method:
    /// 1. Opens a connection
    /// 2. Validates capabilities against the query
    /// 3. Executes the query if permitted
    /// 4. Closes the connection
    /// 5. Returns query results or error
    ///
    /// Capability violations MUST fail before query execution.
    fn execute(
        config: &ConnectionConfig,
        query: &str,
        caps: &Capabilities,
    ) -> impl std::future::Future<Output = Result<QueryResult>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_type_serialization() {
        assert_eq!(serde_json::to_string(&DatabaseType::Postgres).unwrap(), r#""postgres""#);
        assert_eq!(serde_json::to_string(&DatabaseType::MySQL).unwrap(), r#""mysql""#);
        assert_eq!(serde_json::to_string(&DatabaseType::SQLite).unwrap(), r#""sqlite""#);
    }

    #[test]
    fn test_connection_config_constructors() {
        let pg_config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "user".to_string(),
            "pass".to_string(),
            "db".to_string(),
        );
        assert_eq!(pg_config.engine, DatabaseType::Postgres);
        assert_eq!(pg_config.port, Some(5432));

        let mysql_config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "user".to_string(),
            "pass".to_string(),
            "db".to_string(),
        );
        assert_eq!(mysql_config.engine, DatabaseType::MySQL);
        assert_eq!(mysql_config.port, Some(3306));

        let sqlite_config = ConnectionConfig::sqlite(PathBuf::from("/tmp/test.db"));
        assert_eq!(sqlite_config.engine, DatabaseType::SQLite);
        assert!(sqlite_config.file.is_some());
    }

    #[test]
    fn test_capabilities_defaults() {
        let caps = Capabilities::default();
        assert!(caps.max_rows.is_none());
        assert!(caps.timeout_ms.is_none());
    }

    #[test]
    fn test_capabilities_new() {
        let caps = Capabilities::new(Some(100), Some(5000));
        assert_eq!(caps.max_rows, Some(100));
        assert_eq!(caps.timeout_ms, Some(5000));
    }
}
