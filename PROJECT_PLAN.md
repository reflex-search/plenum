# PROJECT_PLAN.md

## Project: Plenum - Agent-First Database Control CLI

**Version:** MVP 1.0  
**Target Completion:** TBD  
**Implementation Language:** Rust

---

## Phase 0: Project Foundation

### 0.1 Repository Setup
- [x] Initialize Git repository ✅
- [x] Create documentation files ✅
  - [x] CLAUDE.md (core principles and architecture)
  - [x] PROJECT_PLAN.md (implementation roadmap)
  - [x] PROBLEMS.md (architectural issues tracking)
  - [x] RESEARCH.md (design decisions and rationale)
  - [x] README.md (placeholder - needs expansion)
- [ ] Initialize Rust project structure
  - [ ] Run `cargo init --name plenum --lib`
  - [ ] Configure Cargo.toml with both binary and library targets:
    ```toml
    [lib]
    name = "plenum"
    path = "src/lib.rs"

    [[bin]]
    name = "plenum"
    path = "src/main.rs"
    ```
  - [ ] Configure Cargo.toml with project metadata (version, authors, edition, license)
  - [ ] Set up `src/lib.rs` to export internal API
  - [ ] Set up `src/main.rs` as CLI entry point
- [ ] Configure `.gitignore` for Rust builds
  - [ ] Add `/target`
  - [ ] Add `/.idea` and other IDE-specific directories
  - [ ] Note: Do NOT add `Cargo.lock` (should be committed for binary projects)
- [ ] Add LICENSE file (MIT OR Apache-2.0)
- [ ] Expand README.md with project description and build instructions

### 0.2 Development Environment
- [ ] Define Rust toolchain version (stable/nightly)
- [ ] Configure rustfmt.toml for code formatting
- [ ] Configure clippy rules for linting
- [ ] Set up CI/CD pipeline configuration (GitHub Actions)
- [ ] Document build/test commands

### 0.3 Dependency Assessment

**CRITICAL: MCP Architecture Research (moved from Phase 7.1)**
- [ ] Research MCP (Model Context Protocol) implementation in Rust
  - [ ] Evaluate `rmcp` crate (official Rust SDK for MCP)
  - [ ] Verify stdio transport compatibility with stateless design
  - [ ] Confirm JSON-RPC protocol handling via `rmcp`
  - [ ] Test `#[tool]` macro pattern compatibility with our architecture
  - [ ] Verify async requirements (tokio integration)
  - [ ] Review reflex-search implementation pattern: https://github.com/reflex-search/reflex
- [ ] Document MCP architecture decision:
  - [ ] **Decision:** Single crate with `plenum mcp` subcommand (not workspace)
  - [ ] **Pattern:** Follow reflex-search implementation pattern
  - [ ] **Rationale:** Simpler structure, proven pattern, uses standard tooling
  - [ ] **Key principle:** Both CLI and MCP call same internal library functions
- [ ] Select MCP dependencies:
  - [ ] `rmcp` - Official Rust MCP SDK
  - [ ] `tokio` - Async runtime (required by rmcp)
  - [ ] `schemars` - JSON schema generation for MCP tool definitions

**Database Driver Selection (MUST use native drivers)**
- [ ] Research and select database driver crates:
  - [ ] PostgreSQL: `tokio-postgres` (native driver, NOT sqlx)
  - [ ] MySQL: `mysql_async` (native driver, NOT sqlx)
  - [ ] SQLite: `rusqlite` (native driver, NOT sqlx)
- [ ] Document rationale for native drivers:
  - [ ] Maximum isolation between engines
  - [ ] Vendor-specific behavior preserved
  - [ ] No risk of abstraction leakage
  - [ ] Each engine handles its own quirks independently
  - [ ] Aligns with "no compatibility layers" principle (CLAUDE.md)

**Core Libraries**
- [ ] Select JSON serialization library: `serde_json`
- [ ] Select CLI framework: `clap`
- [ ] Select error handling: `thiserror` and `anyhow`
- [ ] Select configuration management libraries:
  - [ ] Interactive prompts: `dialoguer` or `inquire`
  - [ ] Cross-platform config paths: `dirs`
  - [ ] Config format: JSON via `serde_json`
- [ ] Document dependency rationale in RESEARCH.md

---

## Phase 1: Core Architecture

### 1.1 Define Core Traits
- [ ] Create `src/engine/mod.rs` with trait definitions
- [ ] Define `DatabaseEngine` trait (stateless design)
  - [ ] `fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo>`
  - [ ] `fn introspect(config: &ConnectionConfig, schema_filter: Option<&str>) -> Result<SchemaInfo>`
  - [ ] `fn execute(config: &ConnectionConfig, query: &str, caps: &Capabilities) -> Result<QueryResult>`
- [ ] Define `Capabilities` struct
  - [ ] `allow_write: bool` (default: false)
  - [ ] `allow_ddl: bool` (default: false)
  - [ ] `max_rows: Option<usize>`
  - [ ] `timeout_ms: Option<u64>`
