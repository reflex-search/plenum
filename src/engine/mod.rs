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

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::Result;

/// TLS/SSL mode for database connections
///
/// Maps to `PostgreSQL`'s `sslmode` parameter and equivalent `MySQL` semantics.
/// `SQLite` ignores this field (no network TLS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// No TLS; plaintext connection only. Fails if server requires TLS.
    #[default]
    Disable,
    /// Require TLS but do not verify server certificate or hostname.
    Require,
    /// Require TLS and verify the server certificate against `ca_cert`.
    /// Hostname is not verified. Requires `ca_cert`.
    VerifyCa,
    /// Require TLS, verify certificate against `ca_cert`, and verify hostname.
    /// Requires `ca_cert`. This is the highest-security mode.
    VerifyFull,
}

/// TLS/SSL configuration for a database connection
///
/// All fields are opt-in. Missing-but-required inputs (e.g. `ca_cert` for
/// `verify-ca`) fail fast with a normalised `CONNECTION_FAILED` error and
/// no credential or path leakage in the message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// SSL mode (disable / require / verify-ca / verify-full)
    pub sslmode: SslMode,

    /// Path to PEM CA certificate file.
    /// Required for `verify-ca` and `verify-full`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,

    /// Path to PEM client certificate file (mTLS).
    /// Must be paired with `client_key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,

    /// Path to PEM client private key file (mTLS).
    /// Must be paired with `client_cert`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,
}

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

    /// TLS/SSL configuration (postgres and mysql only; ignored by sqlite)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsConfig>,
}

impl ConnectionConfig {
    /// Create a new `PostgreSQL` connection config
    #[must_use]
    pub fn postgres(
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
            tls: None,
        }
    }

    /// Create a new `MySQL` connection config
    #[must_use]
    pub fn mysql(
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
            tls: None,
        }
    }

    /// Create a new `SQLite` connection config
    #[must_use]
    pub fn sqlite(file: PathBuf) -> Self {
        Self {
            engine: DatabaseType::SQLite,
            host: None,
            port: None,
            user: None,
            password: None,
            database: None,
            file: Some(file),
            tls: None,
        }
    }
}

/// Connection information returned after successful connection validation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// EXPLAIN output format for the `query` command
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExplainFormat {
    /// Return raw engine rows (default — byte-for-byte unchanged from pre-REF-282 behavior)
    #[default]
    Native,
    /// Return a normalized, engine-stable plan tree in `data.plan`
    Structured,
}

/// Normalized EXPLAIN plan node — engine-stable shape agents can reason about
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainPlanNode {
    /// Engine-specific operation label (e.g. "Seq Scan", "Hash Join", "SCAN TABLE")
    pub node_type: String,
    /// Table or relation name; `null` when the node does not reference one
    pub relation: Option<String>,
    /// Planner's estimated row count; `null` when the engine does not supply it
    pub estimated_rows: Option<f64>,
    /// Planner's estimated cost (engine-specific units); `null` when not available
    pub estimated_cost: Option<f64>,
    /// Child plan nodes (empty for leaf nodes)
    pub children: Vec<ExplainPlanNode>,
}

/// Return `true` when `sql` opens with the `EXPLAIN` keyword (case-insensitive).
pub(crate) fn is_explain_query(sql: &str) -> bool {
    sql.trim().to_uppercase().starts_with("EXPLAIN")
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

    /// Maximum serialized byte size of the rows array in the response.
    /// Truncation occurs at row boundaries; sets `truncated_by="bytes`" when triggered.
    /// None means no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,

    /// Query timeout in milliseconds
    /// None means no timeout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    /// Number of rows to skip before collecting results (for pagination)
    /// None means start from the first row
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,

    /// EXPLAIN output format; `None` / `Native` preserves pre-REF-282 behavior
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain_format: Option<ExplainFormat>,
}

