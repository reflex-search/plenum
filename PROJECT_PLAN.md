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
- [x] Initialize Rust project structure ✅
  - [x] Run `cargo init --name plenum --lib`
  - [x] Configure Cargo.toml with both binary and library targets:
    ```toml
    [lib]
    name = "plenum"
    path = "src/lib.rs"

    [[bin]]
    name = "plenum"
    path = "src/main.rs"
    ```
  - [x] Configure Cargo.toml with project metadata (version, authors, edition, license)
  - [x] Set up `src/lib.rs` to export internal API
  - [x] Set up `src/main.rs` as CLI entry point
- [x] Configure `.gitignore` for Rust builds ✅
  - [x] Add `/target`
  - [x] Add `/.idea` and other IDE-specific directories
  - [x] Note: Do NOT add `Cargo.lock` (should be committed for binary projects)
- [x] Add LICENSE file (MIT OR Apache-2.0) ✅
- [x] Expand README.md with project description and build instructions ✅

### 0.2 Development Environment
- [x] Define Rust toolchain version (stable/nightly) ✅
- [x] Configure rustfmt.toml for code formatting ✅
- [x] Configure clippy rules for linting ✅
- [x] Set up CI/CD pipeline configuration (GitHub Actions) ✅
- [x] Document build/test commands ✅

### 0.3 Dependency Assessment

**CRITICAL: MCP Architecture Research (moved from Phase 7.1)**
- [x] Research MCP (Model Context Protocol) implementation in Rust ✅
  - [x] Evaluate `rmcp` crate (official Rust SDK for MCP)
  - [x] Verify stdio transport compatibility with stateless design
  - [x] Confirm JSON-RPC protocol handling via `rmcp`
  - [x] Test `#[tool]` macro pattern compatibility with our architecture
  - [x] Verify async requirements (tokio integration)
  - [x] Review reflex-search implementation pattern: https://github.com/reflex-search/reflex
- [x] Document MCP architecture decision: ✅
  - [x] **Decision:** Single crate with `plenum mcp` subcommand (not workspace)
  - [x] **Pattern:** Follow reflex-search implementation pattern
  - [x] **Rationale:** Simpler structure, proven pattern, uses standard tooling
  - [x] **Key principle:** Both CLI and MCP call same internal library functions
- [x] Select MCP dependencies: ✅
  - [x] `rmcp` - Official Rust MCP SDK
  - [x] `tokio` - Async runtime (required by rmcp)
  - [x] `schemars` - JSON schema generation for MCP tool definitions

**Database Driver Selection (MUST use native drivers)**
- [x] Research and select database driver crates: ✅
  - [x] PostgreSQL: `tokio-postgres` (native driver, NOT sqlx)
  - [x] MySQL: `mysql_async` (native driver, NOT sqlx)
  - [x] SQLite: `rusqlite` (native driver, NOT sqlx)
- [x] Document rationale for native drivers: ✅
  - [x] Maximum isolation between engines
  - [x] Vendor-specific behavior preserved
  - [x] No risk of abstraction leakage
  - [x] Each engine handles its own quirks independently
  - [x] Aligns with "no compatibility layers" principle (CLAUDE.md)

**Core Libraries**
- [x] Select JSON serialization library: `serde_json` ✅
- [x] Select CLI framework: `clap` ✅
- [x] Select error handling: `thiserror` and `anyhow` ✅
- [x] Select configuration management libraries: ✅
  - [x] Interactive prompts: `dialoguer` or `inquire`
  - [x] Cross-platform config paths: `dirs`
  - [x] Config format: JSON via `serde_json`
- [x] Document dependency rationale in RESEARCH.md ✅

---

## Phase 1: Core Architecture

### 1.1 Define Core Traits
- [x] Create `src/engine/mod.rs` with trait definitions ✅
- [x] Define `DatabaseEngine` trait (stateless design) ✅
  - [x] `fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo>` ✅
  - [x] `fn introspect(config: &ConnectionConfig, schema_filter: Option<&str>) -> Result<SchemaInfo>` ✅
  - [x] `fn execute(config: &ConnectionConfig, query: &str, caps: &Capabilities) -> Result<QueryResult>` ✅