- [ ] Define `ConnectionConfig` struct
  - [ ] `engine: DatabaseType`
  - [ ] `host: Option<String>` (for postgres/mysql)
  - [ ] `port: Option<u16>` (for postgres/mysql)
  - [ ] `user: Option<String>` (for postgres/mysql)
  - [ ] `password: Option<String>` (for postgres/mysql)
  - [ ] `database: Option<String>` (for postgres/mysql)
  - [ ] `file: Option<PathBuf>` (for sqlite)
- [ ] Define `ConnectionInfo` struct (returned by validate_connection)
  - [ ] `database_version: String`
  - [ ] `server_info: String`
  - [ ] `connected_database: String`
  - [ ] `user: String`
- [ ] Define `SchemaInfo` struct
- [ ] Define `QueryResult` struct

### 1.2 Define Output Envelope Types
- [ ] Create `src/output/mod.rs`
- [ ] Define `SuccessEnvelope<T>` struct
  - [ ] `ok: bool` (always true)
  - [ ] `engine: String`
  - [ ] `command: String`
  - [ ] `data: T`
  - [ ] `meta: Metadata`
- [ ] Define `ErrorEnvelope` struct
  - [ ] `ok: bool` (always false)
  - [ ] `engine: String`
  - [ ] `command: String`
  - [ ] `error: ErrorInfo`
- [ ] Define `ErrorInfo` struct
  - [ ] `code: String`
  - [ ] `message: String`
- [ ] Define `Metadata` struct
  - [ ] `execution_ms: u64`
  - [ ] `rows_returned: Option<usize>`
- [ ] Implement `Serialize` for all envelope types

### 1.3 Error Handling Infrastructure
- [ ] Create `src/error/mod.rs`
- [ ] Define `PlenumError` enum with variants:
  - [ ] `CapabilityViolation(String)`
  - [ ] `ConnectionFailed(String)`
  - [ ] `QueryFailed(String)`
  - [ ] `InvalidInput(String)`
  - [ ] `EngineError { engine: String, detail: String }`
- [ ] Implement error code mapping
- [ ] Implement conversion to `ErrorEnvelope`
- [ ] Ensure no panics across public boundaries

### 1.4 Capability Validation
- [ ] Create `src/capability/mod.rs`
- [ ] Implement capability validator
- [ ] **SQL Categorization Strategy: Regex-based with engine-specific implementations**
  - [ ] **Rationale:** Simplest explicit implementation, no external dependencies, respects vendor SQL differences
  - [ ] **Pattern:** Each engine implements its own `categorize_query(sql: &str) -> Result<QueryCategory>` logic
  - [ ] **No shared SQL helpers across engines** (aligns with CLAUDE.md principle)
- [ ] Define SQL statement categorization:
  - [ ] Read-only: SELECT, WITH ... SELECT (CTEs)
  - [ ] Write: INSERT, UPDATE, DELETE, CALL/EXEC (stored procedures)
  - [ ] DDL: CREATE, DROP, ALTER, TRUNCATE, RENAME
  - [ ] Transaction control: BEGIN, COMMIT, ROLLBACK (treat as read-only)
- [ ] Implement SQL pre-processing (before categorization):
  - [ ] Trim leading/trailing whitespace
  - [ ] Strip SQL comments: `--` line comments and `/* */` block comments
  - [ ] Normalize to uppercase for pattern matching
  - [ ] **Detect multi-statement queries** (contains `;` separators)
  - [ ] **Reject multi-statement queries in MVP** (safest approach, can relax post-MVP)
- [ ] Implement engine-specific categorization:
  - [ ] PostgreSQL: Standard SQL categorization
  - [ ] MySQL: Include implicit commit DDL list (CREATE/ALTER/DROP/TRUNCATE/RENAME/LOCK TABLES)
  - [ ] SQLite: SQLite-specific DDL handling
- [ ] Handle edge cases:
  - [ ] **EXPLAIN queries**: Strip EXPLAIN prefix, categorize underlying statement
  - [ ] **EXPLAIN ANALYZE**: Categorize underlying statement (executes in PostgreSQL)
  - [ ] **CTEs (WITH)**: Match final statement type (e.g., `WITH ... SELECT` → read-only)
  - [ ] **Stored procedures (CALL/EXEC)**: Treat as write (conservative, procedures can do anything)
  - [ ] **Transaction control (BEGIN/COMMIT/ROLLBACK)**: Treat as read-only (no-op without write capability)
  - [ ] **Unknown statement types**: Treat as DDL (fail-safe, most restrictive)
  - [ ] **Empty queries**: Return error
  - [ ] **Parsing errors**: Return error
- [ ] Implement capability hierarchy:
  - [ ] **DDL implies write**: If `allow_ddl = true`, treat `allow_write` as true
  - [ ] **Write does NOT imply DDL**: `allow_write` alone cannot execute DDL
  - [ ] Read-only is default (both `allow_write` and `allow_ddl` are false)
  - [ ] Rationale: DDL operations are inherently write operations (more dangerous)
- [ ] Implement pre-execution capability checks:
  - [ ] DDL queries require `allow_ddl = true` (explicit flag required)
  - [ ] Write queries require `allow_write = true` OR `allow_ddl = true`
  - [ ] Read-only queries always permitted