impl Capabilities {
    /// Create new capabilities with optional constraints
    #[must_use]
    pub const fn new(max_rows: Option<usize>, timeout_ms: Option<u64>) -> Self {
        Self { max_rows, max_bytes: None, timeout_ms, offset: None, explain_format: None }
    }
}

/// Schema introspection result
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaInfo {
    /// List of tables in the schema
    pub tables: Vec<TableInfo>,
}

/// Table information
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Table-level comment or description; null when not set or not supported by the engine
    pub comment: Option<String>,

    /// Estimated row count from engine statistics; null when not available
    pub row_estimate: Option<i64>,
}

/// Column information
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Column comment; null when not set or not supported by the engine
    pub comment: Option<String>,
}

/// Foreign key information
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IndexInfo {
    /// Index name
    pub name: String,

    /// Column names included in the index
    pub columns: Vec<String>,

    /// Whether this is a unique index
    pub unique: bool,
}

// Signature is dictated by serde's `skip_serializing_if`, which requires `fn(&T) -> bool`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !b
}

/// Query execution result
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

    /// Whether the result was truncated (by `max_rows` or `max_bytes`); present in output when true
    #[serde(default, skip_serializing_if = "is_false")]
    pub rows_truncated: bool,

    /// Why the result was truncated: "rows" (`max_rows`) or "bytes" (`max_bytes`); absent when not truncated
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_by: Option<String>,

    /// Normalized EXPLAIN plan; populated only when `--explain-format structured` is used
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<ExplainPlanNode>,
}

/// Trim `result.rows` to fit within `max_bytes` of serialized JSON, at row boundaries.
///
/// Each row's contribution is measured as `serde_json::to_string(row).len()`. When the
/// cumulative total would exceed the budget, the result is truncated at that row boundary.
/// Sets `rows_truncated = true` and `truncated_by = Some("bytes")` when truncation occurs.
pub fn apply_byte_budget(result: &mut QueryResult, max_bytes: usize) {
    let mut byte_count: usize = 0;
    let mut cutoff: Option<usize> = None;
    for (i, row) in result.rows.iter().enumerate() {
        let row_bytes = serde_json::to_string(row).map_or(0, |s| s.len());
        if byte_count + row_bytes > max_bytes {
            cutoff = Some(i);
            break;
        }
        byte_count += row_bytes;
    }
    if let Some(n) = cutoff {
        result.rows.truncate(n);
        result.rows_truncated = true;
        result.truncated_by = Some("bytes".to_string());
    }
}

/// Time-only query result (for benchmarking without token consumption)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeOnlyResult {
    /// Query execution time in milliseconds
    pub execution_ms: u64,

    /// Number of rows that matched the query
    pub rows_matched: usize,
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
    ///
    /// `params` are bound server-side via each engine's native placeholders
    /// (`$1`/`$2` for Postgres, `?` for MySQL/SQLite).  Pass an empty slice
    /// when the query has no placeholders.
    fn execute(
        config: &ConnectionConfig,
        query: &str,
        params: &[serde_json::Value],
        caps: &Capabilities,
    ) -> impl std::future::Future<Output = Result<QueryResult>> + Send;
}

/// Change to a column's properties between two schemas
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ColumnChange {
    /// Column name
    pub name: String,
    /// Column state in the base connection
    pub from: ColumnInfo,
    /// Column state in the diff-against connection
    pub to: ColumnInfo,
}

/// Change to a table's primary key between two schemas
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PrimaryKeyChange {
    /// Primary key columns in the base connection; null if no primary key
    pub from: Option<Vec<String>>,
    /// Primary key columns in the diff-against connection; null if no primary key
    pub to: Option<Vec<String>>,
}

/// Change to a view's SQL definition between two schemas
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DefinitionChange {
    /// View definition in the base connection; null if not available
    pub from: Option<String>,
    /// View definition in the diff-against connection; null if not available
    pub to: Option<String>,
}