- [x] Define `Capabilities` struct ✅
  - [x] `allow_write: bool` (default: false) ✅
  - [x] `allow_ddl: bool` (default: false) ✅
  - [x] `max_rows: Option<usize>` ✅
  - [x] `timeout_ms: Option<u64>` ✅
- [x] Define `ConnectionConfig` struct ✅
  - [x] `engine: DatabaseType` ✅
  - [x] `host: Option<String>` (for postgres/mysql) ✅
  - [x] `port: Option<u16>` (for postgres/mysql) ✅
  - [x] `user: Option<String>` (for postgres/mysql) ✅
  - [x] `password: Option<String>` (for postgres/mysql) ✅
  - [x] `database: Option<String>` (for postgres/mysql) ✅
  - [x] `file: Option<PathBuf>` (for sqlite) ✅
- [x] Define `ConnectionInfo` struct (returned by validate_connection) ✅
  - [x] `database_version: String` ✅
  - [x] `server_info: String` ✅
  - [x] `connected_database: String` ✅
  - [x] `user: String` ✅
- [x] Define `SchemaInfo` struct ✅
- [x] Define `QueryResult` struct ✅

### 1.2 Define Output Envelope Types
- [x] Create `src/output/mod.rs` ✅
- [x] Define `SuccessEnvelope<T>` struct ✅
  - [x] `ok: bool` (always true) ✅
  - [x] `engine: String` ✅
  - [x] `command: String` ✅
  - [x] `data: T` ✅
  - [x] `meta: Metadata` ✅
- [x] Define `ErrorEnvelope` struct ✅
  - [x] `ok: bool` (always false) ✅
  - [x] `engine: String` ✅
  - [x] `command: String` ✅
  - [x] `error: ErrorInfo` ✅
- [x] Define `ErrorInfo` struct ✅
  - [x] `code: String` ✅
  - [x] `message: String` ✅
- [x] Define `Metadata` struct ✅
  - [x] `execution_ms: u64` ✅
  - [x] `rows_returned: Option<usize>` ✅
- [x] Implement `Serialize` for all envelope types ✅

### 1.3 Error Handling Infrastructure
- [x] Create `src/error/mod.rs` ✅
- [x] Define `PlenumError` enum with variants: ✅
  - [x] `CapabilityViolation(String)` ✅
  - [x] `ConnectionFailed(String)` ✅
  - [x] `QueryFailed(String)` ✅
  - [x] `InvalidInput(String)` ✅
  - [x] `EngineError { engine: String, detail: String }` ✅
- [x] Implement error code mapping ✅
- [x] Implement conversion to `ErrorEnvelope` ✅
- [x] Ensure no panics across public boundaries ✅

### 1.4 Capability Validation
- [x] Create `src/capability/mod.rs` ✅
- [x] Implement capability validator ✅
- [x] **SQL Categorization Strategy: Regex-based with engine-specific implementations** ✅
  - [x] **Rationale:** Simplest explicit implementation, no external dependencies, respects vendor SQL differences ✅
  - [x] **Pattern:** Each engine implements its own `categorize_query(sql: &str) -> Result<QueryCategory>` logic ✅
  - [x] **No shared SQL helpers across engines** (aligns with CLAUDE.md principle) ✅
- [x] Define SQL statement categorization: ✅
  - [x] Read-only: SELECT, WITH ... SELECT (CTEs) ✅
  - [x] Write: INSERT, UPDATE, DELETE, CALL/EXEC (stored procedures) ✅
  - [x] DDL: CREATE, DROP, ALTER, TRUNCATE, RENAME ✅
  - [x] Transaction control: BEGIN, COMMIT, ROLLBACK (treat as read-only) ✅