- [ ] Handle MySQL implicit commit cases:
  - [ ] Maintain explicit list of DDL statements that cause implicit commit
  - [ ] Document in MySQL engine module
  - [ ] Surface in error messages if needed
- [ ] Add capability validation unit tests:
  - [ ] **Comprehensive edge case matrix per engine**
  - [ ] Comment variations (`--`, `/* */`, mixed)
  - [ ] Whitespace variations (leading, trailing, mixed)
  - [ ] Case sensitivity (lowercase, uppercase, mixed)
  - [ ] CTE queries (`WITH ... SELECT`, `WITH ... INSERT`)
  - [ ] EXPLAIN queries (with and without ANALYZE)
  - [ ] Transaction control (BEGIN, COMMIT, ROLLBACK)
  - [ ] Multi-statement detection (should reject)
  - [ ] Unknown statement types (should default to DDL)
  - [ ] Empty queries (should error)
  - [ ] Stored procedure calls (CALL, EXEC)
  - [ ] Engine-specific edge cases (PostgreSQL/MySQL/SQLite quirks)

### 1.5 Configuration Management
- [ ] Create `src/config/mod.rs`
- [ ] Define configuration file formats:
  - [ ] Local: `.plenum/config.json` (team-shareable)
  - [ ] Global: `~/.config/plenum/connections.json` (per-user)
- [ ] Define `ConnectionRegistry` for loading/saving configs
- [ ] Implement config file structure:
  - [ ] Named connection profiles
  - [ ] Default connection selection
  - [ ] Per-project scoping for global config (keyed by working directory)
- [ ] Implement config loading with precedence:
  - [ ] Explicit CLI flags (highest priority)
  - [ ] Local config (`.plenum/config.json`)
  - [ ] Global config (`~/.config/plenum/connections.json`)
- [ ] Support environment variable substitution:
  - [ ] `password_env` field for credential security
  - [ ] Resolve env vars at runtime
- [ ] Implement config saving:
  - [ ] Save to local vs global locations
  - [ ] Update existing named connections
  - [ ] Create new named connections
- [ ] Add config validation:
  - [ ] Required fields per engine type
  - [ ] Connection name uniqueness
  - [ ] File permissions checks
- [ ] Add config migration/versioning support
- [ ] Implement connection resolution logic:
  - [ ] By name (`--name prod`)
  - [ ] Runtime parameter overrides
  - [ ] Fallback to default connection

### 1.6 Library Module Structure
- [ ] Create `src/lib.rs` with public API exports
- [ ] **IMPORTANT:** Design all modules for reuse by both CLI and MCP
- [ ] Export core types for both CLI and MCP use:
  - [ ] `pub use engine::{DatabaseEngine, ConnectionConfig, ConnectionInfo, SchemaInfo, QueryResult};`
  - [ ] `pub use capability::Capabilities;`
  - [ ] `pub use output::{SuccessEnvelope, ErrorEnvelope};`
  - [ ] `pub use config::{resolve_connection, save_connection};`
  - [ ] `pub use error::PlenumError;`
- [ ] Design internal functions to be CLI/MCP agnostic:
  - [ ] `execute_connect(config: ConnectionConfig) -> Result<ConnectionInfo>`
  - [ ] `execute_introspect(config: ConnectionConfig, filter: Option<&str>) -> Result<SchemaInfo>`
  - [ ] `execute_query(config: ConnectionConfig, sql: &str, caps: Capabilities) -> Result<QueryResult>`
- [ ] Ensure all business logic lives in library modules, not in CLI/MCP wrappers
- [ ] CLI and MCP should be thin wrappers calling library functions
- [ ] Document public API in module-level docs

---

## Phase 2: CLI Foundation

### 2.1 CLI Structure
- [ ] Create `src/main.rs` with CLI entry point
- [ ] Set up `clap` with four subcommands:
  - [ ] `connect` - Connection configuration management
  - [ ] `introspect` - Schema introspection
  - [ ] `query` - Constrained query execution
  - [ ] `mcp` - MCP server (hidden from help, for AI agent integration)
- [ ] Define common flags for connection parameters:
  - [ ] `--engine <postgres|mysql|sqlite>`
  - [ ] `--host`, `--port`, `--user`, `--password`, `--database`, `--file`
- [ ] Ensure stdout is JSON-only (for both CLI and MCP modes)
- [ ] Redirect logs to stderr if needed for debugging
- [ ] Route `mcp` subcommand to `mcp::serve()` function
- [ ] Mark `mcp` subcommand as `#[command(hide = true)]` in clap

### 2.2 Connect Command
- [ ] Define `connect` subcommand arguments:
  - [ ] `--name <NAME>` (connection profile name, optional)
  - [ ] `--engine <ENGINE>` (required for new connections)
  - [ ] `--host <HOST>` (for postgres/mysql)
  - [ ] `--port <PORT>` (for postgres/mysql)
  - [ ] `--user <USER>` (for postgres/mysql)
  - [ ] `--password <PASSWORD>` (for postgres/mysql)
  - [ ] `--password-env <VAR>` (use env var instead of plain password)
  - [ ] `--database <DATABASE>` (for postgres/mysql)
  - [ ] `--file <PATH>` (for sqlite)
  - [ ] `--save <local|global>` (where to save config)