/// Structural diff of a single table between two schemas
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TableDiff {
    /// Table name
    pub name: String,
    /// Columns present in the diff-against connection but not in the base
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_added: Vec<ColumnInfo>,
    /// Columns present in the base connection but not in the diff-against
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_removed: Vec<ColumnInfo>,
    /// Columns present in both connections whose definition changed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_changed: Vec<ColumnChange>,
    /// Primary key change; present only when the primary key differs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_key_changed: Option<PrimaryKeyChange>,
    /// Indexes present in the diff-against connection but not in the base
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes_added: Vec<IndexInfo>,
    /// Indexes present in the base connection but not in the diff-against
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes_removed: Vec<IndexInfo>,
    /// Foreign keys present in the diff-against connection but not in the base
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys_added: Vec<ForeignKeyInfo>,
    /// Foreign keys present in the base connection but not in the diff-against
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys_removed: Vec<ForeignKeyInfo>,
}

/// Structural diff of a single view between two schemas
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ViewDiff {
    /// View name
    pub name: String,
    /// SQL definition change; present only when the definition differs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_changed: Option<DefinitionChange>,
    /// Columns present in the diff-against connection but not in the base
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_added: Vec<ColumnInfo>,
    /// Columns present in the base connection but not in the diff-against
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_removed: Vec<ColumnInfo>,
    /// Columns present in both connections whose definition changed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns_changed: Vec<ColumnChange>,
}