- [x] Implement SQL pre-processing (before categorization): ✅
  - [x] Trim leading/trailing whitespace ✅
  - [x] Strip SQL comments: `--` line comments and `/* */` block comments ✅
  - [x] Normalize to uppercase for pattern matching ✅
  - [x] **Detect multi-statement queries** (contains `;` separators) ✅
  - [x] **Reject multi-statement queries in MVP** (safest approach, can relax post-MVP) ✅
- [x] Implement engine-specific categorization: ✅
  - [x] PostgreSQL: Standard SQL categorization ✅
  - [x] MySQL: Include implicit commit DDL list (CREATE/ALTER/DROP/TRUNCATE/RENAME/LOCK TABLES) ✅
  - [x] SQLite: SQLite-specific DDL handling ✅
- [x] Handle edge cases: ✅
  - [x] **EXPLAIN queries**: Strip EXPLAIN prefix, categorize underlying statement ✅
  - [x] **EXPLAIN ANALYZE**: Categorize underlying statement (executes in PostgreSQL) ✅
  - [x] **CTEs (WITH)**: Match final statement type (e.g., `WITH ... SELECT` → read-only) ✅
  - [x] **Stored procedures (CALL/EXEC)**: Treat as write (conservative, procedures can do anything) ✅
  - [x] **Transaction control (BEGIN/COMMIT/ROLLBACK)**: Treat as read-only (no-op without write capability) ✅
  - [x] **Unknown statement types**: Treat as DDL (fail-safe, most restrictive) ✅
  - [x] **Empty queries**: Return error ✅
  - [x] **Parsing errors**: Return error ✅
- [x] Implement capability hierarchy: ✅
  - [x] **DDL implies write**: If `allow_ddl = true`, treat `allow_write` as true ✅
  - [x] **Write does NOT imply DDL**: `allow_write` alone cannot execute DDL ✅
  - [x] Read-only is default (both `allow_write` and `allow_ddl` are false) ✅
  - [x] Rationale: DDL operations are inherently write operations (more dangerous) ✅
- [x] Implement pre-execution capability checks: ✅
  - [x] DDL queries require `allow_ddl = true` (explicit flag required) ✅
  - [x] Write queries require `allow_write = true` OR `allow_ddl = true` ✅
  - [x] Read-only queries always permitted ✅
- [x] Handle MySQL implicit commit cases: ✅
  - [x] Maintain explicit list of DDL statements that cause implicit commit ✅
  - [x] Document in MySQL engine module ✅
  - [x] Surface in error messages if needed ✅
- [x] Add capability validation unit tests: ✅
  - [x] **Comprehensive edge case matrix per engine** ✅
  - [x] Comment variations (`--`, `/* */`, mixed) ✅
  - [x] Whitespace variations (leading, trailing, mixed) ✅
  - [x] Case sensitivity (lowercase, uppercase, mixed) ✅
  - [x] CTE queries (`WITH ... SELECT`, `WITH ... INSERT`) ✅
  - [x] EXPLAIN queries (with and without ANALYZE) ✅
  - [x] Transaction control (BEGIN, COMMIT, ROLLBACK) ✅
  - [x] Multi-statement detection (should reject) ✅
  - [x] Unknown statement types (should default to DDL) ✅
  - [x] Empty queries (should error) ✅
  - [x] Stored procedure calls (CALL, EXEC) ✅
  - [x] Engine-specific edge cases (PostgreSQL/MySQL/SQLite quirks) ✅

### 1.5 Configuration Management
- [x] Create `src/config/mod.rs` ✅
- [x] Define configuration file formats: ✅
  - [x] Local: `.plenum/config.json` (team-shareable) ✅
  - [x] Global: `~/.config/plenum/connections.json` (per-user) ✅
- [x] Define `ConnectionRegistry` for loading/saving configs ✅
- [x] Implement config file structure: ✅
  - [x] Named connection profiles ✅
  - [x] Default connection selection ✅
  - [x] Per-project scoping for global config (keyed by working directory) ✅