- [ ] Implement interactive connection picker (no args):
  - [ ] Display list of existing named connections
  - [ ] Show connection details (engine, host, database)
  - [ ] Include "--- New ---" option to create new connection
  - [ ] Allow selection via numbered input
- [ ] Implement interactive configuration wizard:
  - [ ] Prompt for engine selection (postgres, mysql, sqlite)
  - [ ] Prompt for connection details based on engine
  - [ ] Prompt for connection name
  - [ ] Prompt for save location (local/global)
  - [ ] Use `dialoguer` or `inquire` for TUI prompts
- [ ] Implement non-interactive config creation (with flags):
  - [ ] Validate all required fields present
  - [ ] Create or update named connection
  - [ ] Save to specified location
- [ ] Implement connection validation:
  - [ ] Call `DatabaseEngine::validate_connection()`
  - [ ] Test connectivity before saving
  - [ ] Return connection metadata (version, server info)
- [ ] Implement config persistence:
  - [ ] Save to local (`.plenum/config.json`)
  - [ ] Save to global (`~/.config/plenum/connections.json`)
  - [ ] Update existing connections
  - [ ] Set default connection if first connection
- [ ] Return JSON success/error envelope
- [ ] Do NOT maintain persistent connections (validate then disconnect)

### 2.3 Introspect Command
- [ ] Define `introspect` subcommand arguments:
  - [ ] `--name <NAME>` (use named connection, optional)
  - [ ] Same connection parameters as `connect` (for overrides)
  - [ ] `--schema <SCHEMA>` (optional filter)
- [ ] Implement connection resolution:
  - [ ] Load from config if `--name` provided
  - [ ] Load from default connection if no flags provided
  - [ ] Override config with explicit CLI flags
  - [ ] Error if no connection available
- [ ] Implement schema introspection orchestration:
  - [ ] Build `ConnectionConfig` from resolved connection
  - [ ] Call `DatabaseEngine::introspect()`
- [ ] Return JSON with schema information:
  - [ ] Tables
  - [ ] Columns (name, type, nullable)
  - [ ] Primary keys
  - [ ] Foreign keys
  - [ ] Indexes
- [ ] Include execution metadata

### 2.4 Query Command
- [ ] Define `query` subcommand arguments:
  - [ ] `--name <NAME>` (use named connection, optional)
  - [ ] Same connection parameters as `connect` (for overrides)
  - [ ] `--sql <SQL>` or `--file <PATH>` (required)
  - [ ] `--allow-write` (explicit flag, default: false)
  - [ ] `--allow-ddl` (explicit flag, default: false)
  - [ ] `--max-rows <N>` (optional)
  - [ ] `--timeout-ms <MS>` (optional)
- [ ] Implement connection resolution:
  - [ ] Load from config if `--name` provided
  - [ ] Load from default connection if no flags provided
  - [ ] Override config with explicit CLI flags
  - [ ] Error if no connection available
- [ ] Build `Capabilities` struct from flags:
  - [ ] Read-only by default (no flag needed)
  - [ ] `allow_write` from `--allow-write` flag
  - [ ] `allow_ddl` from `--allow-ddl` flag
  - [ ] `max_rows` from `--max-rows`
  - [ ] `timeout_ms` from `--timeout-ms`
- [ ] Validate capabilities before execution
- [ ] Execute query through engine trait:
  - [ ] Build `ConnectionConfig` from resolved connection
  - [ ] Call `DatabaseEngine::execute()`
- [ ] Return JSON with query results
- [ ] Include execution metadata

---

## Phase 3: PostgreSQL Engine

### 3.1 PostgreSQL Connection
- [ ] Create `src/engine/postgres/mod.rs`
- [ ] Implement `DatabaseEngine` trait for PostgreSQL
- [ ] Implement connection establishment
- [ ] Handle connection errors with proper wrapping
- [ ] Detect and include PostgreSQL version in metadata

### 3.2 PostgreSQL Introspection
- [ ] Query `information_schema.tables`
- [ ] Query `information_schema.columns`
- [ ] Query primary key information
- [ ] Query foreign key information
- [ ] Query index information
- [ ] Format results as `SchemaInfo`
- [ ] Handle PostgreSQL-specific edge cases

### 3.3 PostgreSQL Query Execution
- [ ] Implement query execution with capability checks
- [ ] Parse result sets into JSON-safe format
- [ ] Handle PostgreSQL data types:
  - [ ] Numeric types
  - [ ] String types
  - [ ] Date/time types
  - [ ] Boolean types
  - [ ] NULL values
  - [ ] Arrays (as JSON arrays)
  - [ ] JSON/JSONB (as nested JSON)
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time

### 3.4 PostgreSQL Testing
- [ ] Set up test database (in-memory or docker)
- [ ] Write capability enforcement tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 4: MySQL Engine

