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
use std::collections::HashMap;
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
/// Capabilities define what operations are permitted for a query execution.
/// All capabilities default to the most restrictive settings (read-only).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Allow write operations (INSERT, UPDATE, DELETE)
    /// Default: false (read-only)
    #[serde(default)]
    pub allow_write: bool,

    /// Allow DDL operations (CREATE, DROP, ALTER, etc.)
    /// DDL implicitly grants write permission (DDL is a superset of write)
    /// Default: false
    #[serde(default)]
    pub allow_ddl: bool,

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
    /// Create read-only capabilities (default)
    #[must_use]
    pub fn read_only() -> Self {
        Self::default()
    }

    /// Create write-enabled capabilities
    #[must_use]
    pub fn with_write() -> Self {
        Self { allow_write: true, ..Default::default() }
    }

    /// Create DDL-enabled capabilities (DDL implies write)
    #[must_use]
    pub fn with_ddl() -> Self {
        Self {
            allow_write: true, // DDL implies write
            allow_ddl: true,
            ..Default::default()
        }
    }

    /// Check if write operations are allowed
    /// Returns true if either `allow_write` or `allow_ddl` is true
    #[must_use]
    pub const fn can_write(&self) -> bool {
        self.allow_write || self.allow_ddl
    }

    /// Check if DDL operations are allowed
    #[must_use]
    pub const fn can_ddl(&self) -> bool {
        self.allow_ddl
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

    /// Result rows (each row is a map of column name to value)
    pub rows: Vec<HashMap<String, serde_json::Value>>,

    /// Number of rows affected (for INSERT/UPDATE/DELETE)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_affected: Option<u64>,
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
    /// 2. Queries schema information (tables, columns, keys, indexes)
    /// 3. Closes the connection
    /// 4. Returns schema info or error
    ///
    /// If `schema_filter` is provided, only tables in that schema are returned.
    fn introspect(
        config: &ConnectionConfig,
        schema_filter: Option<&str>,
    ) -> impl std::future::Future<Output = Result<SchemaInfo>> + Send;

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
        assert!(!caps.allow_write);
        assert!(!caps.allow_ddl);
        assert!(caps.max_rows.is_none());
        assert!(caps.timeout_ms.is_none());
    }

    #[test]
    fn test_capabilities_read_only() {
        let caps = Capabilities::read_only();
        assert!(!caps.can_write());
        assert!(!caps.can_ddl());
    }

    #[test]
    fn test_capabilities_with_write() {
        let caps = Capabilities::with_write();
        assert!(caps.can_write());
        assert!(!caps.can_ddl());
    }

    #[test]
    fn test_capabilities_with_ddl() {
        let caps = Capabilities::with_ddl();
        assert!(caps.can_write()); // DDL implies write
        assert!(caps.can_ddl());
    }

    #[test]
    fn test_capabilities_hierarchy() {
        // DDL implies write
        let caps = Capabilities { allow_write: false, allow_ddl: true, ..Default::default() };
        assert!(caps.can_write()); // can_write() returns true because DDL is enabled
        assert!(caps.can_ddl());
    }
}