- [x] Implement config loading with precedence: ✅
  - [x] Explicit CLI flags (highest priority) ✅
  - [x] Local config (`.plenum/config.json`) ✅
  - [x] Global config (`~/.config/plenum/connections.json`) ✅
- [x] Support environment variable substitution: ✅
  - [x] `password_env` field for credential security ✅
  - [x] Resolve env vars at runtime ✅
- [x] Implement config saving: ✅
  - [x] Save to local vs global locations ✅
  - [x] Update existing named connections ✅
  - [x] Create new named connections ✅
- [x] Add config validation: ✅
  - [x] Required fields per engine type ✅
  - [x] Connection name uniqueness ✅
  - [x] File permissions checks ✅
- [x] Add config migration/versioning support ✅
- [x] Implement connection resolution logic: ✅
  - [x] By name (`--name prod`) ✅
  - [x] Runtime parameter overrides ✅
  - [x] Fallback to default connection ✅

### 1.6 Library Module Structure
- [x] Create `src/lib.rs` with public API exports ✅
- [x] **IMPORTANT:** Design all modules for reuse by both CLI and MCP ✅
- [x] Export core types for both CLI and MCP use: ✅
  - [x] `pub use engine::{DatabaseEngine, ConnectionConfig, ConnectionInfo, SchemaInfo, QueryResult};` ✅
  - [x] `pub use capability::Capabilities;` ✅
  - [x] `pub use output::{SuccessEnvelope, ErrorEnvelope};` ✅
  - [x] `pub use config::{resolve_connection, save_connection};` ✅
  - [x] `pub use error::PlenumError;` ✅
- [x] Design internal functions to be CLI/MCP agnostic: ✅
  - [x] `execute_connect(config: ConnectionConfig) -> Result<ConnectionInfo>` ✅
  - [x] `execute_introspect(config: ConnectionConfig, filter: Option<&str>) -> Result<SchemaInfo>` ✅
  - [x] `execute_query(config: ConnectionConfig, sql: &str, caps: Capabilities) -> Result<QueryResult>` ✅
- [x] Ensure all business logic lives in library modules, not in CLI/MCP wrappers ✅
- [x] CLI and MCP should be thin wrappers calling library functions ✅
- [x] Document public API in module-level docs ✅

---

## Phase 2: CLI Foundation

### 2.1 CLI Structure
- [x] Create `src/main.rs` with CLI entry point ✅
- [x] Set up `clap` with four subcommands: ✅
  - [x] `connect` - Connection configuration management ✅
  - [x] `introspect` - Schema introspection ✅
  - [x] `query` - Constrained query execution ✅
  - [x] `mcp` - MCP server (hidden from help, for AI agent integration) ✅
- [x] Define common flags for connection parameters: ✅
  - [x] `--engine <postgres|mysql|sqlite>` ✅
  - [x] `--host`, `--port`, `--user`, `--password`, `--database`, `--file` ✅
- [x] Ensure stdout is JSON-only (for both CLI and MCP modes) ✅
- [x] Redirect logs to stderr if needed for debugging ✅
- [x] Route `mcp` subcommand to `mcp::serve()` function ✅
- [x] Mark `mcp` subcommand as `#[command(hide = true)]` in clap ✅

### 2.2 Connect Command
- [x] Define `connect` subcommand arguments: ✅
  - [x] `--name <NAME>` (connection profile name, optional) ✅
  - [x] `--engine <ENGINE>` (required for new connections) ✅
  - [x] `--host <HOST>` (for postgres/mysql) ✅
  - [x] `--port <PORT>` (for postgres/mysql) ✅
  - [x] `--user <USER>` (for postgres/mysql) ✅
  - [x] `--password <PASSWORD>` (for postgres/mysql) ✅
  - [x] `--password-env <VAR>` (use env var instead of plain password) ✅
  - [x] `--database <DATABASE>` (for postgres/mysql) ✅
  - [x] `--file <PATH>` (for sqlite) ✅
  - [x] `--save <local|global>` (where to save config) ✅