### 4.1 MySQL Connection
- [ ] Create `src/engine/mysql/mod.rs`
- [ ] Implement `DatabaseEngine` trait for MySQL
- [ ] Implement connection establishment
- [ ] Detect MySQL version explicitly
- [ ] Handle MariaDB detection and versioning
- [ ] Handle connection errors with proper wrapping

### 4.2 MySQL Introspection
- [ ] Query `information_schema.tables`
- [ ] Query `information_schema.columns`
- [ ] Query primary key information
- [ ] Query foreign key information
- [ ] Query index information
- [ ] Avoid non-standard INFORMATION_SCHEMA extensions
- [ ] Format results as `SchemaInfo`
- [ ] Handle MySQL-specific edge cases
- [ ] Handle storage engine variations

### 4.3 MySQL Query Execution
- [ ] Implement query execution with capability checks
- [ ] Handle implicit commit detection (DDL statements)
- [ ] Parse result sets into JSON-safe format
- [ ] Handle MySQL data types:
  - [ ] Numeric types (INT, DECIMAL, FLOAT, etc.)
  - [ ] String types (VARCHAR, TEXT, CHAR, etc.)
  - [ ] Date/time types (DATE, DATETIME, TIMESTAMP)
  - [ ] Boolean/TINYINT(1)
  - [ ] NULL values
  - [ ] ENUM and SET types
  - [ ] Binary types
  - [ ] JSON type (MySQL 5.7+)
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time
- [ ] Surface version-specific behaviors in metadata

### 4.4 MySQL Testing
- [ ] Set up test database (docker with specific versions)
- [ ] Test against multiple MySQL versions
- [ ] Write capability enforcement tests
- [ ] Write DDL implicit commit tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 5: SQLite Engine

### 5.1 SQLite Connection
- [ ] Create `src/engine/sqlite/mod.rs`
- [ ] Implement `DatabaseEngine` trait for SQLite
- [ ] Implement file-based connection
- [ ] Implement in-memory connection (`:memory:`)
- [ ] Detect SQLite version
- [ ] Handle connection errors with proper wrapping

### 5.2 SQLite Introspection
- [ ] Query `sqlite_master` table
- [ ] Use `PRAGMA table_info()` for column information
- [ ] Use `PRAGMA foreign_key_list()` for foreign keys
- [ ] Use `PRAGMA index_list()` for indexes
- [ ] Format results as `SchemaInfo`
- [ ] Handle SQLite-specific edge cases

### 5.3 SQLite Query Execution
- [ ] Implement query execution with capability checks
- [ ] Parse result sets into JSON-safe format
- [ ] Handle SQLite data types (dynamic typing):
  - [ ] INTEGER
  - [ ] REAL
  - [ ] TEXT
  - [ ] BLOB (as base64 or hex)
  - [ ] NULL
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time

### 5.4 SQLite Testing
- [ ] Set up test database (in-memory)
- [ ] Write capability enforcement tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 6: Integration & Polish

### 6.1 Cross-Engine Testing
- [ ] Create integration test suite
- [ ] Test identical queries across all engines
- [ ] Verify JSON output consistency
- [ ] Test capability enforcement across engines
- [ ] Test error handling across engines
- [ ] Verify no cross-engine behavior leaks

### 6.2 Output Validation
- [ ] Verify all stdout is valid JSON
- [ ] Verify no logs appear on stdout
- [ ] Verify success envelope schema
- [ ] Verify error envelope schema
- [ ] Verify metadata consistency
- [ ] Create JSON schema files for validation

### 6.3 Edge Case Handling
- [ ] Test empty result sets
- [ ] Test very large result sets
- [ ] Test malformed SQL
- [ ] Test connection failures
- [ ] Test timeout scenarios
- [ ] Test max_rows enforcement
- [ ] Test invalid capability combinations
- [ ] Test NULL handling across all engines
- [ ] Test special characters in data

### 6.4 Performance Baseline
- [ ] Benchmark connection time for each engine
- [ ] Benchmark introspection time for each engine
- [ ] Benchmark query execution for each engine
- [ ] Document performance characteristics
- [ ] Identify performance bottlenecks (if any)

### 6.5 Documentation
- [ ] Update README.md with:
  - [ ] Project overview
  - [ ] Installation instructions
  - [ ] Usage examples for each command
  - [ ] Capability model explanation
  - [ ] Error code reference
- [ ] Create EXAMPLES.md with:
  - [ ] Connect examples for each engine
  - [ ] Introspect examples
  - [ ] Query examples (read-only, write, DDL)
  - [ ] Error handling examples
- [ ] Create ARCHITECTURE.md with:
  - [ ] System architecture diagram
  - [ ] Trait hierarchy
  - [ ] Data flow diagrams
  - [ ] Engine isolation explanation

---

## Phase 7: MCP Server

**Note:** MCP architecture research completed in Phase 0.3

### 7.1 MCP Server Setup
- [ ] Create `src/mcp.rs` module
- [ ] Import `rmcp` types:
  - [ ] `use rmcp::{tool, ServerHandler, CallToolResult, Parameters};`
  - [ ] `use rmcp::transport::stdio;`
  - [ ] `use rmcp::model::ServerInfo;`