/// Full structural schema diff between two connections
///
/// Produced by `plenum introspect --diff-against <name>`.
/// All top-level arrays are always present; identical schemas produce all-empty arrays.
/// Stable ordering: all arrays sorted alphabetically by name for deterministic output.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchemaDiff {
    /// Tables present in the diff-against connection but not in the base
    pub tables_added: Vec<String>,
    /// Tables present in the base connection but not in the diff-against
    pub tables_removed: Vec<String>,
    /// Tables present in both connections with structural changes
    pub tables_changed: Vec<TableDiff>,
    /// Views present in the diff-against connection but not in the base
    pub views_added: Vec<String>,
    /// Views present in the base connection but not in the diff-against
    pub views_removed: Vec<String>,
    /// Views present in both connections with changes
    pub views_changed: Vec<ViewDiff>,
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
        assert!(pg_config.tls.is_none());

        let mysql_config = ConnectionConfig::mysql(
            "localhost".to_string(),
            3306,
            "user".to_string(),
            "pass".to_string(),
            "db".to_string(),
        );
        assert_eq!(mysql_config.engine, DatabaseType::MySQL);
        assert_eq!(mysql_config.port, Some(3306));
        assert!(mysql_config.tls.is_none());

        let sqlite_config = ConnectionConfig::sqlite(PathBuf::from("/tmp/test.db"));
        assert_eq!(sqlite_config.engine, DatabaseType::SQLite);
        assert!(sqlite_config.file.is_some());
        assert!(sqlite_config.tls.is_none());
    }

    #[test]
    fn test_ssl_mode_serialization() {
        assert_eq!(serde_json::to_string(&SslMode::Disable).unwrap(), r#""disable""#);
        assert_eq!(serde_json::to_string(&SslMode::Require).unwrap(), r#""require""#);
        assert_eq!(serde_json::to_string(&SslMode::VerifyCa).unwrap(), r#""verify-ca""#);
        assert_eq!(serde_json::to_string(&SslMode::VerifyFull).unwrap(), r#""verify-full""#);
    }

    #[test]
    fn test_ssl_mode_default_is_disable() {
        let mode = SslMode::default();
        assert_eq!(mode, SslMode::Disable);
    }

    #[test]
    fn test_tls_config_serialization_omits_none_fields() {
        let tls = TlsConfig {
            sslmode: SslMode::Require,
            ca_cert: None,
            client_cert: None,
            client_key: None,
        };
        let json = serde_json::to_string(&tls).unwrap();
        assert!(json.contains("\"sslmode\":\"require\""));
        assert!(!json.contains("ca_cert"));
        assert!(!json.contains("client_cert"));
        assert!(!json.contains("client_key"));
    }

    #[test]
    fn test_connection_config_tls_roundtrip() {
        let mut config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "user".to_string(),
            "pass".to_string(),
            "db".to_string(),
        );
        config.tls = Some(TlsConfig {
            sslmode: SslMode::VerifyFull,
            ca_cert: Some(PathBuf::from("/etc/ssl/ca.pem")),
            client_cert: None,
            client_key: None,
        });

        let json = serde_json::to_string(&config).unwrap();
        let parsed: ConnectionConfig = serde_json::from_str(&json).unwrap();
        let tls = parsed.tls.unwrap();
        assert_eq!(tls.sslmode, SslMode::VerifyFull);
        assert_eq!(tls.ca_cert, Some(PathBuf::from("/etc/ssl/ca.pem")));
    }

    #[test]
    fn test_connection_config_tls_absent_omits_field() {
        let config = ConnectionConfig::postgres(
            "localhost".to_string(),
            5432,
            "user".to_string(),
            "pass".to_string(),
            "db".to_string(),
        );
        let json = serde_json::to_string(&config).unwrap();
        // tls field should not appear in JSON when None
        assert!(!json.contains("\"tls\""));
    }

    #[test]
    fn test_capabilities_defaults() {
        let caps = Capabilities::default();
        assert!(caps.max_rows.is_none());
        assert!(caps.max_bytes.is_none());
        assert!(caps.timeout_ms.is_none());
    }

    #[test]
    fn test_capabilities_new() {
        let caps = Capabilities::new(Some(100), Some(5000));
        assert_eq!(caps.max_rows, Some(100));
        assert!(caps.max_bytes.is_none());
        assert_eq!(caps.timeout_ms, Some(5000));
    }

    #[test]
    fn test_apply_byte_budget_truncates_at_row_boundary() {
        use serde_json::json;
        let mut result = QueryResult {
            columns: vec!["v".to_string()],
            rows: vec![
                vec![json!("aaaaaaaaaa")], // ~14 bytes serialized
                vec![json!("bbbbbbbbbb")], // ~14 bytes
                vec![json!("cccccccccc")], // ~14 bytes
            ],
            rows_affected: None,
            execution_ms: 0,
            rows_truncated: false,
            truncated_by: None,
            plan: None,
        };
        // Budget tight enough for 2 rows but not 3
        apply_byte_budget(&mut result, 30);
        assert_eq!(result.rows.len(), 2);
        assert!(result.rows_truncated);
        assert_eq!(result.truncated_by.as_deref(), Some("bytes"));
    }

    #[test]
    fn test_apply_byte_budget_no_truncation_when_under_budget() {
        use serde_json::json;
        let mut result = QueryResult {
            columns: vec!["v".to_string()],
            rows: vec![vec![json!(1)], vec![json!(2)]],
            rows_affected: None,
            execution_ms: 0,
            rows_truncated: false,
            truncated_by: None,
            plan: None,
        };
        apply_byte_budget(&mut result, 1_000_000);
        assert_eq!(result.rows.len(), 2);
        assert!(!result.rows_truncated);
        assert!(result.truncated_by.is_none());
    }

    #[test]
    fn test_apply_byte_budget_zero_budget_returns_empty() {
        use serde_json::json;
        let mut result = QueryResult {
            columns: vec!["v".to_string()],
            rows: vec![vec![json!(1)]],
            rows_affected: None,
            execution_ms: 0,
            rows_truncated: false,
            truncated_by: None,
            plan: None,
        };
        apply_byte_budget(&mut result, 0);
        assert_eq!(result.rows.len(), 0);
        assert!(result.rows_truncated);
        assert_eq!(result.truncated_by.as_deref(), Some("bytes"));
    }
}