- [x] Implement interactive connection picker (no args): ✅
  - [x] Display list of existing named connections ✅
  - [x] Show connection details (engine, host, database) ✅
  - [x] Include "--- New ---" option to create new connection ✅
  - [x] Allow selection via numbered input ✅
- [x] Implement interactive configuration wizard: ✅
  - [x] Prompt for engine selection (postgres, mysql, sqlite) ✅
  - [x] Prompt for connection details based on engine ✅
  - [x] Prompt for connection name ✅
  - [x] Prompt for save location (local/global) ✅
  - [x] Use `dialoguer` or `inquire` for TUI prompts ✅
- [x] Implement non-interactive config creation (with flags): ✅
  - [x] Validate all required fields present ✅
  - [x] Create or update named connection ✅
  - [x] Save to specified location ✅
- [x] Implement connection validation: ✅ Complete for all engines
  - [x] Call `DatabaseEngine::validate_connection()` for SQLite ✅ (Phase 3)
  - [x] Call `DatabaseEngine::validate_connection()` for Postgres ✅ (Phase 4)
  - [x] Call `DatabaseEngine::validate_connection()` for MySQL ✅ (Phase 5)
  - [x] Test connectivity before saving (all engines) ✅
  - [x] Return connection metadata (version, server info) ✅
- [x] Implement config persistence: ✅
  - [x] Save to local (`.plenum/config.json`) ✅
  - [x] Save to global (`~/.config/plenum/connections.json`) ✅
  - [x] Update existing connections ✅
  - [x] Set default connection if first connection ✅
- [x] Return JSON success/error envelope ✅
- [x] Do NOT maintain persistent connections (validate then disconnect) ✅

### 2.3 Introspect Command
- [x] Define `introspect` subcommand arguments: ✅
  - [x] `--name <NAME>` (use named connection, optional) ✅
  - [x] Same connection parameters as `connect` (for overrides) ✅
  - [x] `--schema <SCHEMA>` (optional filter) ✅
- [x] Implement connection resolution: ✅
  - [x] Load from config if `--name` provided ✅
  - [x] Load from default connection if no flags provided ✅
  - [x] Override config with explicit CLI flags ✅
  - [x] Error if no connection available ✅
- [x] Implement schema introspection orchestration: ✅ Complete for all engines
  - [x] Build `ConnectionConfig` from resolved connection ✅
  - [x] Call `DatabaseEngine::introspect()` for SQLite ✅ (Phase 3)
  - [x] Call `DatabaseEngine::introspect()` for Postgres ✅ (Phase 4)
  - [x] Call `DatabaseEngine::introspect()` for MySQL ✅ (Phase 5)
- [x] Return JSON with schema information: ✅ Complete for all engines
  - [x] Tables (all engines) ✅
  - [x] Columns (name, type, nullable) (all engines) ✅
  - [x] Primary keys (all engines) ✅
  - [x] Foreign keys (all engines) ✅
  - [x] Indexes (all engines) ✅
- [x] Include execution metadata ✅

### 2.4 Query Command
- [x] Define `query` subcommand arguments: ✅
  - [x] `--name <NAME>` (use named connection, optional) ✅
  - [x] Same connection parameters as `connect` (for overrides) ✅
  - [x] `--sql <SQL>` or `--sql-file <PATH>` (required) ✅
  - [x] `--allow-write` (explicit flag, default: false) ✅
  - [x] `--allow-ddl` (explicit flag, default: false) ✅
  - [x] `--max-rows <N>` (optional) ✅
  - [x] `--timeout-ms <MS>` (optional) ✅
- [x] Implement connection resolution: ✅
  - [x] Load from config if `--name` provided ✅
  - [x] Load from default connection if no flags provided ✅
  - [x] Override config with explicit CLI flags ✅
  - [x] Error if no connection available ✅