- [ ] Define `PlenumServer` struct:
  ```rust
  #[derive(Clone)]
  pub struct PlenumServer;
  ```
- [ ] Implement `ServerHandler` trait:
  ```rust
  impl ServerHandler for PlenumServer {
      fn get_info(&self) -> ServerInfo {
          ServerInfo {
              name: "plenum".into(),
              version: env!("CARGO_PKG_VERSION").into(),
          }
      }
  }
  ```
- [ ] Create `serve()` async function to start MCP server:
  ```rust
  pub async fn serve() -> anyhow::Result<()> {
      let server = PlenumServer;
      server.serve(stdio()).await?;
      Ok(())
  }
  ```
- [ ] Wire up `plenum mcp` subcommand in main.rs to call `mcp::serve()`
- [ ] Add `#[tokio::main]` to main function for async support

### 7.2 MCP Tool: connect
- [ ] Define `ConnectRequest` struct with `serde` and `schemars` derives:
  - [ ] `name: Option<String>` - Named connection profile
  - [ ] `engine: String` - Database engine (postgres, mysql, sqlite)
  - [ ] `host: Option<String>` - For postgres/mysql
  - [ ] `port: Option<u16>` - For postgres/mysql
  - [ ] `user: Option<String>` - For postgres/mysql
  - [ ] `password: Option<String>` - For postgres/mysql
  - [ ] `password_env: Option<String>` - Environment variable for password
  - [ ] `database: Option<String>` - For postgres/mysql
  - [ ] `file: Option<PathBuf>` - For sqlite
  - [ ] `save: Option<String>` - Save location (local/global)
- [ ] Implement `#[tool]` method on `PlenumServer`:
  ```rust
  #[tool(description = "Validate and save database connection configuration")]
  async fn connect(
      &self,
      Parameters(request): Parameters<ConnectRequest>,
  ) -> Result<CallToolResult, McpError> {
      // Build ConnectionConfig from request
      // Call crate::execute_connect()
      // Wrap result in SuccessEnvelope
      // Return as MCP tool result
  }
  ```
- [ ] Handle errors and convert to `McpError`
- [ ] Return connection metadata (version, server info) in response

### 7.3 MCP Tool: introspect
- [ ] Define `IntrospectRequest` struct:
  - [ ] `name: Option<String>` - Use named connection
  - [ ] Connection parameters (for overrides)
  - [ ] `schema: Option<String>` - Schema filter
- [ ] Implement `#[tool]` method:
  ```rust
  #[tool(description = "Introspect database schema and return table/column information")]
  async fn introspect(
      &self,
      Parameters(request): Parameters<IntrospectRequest>,
  ) -> Result<CallToolResult, McpError>
  ```
- [ ] Resolve connection from config or explicit parameters
- [ ] Call library function `crate::execute_introspect()`
- [ ] Return schema information (tables, columns, keys, indexes) as MCP tool result
- [ ] Include execution metadata in response

### 7.4 MCP Tool: query
- [ ] Define `QueryRequest` struct:
  - [ ] `name: Option<String>` - Use named connection
  - [ ] Connection parameters (for overrides)
  - [ ] `sql: String` - SQL query to execute (required)
  - [ ] `allow_write: Option<bool>` - Enable write operations (default: false)
  - [ ] `allow_ddl: Option<bool>` - Enable DDL operations (default: false)
  - [ ] `max_rows: Option<usize>` - Limit result set size
  - [ ] `timeout_ms: Option<u64>` - Query timeout in milliseconds
- [ ] Implement `#[tool]` method:
  ```rust
  #[tool(description = "Execute SQL query with capability constraints (read-only by default)")]
  async fn query(
      &self,
      Parameters(request): Parameters<QueryRequest>,
  ) -> Result<CallToolResult, McpError>
  ```
- [ ] Resolve connection from config or explicit parameters
- [ ] Build `Capabilities` struct from request flags
- [ ] Call library function `crate::execute_query()`
- [ ] Return query results with execution metadata
- [ ] Ensure capability violations are caught and returned as errors

### 7.5 Stateless Design Verification
- [ ] Verify `PlenumServer` struct has no state fields
- [ ] Verify each tool invocation is completely independent
- [ ] Verify connections are opened and closed within each tool call
- [ ] Test concurrent tool invocations for thread safety
- [ ] Document that credentials are passed per-invocation (never cached)
- [ ] Ensure no global mutable state anywhere in MCP module

### 7.6 MCP Protocol Testing
- [ ] Test MCP initialization handshake
- [ ] Test `tools/list` returns all three tools with correct schemas
- [ ] Test `tools/call` for `connect` tool:
  - [ ] With valid connection parameters
  - [ ] With invalid parameters (error handling)
  - [ ] With named connection reference
- [ ] Test `tools/call` for `introspect` tool:
  - [ ] With direct connection parameters
  - [ ] With named connection
  - [ ] With schema filter
- [ ] Test `tools/call` for `query` tool:
  - [ ] Read-only query (default)
  - [ ] Write query with `--allow-write`
  - [ ] DDL query with `--allow-ddl`
  - [ ] Capability violation (should fail before execution)
- [ ] Verify tool schemas are correctly generated via `schemars`
- [ ] Test error propagation through MCP protocol
- [ ] Verify JSON output format matches CLI output format
- [ ] Test with actual MCP client (Claude Desktop configuration)
- [ ] Document MCP client configuration in README:
  ```json
  {
    "mcpServers": {
      "plenum": {
        "command": "plenum",
        "args": ["mcp"]
      }
    }
  }
  ```

---

## Phase 8: Security Audit

### 8.1 Capability Enforcement Audit
- [ ] Audit all capability check points
- [ ] Verify checks occur before execution
- [ ] Test capability bypass attempts
- [ ] Verify DDL detection across engines
- [ ] Verify write detection across engines
- [ ] Document capability enforcement guarantees

### 8.2 Security Model Verification
- [ ] Verify capability enforcement prevents unauthorized operations
- [ ] Verify DDL detection catches all DDL statement types
- [ ] Verify write detection catches all write operations
- [ ] Document that SQL injection prevention is the agent's responsibility
- [ ] Document that Plenum passes SQL verbatim to the database
- [ ] Verify Plenum does not modify, sanitize, or interpret SQL content
- [ ] Document security boundaries clearly in README

### 8.3 Credential Security
- [ ] Audit credential handling paths
- [ ] Verify credentials not in logs
- [ ] Verify credentials not in error messages
- [ ] Verify credentials not persisted to disk
- [ ] Document credential security model

### 8.4 Error Information Leakage
- [ ] Review all error messages
- [ ] Ensure no sensitive data in errors
- [ ] Ensure no path information leakage
- [ ] Ensure no credential leakage
- [ ] Verify error messages are agent-appropriate

---

## Phase 9: Release Preparation

### 9.1 Code Quality
- [ ] Run `cargo fmt` on all code
- [ ] Run `cargo clippy` and address all warnings
- [ ] Run `cargo audit` for dependency vulnerabilities
- [ ] Review all TODO/FIXME comments
- [ ] Ensure consistent code style

### 9.2 Testing
- [ ] Run full test suite
- [ ] Verify 100% of critical paths tested
- [ ] Generate code coverage report
- [ ] Document test coverage
- [ ] Fix any flaky tests

### 9.3 Documentation Review
- [ ] Review all markdown documentation
- [ ] Verify code examples work
- [ ] Check for broken links
- [ ] Verify JSON schemas are accurate
- [ ] Update CLAUDE.md if needed

### 9.4 Build & Distribution
- [ ] Create release build configuration
- [ ] Test release builds on:
  - [ ] Linux (x86_64)
  - [ ] macOS (x86_64, ARM64)
  - [ ] Windows (x86_64)
- [ ] Create installation scripts
- [ ] Document system requirements
- [ ] Create distribution packages

### 9.5 CI/CD Pipeline
- [ ] Configure automated testing on PR
- [ ] Configure automated testing on push to main
- [ ] Configure automated builds
- [ ] Configure automated security scanning
- [ ] Document CI/CD pipeline

### 9.6 Version Tagging
- [ ] Choose semantic version (1.0.0)
- [ ] Create git tag
- [ ] Write release notes
- [ ] Publish release artifacts

---

## Phase 10: Post-MVP Considerations

### 10.1 Observability (Future)
- [ ] Consider structured logging to stderr
- [ ] Consider metrics collection
- [ ] Consider tracing integration
- [ ] Document observability strategy

### 10.2 Additional Engines (Future)
- [ ] Evaluate Microsoft SQL Server support
- [ ] Evaluate MongoDB support (if relevant)
- [ ] Evaluate other engines as needed
- [ ] Document engine addition process

### 10.3 Performance Optimization (Future)
- [ ] Profile hot paths
- [ ] Optimize JSON serialization
- [ ] Optimize query parsing
- [ ] Consider zero-copy optimizations

### 10.4 Extended Capabilities (Future)
- [ ] Consider prepared statement support
- [ ] Consider transaction management
- [ ] Consider batch query execution
- [ ] Document extension points

---

## Dependencies Checklist

### Core Dependencies
- [ ] `clap` - CLI framework (with derive feature)
- [ ] `serde` - Serialization framework (with derive feature)
- [ ] `serde_json` - JSON serialization
- [ ] `thiserror` - Error handling (structured errors)
- [ ] `anyhow` - Error context and convenience
- [ ] `tokio` - Async runtime (required by rmcp and async database drivers)

### MCP Server Dependencies
- [ ] `rmcp` - Official Rust MCP SDK
- [ ] `schemars` - JSON schema generation for MCP tool definitions

### Database Drivers (native drivers only, NO sqlx)
- [ ] `tokio-postgres` - PostgreSQL native driver
- [ ] `mysql_async` - MySQL native driver
- [ ] `rusqlite` - SQLite native driver

### Configuration Management
- [ ] `dialoguer` or `inquire` - Interactive prompts for connection setup
- [ ] `dirs` - Cross-platform configuration directory paths