- [x] Build `Capabilities` struct from flags: ✅
  - [x] Read-only by default (no flag needed) ✅
  - [x] `allow_write` from `--allow-write` flag ✅
  - [x] `allow_ddl` from `--allow-ddl` flag ✅
  - [x] `max_rows` from `--max-rows` ✅
  - [x] `timeout_ms` from `--timeout-ms` ✅
- [x] Validate capabilities before execution ✅
- [x] Execute query through engine trait: ✅ Complete for all engines
  - [x] Build `ConnectionConfig` from resolved connection ✅
  - [x] Call `DatabaseEngine::execute()` for SQLite ✅ (Phase 3)
  - [x] Call `DatabaseEngine::execute()` for Postgres ✅ (Phase 4)
  - [x] Call `DatabaseEngine::execute()` for MySQL ✅ (Phase 5)
- [x] Return JSON with query results ✅ Complete for all engines
- [x] Include execution metadata ✅

---

## Phase 3: SQLite Engine ✅ COMPLETE

**Note:** SQLite is implemented first (before PostgreSQL and MySQL) because:
- **No external dependencies**: File-based, no database server needed
- **Synchronous driver**: Simpler than async drivers (validates trait design)
- **Easy testing**: In-memory databases (`:memory:`) for fast, isolated tests
- **Fastest development cycle**: Immediate feedback without setup complexity

### 3.1 SQLite Connection
- [x] Create `src/engine/sqlite/mod.rs` ✅
- [x] Implement `DatabaseEngine` trait for SQLite ✅
- [x] Implement file-based connection ✅
- [x] Implement in-memory connection (`:memory:`) ✅
- [x] Detect SQLite version ✅
- [x] Handle connection errors with proper wrapping ✅

### 3.2 SQLite Introspection
- [x] Query `sqlite_master` table ✅
- [x] Use `PRAGMA table_info()` for column information ✅
- [x] Use `PRAGMA foreign_key_list()` for foreign keys ✅
- [x] Use `PRAGMA index_list()` for indexes ✅
- [x] Format results as `SchemaInfo` ✅
- [x] Handle SQLite-specific edge cases ✅

### 3.3 SQLite Query Execution
- [x] Implement query execution with capability checks ✅
- [x] Parse result sets into JSON-safe format ✅
- [x] Handle SQLite data types (dynamic typing): ✅
  - [x] INTEGER ✅
  - [x] REAL ✅
  - [x] TEXT ✅
  - [x] BLOB (as base64) ✅
  - [x] NULL ✅
- [x] Implement timeout enforcement (via busy_timeout) ✅
- [x] Implement row limit enforcement ✅
- [x] Track execution time ✅

### 3.4 SQLite Testing
- [x] Set up test database (in-memory) ✅
- [x] Write capability enforcement tests ✅
- [x] Write introspection tests ✅
- [x] Write query execution tests ✅
- [x] Write error handling tests ✅
- [x] All 16 SQLite-specific tests passing ✅

---

## Phase 4: PostgreSQL Engine ✅ COMPLETE

### 4.1 PostgreSQL Connection
- [x] Create `src/engine/postgres/mod.rs` ✅
- [x] Implement `DatabaseEngine` trait for PostgreSQL ✅
- [x] Implement connection establishment ✅
- [x] Handle connection errors with proper wrapping ✅
- [x] Detect and include PostgreSQL version in metadata ✅

### 4.2 PostgreSQL Introspection
- [x] Query `information_schema.tables` ✅
- [x] Query `information_schema.columns` ✅
- [x] Query primary key information ✅
- [x] Query foreign key information ✅
- [x] Query index information ✅
- [x] Format results as `SchemaInfo` ✅
- [x] Handle PostgreSQL-specific edge cases ✅

### 4.3 PostgreSQL Query Execution
- [x] Implement query execution with capability checks ✅
- [x] Parse result sets into JSON-safe format ✅
- [x] Handle PostgreSQL data types: ✅
  - [x] Numeric types ✅
  - [x] String types ✅
  - [x] Date/time types ✅
  - [x] Boolean types ✅
  - [x] NULL values ✅
  - [x] Arrays (as JSON arrays) ✅
  - [x] JSON/JSONB (as nested JSON) ✅
- [x] Implement timeout enforcement ✅
- [x] Implement row limit enforcement ✅
- [x] Track execution time ✅

### 4.4 PostgreSQL Testing
- [x] Set up test database (integration tests with `#[ignore]` attribute) ✅
- [x] Write capability enforcement tests ✅
- [x] Write introspection tests ✅
- [x] Write query execution tests ✅
- [x] Write error handling tests ✅
- [x] Write JSON output snapshot tests (11 tests total) ✅

---

## Phase 5: MySQL Engine ✅ COMPLETE

### 5.1 MySQL Connection
- [x] Create `src/engine/mysql/mod.rs` ✅
- [x] Implement `DatabaseEngine` trait for MySQL ✅
- [x] Implement connection establishment ✅
- [x] Detect MySQL version explicitly ✅
- [x] Handle MariaDB detection and versioning ✅
- [x] Handle connection errors with proper wrapping ✅

### 5.2 MySQL Introspection
- [x] Query `information_schema.tables` ✅
- [x] Query `information_schema.columns` ✅
- [x] Query primary key information ✅
- [x] Query foreign key information ✅
- [x] Query index information ✅
- [x] Avoid non-standard INFORMATION_SCHEMA extensions ✅
- [x] Format results as `SchemaInfo` ✅
- [x] Handle MySQL-specific edge cases ✅
- [x] Handle storage engine variations ✅

### 5.3 MySQL Query Execution
- [x] Implement query execution with capability checks ✅
- [x] Handle implicit commit detection (DDL statements) ✅
- [x] Parse result sets into JSON-safe format ✅
- [x] Handle MySQL data types: ✅
  - [x] Numeric types (INT, DECIMAL, FLOAT, etc.) ✅
  - [x] String types (VARCHAR, TEXT, CHAR, etc.) ✅
  - [x] Date/time types (DATE, DATETIME, TIMESTAMP) ✅
  - [x] Boolean/TINYINT(1) ✅
  - [x] NULL values ✅
  - [x] ENUM and SET types ✅
  - [x] Binary types ✅
  - [x] JSON type (MySQL 5.7+) - handled as Bytes/String ✅
- [x] Implement timeout enforcement ✅
- [x] Implement row limit enforcement ✅
- [x] Track execution time ✅
- [x] Surface version-specific behaviors in metadata ✅