### Testing
- [ ] `criterion` - Benchmarking
- [ ] `pretty_assertions` - Better test output
- [ ] `insta` - Snapshot testing for JSON output validation

---

## Risk Mitigation Plan

### Risk 1: Complex Capability Enforcement
**Mitigation:**
- [ ] Start with simple SELECT/INSERT/UPDATE/DELETE categorization
- [ ] Add DDL detection incrementally
- [ ] Test extensively with real-world queries
- [ ] Document edge cases

### Risk 2: Database Driver Compatibility
**Mitigation:**
- [ ] Use native drivers (NOT sqlx) to maximize engine isolation
- [ ] Test with multiple database versions for each engine
- [ ] Document version requirements clearly
- [ ] Isolate driver-specific code in engine modules
- [ ] Each engine handles its own quirks independently
- [ ] No shared database abstraction layer

### Risk 3: MySQL Implicit Commits
**Mitigation:**
- [ ] Maintain explicit list of DDL statements
- [ ] Test with comprehensive DDL statement list
- [ ] Document MySQL-specific behaviors
- [ ] Surface warnings in metadata

### Risk 4: JSON Output Stability
**Mitigation:**
- [ ] Use snapshot tests
- [ ] Version output schema
- [ ] Document breaking changes
- [ ] Consider schema versioning in output

### Risk 5: MCP Integration Complexity
**Mitigation:**
- [ ] Use proven pattern from reflex-search project
- [ ] Use standard `rmcp` SDK (official tooling, not custom implementation)
- [ ] Keep MCP layer thin (just tool definitions wrapping library functions)
- [ ] Maintain library functions as single source of truth for both CLI and MCP
- [ ] Test MCP and CLI independently with identical assertions
- [ ] Document integration boundaries clearly
- [ ] Verify determinism: same library call should produce same result from CLI or MCP

---

## Success Metrics

### Functionality
- [ ] All three commands work for all three engines
- [ ] 100% capability enforcement coverage
- [ ] Zero capability bypasses

### Quality
- [ ] Test coverage > 80%
- [ ] Zero clippy warnings
- [ ] Zero security vulnerabilities
- [ ] All tests deterministic

### Performance
- [ ] Connection time < 1 second
- [ ] Introspection time < 5 seconds for typical schema
- [ ] Query execution overhead < 10ms

### Documentation
- [ ] All commands documented with examples
- [ ] All error codes documented
- [ ] Architecture documented
- [ ] Contribution guide complete

---

## Timeline Estimates

**Note:** These are rough estimates and will vary based on team size and experience.

- **Phase 0-1:** 1-2 weeks (Foundation)
- **Phase 2:** 1 week (CLI)
- **Phase 3:** 1-2 weeks (PostgreSQL)
- **Phase 4:** 1-2 weeks (MySQL)
- **Phase 5:** 1 week (SQLite)
- **Phase 6:** 1-2 weeks (Integration)
- **Phase 7:** 1-2 weeks (MCP Server)
- **Phase 8:** 1 week (Security Audit)
- **Phase 9:** 1 week (Release Prep)

**Total Estimated Timeline:** 10-14 weeks

---

## Review Gates

Each phase must pass review before proceeding:

### Phase Gate Checklist
- [ ] All items in phase completed
- [ ] Tests passing
- [ ] Code reviewed
- [ ] Documentation updated
- [ ] No known critical bugs
- [ ] Performance acceptable
- [ ] Security reviewed

---

## Appendix: Command Reference

### Connect Command
```bash
plenum connect --engine postgres --host localhost --port 5432 \
  --user admin --password secret --database mydb
```

### Introspect Command
```bash
plenum introspect --engine mysql --host localhost --port 3306 \
  --user admin --password secret --database mydb --schema public
```

### Query Command (Read-Only)
```bash
plenum query --engine sqlite --file /path/to/db.sqlite \
  --sql "SELECT * FROM users WHERE id = 1"
```

### Query Command (With Write)
```bash
plenum query --engine postgres --host localhost --port 5432 \
  --user admin --password secret --database mydb \
  --sql "UPDATE users SET name = 'John' WHERE id = 1" \
  --allow-write --timeout-ms 5000
```

### Query Command (With DDL)
```bash
plenum query --engine mysql --host localhost --port 3306 \
  --user admin --password secret --database mydb \
  --sql "CREATE TABLE test (id INT PRIMARY KEY)" \
  --allow-ddl
```

---

## Final Notes

This project plan represents the complete path from empty repository to production-ready MVP. Each phase builds on the previous, maintaining the core principles defined in CLAUDE.md throughout.

The plan prioritizes:
1. **Agent safety** - Capability enforcement is paramount
2. **Determinism** - Identical inputs produce identical outputs
3. **Explicitness** - No implicit behavior
4. **Simplicity** - Minimal abstractions
5. **Testability** - Comprehensive, deterministic tests

Remember the guiding question for all implementation work:

> **"Does this make autonomous agents safer, more deterministic, or more constrained?"**

If the answer is no, it does not belong in Plenum.