### 5.4 MySQL Testing
- [x] Set up test structure (integration tests with #[ignore] attribute) ✅
- [x] Write basic unit tests (version parsing, config validation) ✅
- [x] Write capability enforcement tests (via shared capability module) ✅
- [x] DDL implicit commit handled via capability module ✅
- [x] Introspection tests (marked #[ignore], require MySQL instance) ✅
- [x] Query execution tests (marked #[ignore], require MySQL instance) ✅
- [x] Error handling tests ✅
- [x] Integration tests follow PostgreSQL pattern ✅

---

## Phase 6: Integration & Polish ✅ COMPLETE

**Summary:**
- 102 tests passing (16 integration + 11 output validation + 12 edge cases + 63 unit tests)
- 7 benchmarks implemented (connection, introspection, query execution)
- Comprehensive documentation (README.md, EXAMPLES.md, ARCHITECTURE.md)
- JSON Schema validation files created
- Cross-engine consistency verified

### 6.1 Cross-Engine Testing ✅
- [x] Create integration test suite ✅ (tests/integration_tests.rs)
- [x] Test identical queries across all engines ✅ (16 tests)
- [x] Verify JSON output consistency ✅
- [x] Test capability enforcement across engines ✅
- [x] Test error handling across engines ✅
- [x] Verify no cross-engine behavior leaks ✅

### 6.2 Output Validation ✅
- [x] Verify all stdout is valid JSON ✅
- [x] Verify no logs appear on stdout ✅
- [x] Verify success envelope schema ✅
- [x] Verify error envelope schema ✅
- [x] Verify metadata consistency ✅
- [x] Create JSON schema files for validation ✅ (schemas/success_envelope.json, schemas/error_envelope.json)

### 6.3 Edge Case Handling ✅
- [x] Test empty result sets ✅
- [x] Test very large result sets ✅ (5000+ rows)
- [x] Test malformed SQL ✅
- [x] Test connection failures ✅
- [x] Test timeout scenarios ✅
- [x] Test max_rows enforcement ✅
- [x] Test invalid capability combinations ✅
- [x] Test NULL handling across all engines ✅
- [x] Test special characters in data ✅ (Unicode, emoji, SQL injection patterns)

### 6.4 Performance Baseline ✅
- [x] Benchmark connection time for each engine ✅ (benches/connection.rs)
- [x] Benchmark introspection time for each engine ✅ (benches/introspection.rs)
- [x] Benchmark query execution for each engine ✅ (benches/query.rs)
- [x] Document performance characteristics ✅ (ARCHITECTURE.md, EXAMPLES.md)
- [x] Identify performance bottlenecks (if any) ✅ (Documented in ARCHITECTURE.md)

### 6.5 Documentation ✅
- [x] Update README.md with: ✅
  - [x] Project overview ✅
  - [x] Installation instructions ✅
  - [x] Usage examples for each command ✅
  - [x] Capability model explanation ✅
  - [x] Error code reference ✅
- [x] Create EXAMPLES.md with: ✅
  - [x] Connect examples for each engine ✅
  - [x] Introspect examples ✅
  - [x] Query examples (read-only, write, DDL) ✅
  - [x] Error handling examples ✅
- [x] Create ARCHITECTURE.md with: ✅
  - [x] System architecture diagram ✅
  - [x] Trait hierarchy ✅
  - [x] Data flow diagrams ✅
  - [x] Engine isolation explanation ✅

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
- [x] `clap` - CLI framework (with derive feature) ✅
- [x] `serde` - Serialization framework (with derive feature) ✅
- [x] `serde_json` - JSON serialization ✅
- [x] `thiserror` - Error handling (structured errors) ✅
- [x] `anyhow` - Error context and convenience ✅
- [x] `tokio` - Async runtime (required by rmcp and async database drivers) ✅

### MCP Server Dependencies
- [x] `rmcp` - Official Rust MCP SDK ✅
- [x] `schemars` - JSON schema generation for MCP tool definitions ✅

### Database Drivers (native drivers only, NO sqlx)
- [x] `tokio-postgres` - PostgreSQL native driver (optional feature) ✅
- [x] `mysql_async` - MySQL native driver (optional feature) ✅
- [x] `rusqlite` - SQLite native driver (optional feature) ✅

### Configuration Management
- [x] `dialoguer` - Interactive prompts for connection setup ✅
- [x] `dirs` - Cross-platform configuration directory paths ✅

### Testing
- [x] `criterion` - Benchmarking ✅
- [x] `pretty_assertions` - Better test output ✅
- [x] `insta` - Snapshot testing for JSON output validation ✅

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
- **Phase 3:** 3-5 days (SQLite) ⚡ Fastest implementation
- **Phase 4:** 1-2 weeks (PostgreSQL)
- **Phase 5:** 1-2 weeks (MySQL)
- **Phase 6:** 1-2 weeks (Integration)
- **Phase 7:** 1-2 weeks (MCP Server)
- **Phase 8:** 1 week (Security Audit)
- **Phase 9:** 1 week (Release Prep)

**Total Estimated Timeline:** 9-14 weeks

**Phase 3 Rationale:** SQLite implementation is faster because it's synchronous (no async complexity), requires no external setup, and allows immediate in-memory testing. This validates the architecture quickly before tackling async drivers.

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
