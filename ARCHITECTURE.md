# Plenum Architecture

## Table of Contents

- [Overview](#overview)
- [System Architecture](#system-architecture)
- [Core Principles](#core-principles)
- [Component Diagrams](#component-diagrams)
- [Trait Hierarchy](#trait-hierarchy)
- [Data Flow](#data-flow)
- [Engine Isolation](#engine-isolation)
- [Stateless Design](#stateless-design)
- [Capability Enforcement](#capability-enforcement)
- [Configuration Resolution](#configuration-resolution)
- [Output Envelopes](#output-envelopes)
- [MCP Integration](#mcp-integration)
- [Security Model](#security-model)

---

## Overview

Plenum is a lightweight, agent-first database control CLI built in Rust. It provides a deterministic, least-privilege execution surface for AI agents to interact with databases safely.

**Key architectural decisions:**
- **Stateless execution**: No persistent connections between invocations
- **Engine isolation**: Each database engine is completely independent
- **Capability-based security**: Read-only by default, explicit flags for writes/DDL
- **JSON-only output**: Machine-parseable, no human-oriented features
- **Vendor-specific SQL**: No query abstraction layer
- **Native drivers**: tokio-postgres, mysql_async, rusqlite (NOT sqlx)

---

## System Architecture

Plenum is structured as a library with thin CLI and MCP wrappers:

```
┌─────────────────────────────────────────────────────────────────┐
│                        Plenum Library                            │
│  ┌────────────┐  ┌────────────┐  ┌────────────┐  ┌──────────┐  │
│  │   Error    │  │   Output   │  │ Capability │  │  Config  │  │
│  │  Handling  │  │  Envelopes │  │ Validation │  │  Mgmt    │  │
│  └────────────┘  └────────────┘  └────────────┘  └──────────┘  │
│                                                                   │
│  ┌─────────────────────────────────────────────────────────┐    │
│  │            DatabaseEngine Trait (stateless)              │    │
│  └─────────────────────────────────────────────────────────┘    │
│         │                    │                   │               │
│         ▼                    ▼                   ▼               │
│  ┌────────────┐      ┌────────────┐     ┌────────────┐         │
│  │  SQLite    │      │ PostgreSQL │     │   MySQL    │         │
│  │  Engine    │      │   Engine   │     │   Engine   │         │
│  │ (rusqlite) │      │ (tokio-pg) │     │(mysql_async)│         │
│  └────────────┘      └────────────┘     └────────────┘         │
└─────────────────────────────────────────────────────────────────┘
         │                                    │
         ▼                                    ▼
┌─────────────────┐                  ┌─────────────────┐
│   CLI Wrapper   │                  │   MCP Server    │
│   (main.rs)     │                  │   (mcp.rs)      │
│                 │                  │                 │
│  • connect      │                  │  • connect tool │
│  • introspect   │                  │  • introspect   │
│  • query        │                  │  • query tool   │
└─────────────────┘                  └─────────────────┘
         │                                    │
         ▼                                    ▼
    JSON stdout                          JSON-RPC stdio
```

---

## Core Principles

### 1. Agent-First Design
- **JSON-only output** to stdout (no human-friendly formatting)
- **Deterministic behavior** (same input → same output)
- **No interactive UX** for execution (interactive config setup allowed)
- **Explicit over implicit** (no inferred values)

### 2. No Query Abstraction
- SQL remains **vendor-specific**
- PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL
- No compatibility layers or universal SQL
- Each engine handles its own quirks

### 3. Least Privilege by Default
- **Read-only mode** is the default
- Writes require `--allow-write` flag
- DDL requires `--allow-ddl` flag
- Capability checks happen **before** execution

### 4. Stateless Execution
- No persistent database connections
- Each command invocation:
  1. Reads config from disk
  2. Opens connection
  3. Executes operation
  4. Closes connection
  5. Returns result
- No connection pooling across invocations
- No caching

### 5. Engine Isolation
- Each engine module is completely independent
- No shared SQL helpers across engines
- Native drivers (NOT sqlx) for maximum isolation
- Engine-specific behavior stays inside engine modules

---

## Component Diagrams

### Module Organization

```
src/
├── lib.rs                  # Public API exports
├── main.rs                 # CLI entry point
├── error.rs                # PlenumError enum & Result type
├── output.rs               # JSON envelope types
├── capability/
│   └── mod.rs             # SQL categorization & validation
├── config/
│   └── mod.rs             # Connection configuration management
└── engine/
    ├── mod.rs             # DatabaseEngine trait & core types
    ├── sqlite/
    │   └── mod.rs         # SQLite implementation
    ├── postgres/
    │   └── mod.rs         # PostgreSQL implementation
    └── mysql/
        └── mod.rs         # MySQL implementation
```

### Crate Dependencies

```
┌──────────────────────────────────────────────────┐
│                 Plenum Binary                     │
│  ┌──────────────────────────────────────────┐   │
│  │           Plenum Library                  │   │
│  │                                           │   │
│  │  Core Deps:                               │   │
│  │  • serde, serde_json  (serialization)    │   │
│  │  • thiserror, anyhow  (error handling)   │   │
│  │  • clap               (CLI)              │   │
│  │  • dirs               (config paths)     │   │
│  │  • dialoguer          (interactive UI)   │   │
│  │  • tokio              (async runtime)    │   │
│  │                                           │   │
│  │  Engine Deps (feature-gated):            │   │
│  │  • rusqlite           (SQLite)           │   │
│  │  • tokio-postgres     (PostgreSQL)       │   │
│  │  • mysql_async        (MySQL)            │   │
│  │                                           │   │
│  │  MCP Deps:                                │   │
│  │  • rmcp               (MCP protocol)     │   │
│  │  • schemars           (JSON schemas)     │   │
│  └──────────────────────────────────────────┘   │
└──────────────────────────────────────────────────┘
```

---

## Trait Hierarchy

### DatabaseEngine Trait

The `DatabaseEngine` trait is the core abstraction. All three engines implement it identically:

```rust
pub trait DatabaseEngine {
    /// Validate connection and return metadata
    /// Opens connection, retrieves info, closes connection
    fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo>;

    /// Introspect database schema
    /// Opens connection, queries schema, closes connection
    fn introspect(
        config: &ConnectionConfig,
        schema_filter: Option<&str>
    ) -> Result<SchemaInfo>;

    /// Execute query with capability constraints
    /// Opens connection, validates capabilities, executes, closes
    fn execute(
        config: &ConnectionConfig,
        query: &str,
        caps: &Capabilities
    ) -> Result<QueryResult>;
}
```

**Key trait characteristics:**
- All methods are **static** (no `&self` parameter)
- All methods are **stateless** (no internal state)
- All methods take `&ConnectionConfig` as input
- Connections are opened and closed within each method
- No persistent connections between calls

### Engine Implementations

```
DatabaseEngine (trait)
    ├── SqliteEngine::validate_connection()
    ├── SqliteEngine::introspect()
    ├── SqliteEngine::execute()
    │
    ├── PostgresEngine::validate_connection()
    ├── PostgresEngine::introspect()
    ├── PostgresEngine::execute()
    │
    ├── MysqlEngine::validate_connection()
    ├── MysqlEngine::introspect()
    └── MysqlEngine::execute()
```

---

## Data Flow

### Command Execution Flow

```
User/Agent Input
      │
      ▼
┌─────────────┐
│ CLI Parsing │  (clap)
│  or MCP     │
└─────────────┘
      │
      ▼
┌─────────────────────┐
│ Config Resolution   │  (config::resolve_connection)
│  1. Explicit flags  │  Priority order:
│  2. Local config    │  Explicit > Local > Global
│  3. Global config   │
└─────────────────────┘
      │
      ▼
┌─────────────────────┐
│ Capability Building │  (Capabilities struct)
│  • allow_write      │
│  • allow_ddl        │
│  • max_rows         │
│  • timeout_ms       │
└─────────────────────┘
      │
      ▼
┌─────────────────────┐
│ Engine Dispatch     │  (match on DatabaseType)
│  match engine {     │
│    SQLite => ...    │
│    Postgres => ...  │
│    MySQL => ...     │
│  }                  │
└─────────────────────┘
      │
      ▼
┌─────────────────────┐
│ Engine Execution    │  (DatabaseEngine trait)
│  1. Open connection │
│  2. Execute op      │
│  3. Close conn      │
│  4. Return result   │
└─────────────────────┘
      │
      ▼
┌─────────────────────┐
│ Envelope Wrapping   │  (SuccessEnvelope or ErrorEnvelope)
│  {                  │
│    ok: true/false,  │
│    engine: "...",   │
│    command: "...",  │
│    data: {...},     │
│    meta: {...}      │
│  }                  │
└─────────────────────┘
      │
      ▼
   JSON stdout
```

### Query Execution Flow (with Capability Validation)

```
Query Input
      │
      ▼
┌──────────────────────┐
│ SQL Preprocessing    │
│  1. Trim whitespace  │
│  2. Strip comments   │
│  3. Detect multi-stmt│
│  4. Uppercase norm   │
└──────────────────────┘
      │
      ▼
┌──────────────────────┐
│ SQL Categorization   │  (engine-specific regex)
│  • ReadOnly: SELECT  │
│  • Write: INSERT/... │
│  • DDL: CREATE/...   │
└──────────────────────┘
      │
      ▼
┌──────────────────────┐
│ Capability Check     │  BEFORE EXECUTION
│  ReadOnly → always OK│
│  Write → need write  │
│  DDL → need ddl      │
└──────────────────────┘
      │
      ├──[FAIL]──────────────────┐
      │                           ▼
      │                    CapabilityViolation
      │                           Error
      ▼
┌──────────────────────┐
│ Query Execution      │
│  • Open connection   │
│  • Execute SQL       │
│  • Parse results     │
│  • Close connection  │
└──────────────────────┘
      │
      ├──[FAIL]──────────────────┐
      │                           ▼
      │                      QueryFailed
      │                           Error
      ▼
   QueryResult
   (success)
```

---

## Engine Isolation

Each database engine is **completely isolated** from others:

### Isolation Principles

1. **No shared SQL helpers**
   - Each engine has its own SQL categorization logic
   - PostgreSQL regex ≠ MySQL regex ≠ SQLite regex
   - No common query parsing utilities

2. **Native drivers only**
   - SQLite: `rusqlite` (synchronous)
   - PostgreSQL: `tokio-postgres` (async)
   - MySQL: `mysql_async` (async)
   - **NO sqlx** (avoids abstraction leakage)

3. **Engine-specific modules**
   ```
   src/engine/
   ├── mod.rs         # Trait definition only
   ├── sqlite/        # SQLite-specific code
   ├── postgres/      # PostgreSQL-specific code
   └── mysql/         # MySQL-specific code
   ```

4. **Vendor-specific behavior**
   - MySQL implicit commits (DDL operations)
   - PostgreSQL EXPLAIN ANALYZE execution
   - SQLite dynamic typing
   - Each engine handles its own quirks

### Engine Feature Gates

Engines are feature-gated to reduce binary size:

```toml
[features]
default = ["sqlite", "postgres", "mysql"]
sqlite = ["dep:rusqlite"]
postgres = ["dep:tokio-postgres"]
mysql = ["dep:mysql_async"]
```

Build with specific engines:
```bash
cargo build --no-default-features --features sqlite,postgres
```

---

## Stateless Design

Plenum maintains **no state** between command invocations.

### Why Stateless?

1. **Determinism**: Same input → same output (no hidden state)
2. **Safety**: No connection leaks or orphaned transactions
3. **Simplicity**: No connection pool management
4. **Agent-friendly**: Predictable behavior for AI agents

### Stateless Execution Pattern

Every command follows this pattern:

```rust
pub fn execute_command(config: ConnectionConfig) -> Result<Output> {
    // 1. Read config from disk (if needed)
    let resolved_config = resolve_connection(Some("prod"))?;

    // 2. Open connection
    let mut conn = engine::connect(&resolved_config)?;

    // 3. Execute operation
    let result = operation(&mut conn)?;

    // 4. Close connection (automatic via Drop)
    drop(conn);

    // 5. Return result
    Ok(result)
}
```

**No persistent state:**
- No `struct Server` with connection fields
- No global connection pool
- No cached query results
- No session state

**Config files are NOT state:**
- Config files store connection parameters (like bookmarks)
- Each invocation reads config fresh from disk
- Config is input data, not runtime state

---

## Capability Enforcement

Capability enforcement is the **core security mechanism**.

### Capability Hierarchy

```
┌─────────────────────────────────────┐
│         DDL Capability              │  ← Most privileged
│  • CREATE, DROP, ALTER, TRUNCATE    │
│  • Implies Write capability         │
│  • Requires --allow-ddl flag        │
└─────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────┐
│        Write Capability             │  ← Medium privilege
│  • INSERT, UPDATE, DELETE           │
│  • Requires --allow-write flag      │
│  • Does NOT imply DDL               │
└─────────────────────────────────────┘
                │
                ▼
┌─────────────────────────────────────┐
│      Read-Only (Default)            │  ← Least privileged
│  • SELECT queries only              │
│  • No flags needed                  │
│  • Always permitted                 │
└─────────────────────────────────────┘
```

### Capability Check Flow

```rust
pub fn validate_query(
    sql: &str,
    caps: &Capabilities,
    engine: DatabaseType
) -> Result<QueryCategory> {
    // 1. Preprocess SQL (strip comments, normalize)
    let processed = preprocess_sql(sql)?;

    // 2. Categorize query (engine-specific)
    let category = categorize_query(&processed, engine)?;

    // 3. Check capabilities BEFORE execution
    match category {
        QueryCategory::ReadOnly => Ok(category), // Always allowed
        QueryCategory::Write => {
            if caps.can_write() {
                Ok(category)
            } else {
                Err(CapabilityViolation("Need --allow-write"))
            }
        }
        QueryCategory::DDL => {
            if caps.can_ddl() {
                Ok(category)
            } else {
                Err(CapabilityViolation("Need --allow-ddl"))
            }
        }
    }
}
```

### SQL Categorization Strategy

Each engine implements its own categorization logic:

**PostgreSQL:**
```rust
fn categorize_postgres(sql: &str) -> QueryCategory {
    if sql.starts_with("CREATE ") || sql.starts_with("DROP ") { DDL }
    else if sql.starts_with("INSERT ") || sql.starts_with("UPDATE ") { Write }
    else if sql.starts_with("SELECT ") { ReadOnly }
    else { DDL } // Fail-safe: unknown → most restrictive
}
```

**MySQL:**
```rust
fn categorize_mysql(sql: &str) -> QueryCategory {
    // MySQL-specific: LOCK TABLES causes implicit commit → DDL
    if sql.starts_with("LOCK TABLES") { DDL }
    else if sql.starts_with("CREATE ") { DDL }
    // ... (similar to PostgreSQL)
}
```

**SQLite:**
```rust
fn categorize_sqlite(sql: &str) -> QueryCategory {
    // SQLite-specific: VACUUM, REINDEX are DDL
    if sql.starts_with("VACUUM") || sql.starts_with("REINDEX") { DDL }
    // ... (similar to PostgreSQL)
}
```

### Edge Cases Handled

- **EXPLAIN queries**: Strip EXPLAIN prefix, categorize underlying statement
- **CTEs (WITH ... SELECT)**: Check final statement type
- **Multi-statement queries**: Rejected in MVP (safest approach)
- **Comments**: Stripped before categorization
- **Unknown statements**: Default to DDL (fail-safe)

---

## Configuration Resolution

Configuration is resolved in **strict precedence order**.

### Precedence Chain

```
1. Explicit CLI flags (highest priority)
      │
      ▼
   --engine postgres --host db.example.com --port 5432
      │
      ├──[if missing]─────────┐
      │                        ▼
      │                 2. Local config
      │                 .plenum/config.json
      │                        │
      │                        ├──[if missing]─────────┐
      │                        │                        ▼
      │                        │                 3. Global config
      │                        │        ~/.config/plenum/connections.json
      │                        │                        │
      │                        │                        ├──[if missing]─┐
      │                        │                        │                ▼
      │                        │                        │             ERROR
      │                        ▼                        ▼                │
      └────────────────────> Merge ◄───────────────────┘                │
                               │                                        │
                               ▼                                        │
                        ConnectionConfig ◄──────────────────────────────┘
```

### Config File Locations

**Local config** (`.plenum/config.json`):
- Project-specific
- Team-shareable (can be committed to git)
- Located at project root

**Global config** (`~/.config/plenum/connections.json`):
- User-specific
- Cross-project
- Located in user config directory

### Config File Format

```json
{
  "connections": {
    "prod": {
      "engine": "postgres",
      "host": "db.example.com",
      "port": 5432,
      "user": "readonly",
      "database": "production",
      "password_env": "PROD_DB_PASSWORD"
    },
    "local": {
      "engine": "sqlite",
      "file": "./app.db"
    }
  },
  "default": "local"
}
```

### Environment Variable Support

Passwords can be stored as environment variable references:

```json
{
  "connections": {
    "secure": {
      "engine": "postgres",
      "host": "db.example.com",
      "user": "admin",
      "password_env": "DB_PASSWORD",  ← References env var
      "database": "app"
    }
  }
}
```

At runtime:
```rust
// Resolve password from environment
if let Some(env_var) = &stored.password_env {
    config.password = Some(std::env::var(env_var)?);
}
```

---

## Output Envelopes

All Plenum output follows a **strict JSON envelope format**.

### Success Envelope

```json
{
  "ok": true,
  "engine": "postgres",
  "command": "query",
  "data": {
    "columns": ["id", "name"],
    "rows": [
      {"id": 1, "name": "Alice"}
    ]
  },
  "meta": {
    "execution_ms": 42,
    "rows_returned": 1
  }
}
```

**Type definition:**
```rust
pub struct SuccessEnvelope<T> {
    pub ok: bool,            // Always true
    pub engine: String,      // "postgres" | "mysql" | "sqlite"
    pub command: String,     // "connect" | "introspect" | "query"
    pub data: T,             // Command-specific data
    pub meta: Metadata,      // Execution metadata
}

pub struct Metadata {
    pub execution_ms: u64,
    pub rows_returned: Option<usize>,
}
```

### Error Envelope

```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "DDL operations require --allow-ddl flag"
  }
}
```

**Type definition:**
```rust
pub struct ErrorEnvelope {
    pub ok: bool,          // Always false
    pub engine: String,    // Engine that produced error
    pub command: String,   // Command that failed
    pub error: ErrorInfo,  // Error details
}

pub struct ErrorInfo {
    pub code: String,      // Machine-readable error code
    pub message: String,   // Human-readable message
}
```

### Error Codes

| Code | Description | When It Occurs |
|------|-------------|----------------|
| `CAPABILITY_VIOLATION` | Operation blocked by capabilities | Write/DDL without appropriate flags |
| `CONNECTION_FAILED` | Database connection failed | Invalid credentials, unreachable host |
| `QUERY_FAILED` | Query execution failed | SQL syntax errors, missing tables |
| `INVALID_INPUT` | Malformed input | Missing required flags, invalid engine |
| `ENGINE_ERROR` | Engine-specific error | Database driver errors |
| `CONFIG_ERROR` | Configuration error | Missing config file, connection not found |

### Output Contract

**Guarantees:**
- All output is valid JSON
- No logs or diagnostic text to stdout (errors go to stderr)
- Deterministic serialization (stable field order)
- No sensitive data in error messages (passwords never logged)

**Validation:**
- JSON Schema files in `schemas/` directory
- Snapshot tests ensure schema stability
- Integration tests verify cross-engine consistency

---

## MCP Integration

Plenum exposes functionality via MCP (Model Context Protocol).

### MCP Architecture

```
┌──────────────────────────────────────────────────┐
│              MCP Client (e.g. Claude)             │
└──────────────────────────────────────────────────┘
                     │
                     │ JSON-RPC over stdio
                     ▼
┌──────────────────────────────────────────────────┐
│            plenum mcp (MCP Server)                │
│                                                   │
│  ┌────────────────────────────────────────────┐  │
│  │       PlenumServer (ServerHandler)         │  │
│  │                                            │  │
│  │  #[tool] connect(...)                     │  │
│  │  #[tool] introspect(...)                  │  │
│  │  #[tool] query(...)                       │  │
│  └────────────────────────────────────────────┘  │
│                     │                             │
│                     │ Calls library functions     │
│                     ▼                             │
│  ┌────────────────────────────────────────────┐  │
│  │         Plenum Library (lib.rs)            │  │
│  │  • execute_connect()                       │  │
│  │  • execute_introspect()                    │  │
│  │  • execute_query()                         │  │
│  └────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

### MCP Server Pattern

**Key principle:** MCP tools are thin wrappers around library functions.

```rust
#[derive(Clone)]
pub struct PlenumServer;

impl ServerHandler for PlenumServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            name: "plenum".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

#[tool(description = "Execute SQL query with capability constraints")]
async fn query(
    &self,
    Parameters(request): Parameters<QueryRequest>,
) -> Result<CallToolResult, McpError> {
    // 1. Resolve connection from request
    let config = resolve_config_from_request(&request)?;

    // 2. Build capabilities from request
    let caps = Capabilities {
        allow_write: request.allow_write.unwrap_or(false),
        allow_ddl: request.allow_ddl.unwrap_or(false),
        max_rows: request.max_rows,
        timeout_ms: request.timeout_ms,
    };

    // 3. Call library function (SAME as CLI)
    let result = crate::execute_query(config, &request.sql, caps)?;

    // 4. Wrap in MCP response
    Ok(CallToolResult::success(result))
}
```

### CLI vs MCP Comparison

Both interfaces call the **same library functions**:

```
CLI:                              MCP:
plenum query \                    {
  --name prod \                     "tool": "query",
  --sql "SELECT ..." \              "arguments": {
  --allow-write                       "name": "prod",
                                      "sql": "SELECT ...",
                                      "allow_write": true
                                    }
                                  }
      │                                   │
      └───────────┬───────────────────────┘
                  │
                  ▼
          crate::execute_query()
                  │
                  ▼
         DatabaseEngine::execute()
```

**Identical behavior guaranteed:**
- Same capability validation
- Same SQL categorization
- Same connection resolution
- Same error handling
- Same JSON output format

---

## Security Model

Plenum's security model is based on **capability constraints**, not SQL validation.

### Security Boundaries

#### What Plenum Enforces:

1. **Capability-based access control**
   - Read-only by default
   - Explicit flags for write/DDL operations
   - Pre-execution capability checks

2. **Operation type restrictions**
   - SELECT vs INSERT/UPDATE/DELETE vs DDL
   - Engine-specific categorization

3. **Resource limits**
   - Row limits (`max_rows`)
   - Query timeouts (`timeout_ms`)

4. **Credential security**
   - No passwords in logs or error messages
   - Support for environment variable references
   - Credentials never cached in memory

#### What Plenum Does NOT Enforce:

1. **SQL injection prevention**
   - Agent's responsibility to sanitize inputs
   - Plenum passes SQL verbatim to database

2. **Query semantic correctness**
   - No validation of table/column names
   - No type checking

3. **Business logic constraints**
   - No row-level security
   - No data access policies

### Agent Responsibility

**The calling agent MUST:**
- Sanitize user inputs before constructing SQL
- Validate queries for safety
- Implement application-level security

**Plenum assumes SQL passed to it is safe.**

### Example: SQL Injection (Agent's Job)

```javascript
// ❌ WRONG: Agent constructs unsafe SQL
const userId = userInput; // Could be "1 OR 1=1"
exec(`plenum query --sql "SELECT * FROM users WHERE id = ${userId}"`);

// ✅ CORRECT: Agent sanitizes input
const userId = sanitizeInput(userInput);
const sql = `SELECT * FROM users WHERE id = ${userId}`;
exec(`plenum query --sql "${sql}"`);
```

### Credential Security

**Storage:**
- Credentials stored as plain JSON in config files
- User's responsibility to secure config files
- Support for environment variable references

**Runtime:**
- Passwords never logged
- Passwords never in error messages
- Passwords not cached between invocations

**Best practices:**
```json
{
  "connections": {
    "prod": {
      "engine": "postgres",
      "host": "db.example.com",
      "user": "app",
      "password_env": "PROD_DB_PASSWORD",  ← Use env var
      "database": "production"
    }
  }
}
```

---

## Design Rationale

### Why Stateless?

**Alternative considered:** Connection pooling with persistent connections

**Rejected because:**
- Adds complexity (connection lifecycle management)
- Reduces determinism (pool state affects behavior)
- Increases risk of connection leaks
- Harder to reason about for agents

**Stateless wins:**
- Simpler implementation
- Predictable behavior
- No connection leaks possible
- Each invocation is independent

### Why Native Drivers (Not sqlx)?

**Alternative considered:** Use sqlx for unified database interface

**Rejected because:**
- Introduces abstraction layer (violates core principle)
- Risk of cross-engine behavior leakage
- Vendor-specific quirks hidden behind abstraction
- Harder to debug engine-specific issues

**Native drivers win:**
- Maximum engine isolation
- Vendor-specific behavior preserved
- Clear separation of concerns
- Each engine owns its quirks

### Why Regex-Based SQL Categorization?

**Alternatives considered:**
1. Full SQL parser (sqlparser-rs)
2. Database EXPLAIN to categorize queries
3. Trial execution with rollback

**Rejected because:**
1. Parser: Too complex, unnecessary for categorization
2. EXPLAIN: Requires database connection (fails for invalid credentials)
3. Trial execution: Unsafe, side effects possible

**Regex wins:**
- Simple and explicit
- Engine-specific (respects SQL dialects)
- No external dependencies
- Fast and deterministic

### Why JSON-Only Output?

**Alternative considered:** Human-friendly output with tables/colors

**Rejected because:**
- Agents don't need pretty printing
- Parsing structured data is harder than JSON
- Human UX features add complexity

**JSON-only wins:**
- Machine-parseable
- Deterministic serialization
- Easy for agents to consume
- Clear contract (JSON Schema)

---

## Performance Considerations

### Connection Overhead

Each command opens and closes a connection:

```
Command invocation time = Connection time + Execution time + Close time
```

**Typical overhead:**
- SQLite: < 10ms (file-based, no network)
- PostgreSQL: 50-200ms (network + auth)
- MySQL: 50-200ms (network + auth)

**Trade-off accepted:**
- Stateless design prioritized over performance
- Connection overhead is acceptable for agent use cases
- Agent can batch operations if needed

### Optimization Strategies

1. **Use SQLite for local/dev workflows** (fastest)
2. **Batch queries in agent logic** (not in Plenum)
3. **Use named connections** (avoid repeated config lookup)
4. **Set max_rows** to limit result set size

### Benchmark Results

See `benches/` directory for detailed performance benchmarks:

```bash
cargo bench --features sqlite
```

**Typical results (SQLite):**
- Connection validation: ~5ms
- Simple introspection: ~20ms
- SELECT query (100 rows): ~15ms
- INSERT query: ~10ms

---

## Testing Strategy

### Test Pyramid

```
        ┌──────────────────┐
        │  Integration     │  ← Cross-engine consistency
        │  Tests           │    (tests/integration_tests.rs)
        └──────────────────┘
              │
        ┌────────────────────────┐
        │  Output Validation     │  ← JSON envelope schemas
        │  Tests                 │    (tests/output_validation.rs)
        └────────────────────────┘
              │
        ┌──────────────────────────────┐
        │  Edge Case Tests             │  ← Large datasets, Unicode, etc.
        │                              │    (tests/edge_cases.rs)
        └──────────────────────────────┘
              │
        ┌────────────────────────────────────┐
        │  Unit Tests                        │  ← Capability validation, etc.
        │  (in each module)                  │    (src/**/*.rs)
        └────────────────────────────────────┘
```

### Test Categories

1. **Unit tests** (69 tests)
   - Capability validation
   - SQL categorization
   - Config resolution
   - Error handling

2. **Integration tests** (16 tests)
   - Cross-engine query consistency
   - Capability enforcement uniformity
   - NULL handling across engines

3. **Output validation tests** (11 tests)
   - JSON envelope structure
   - Schema compliance
   - No stdout pollution

4. **Edge case tests** (12 tests)
   - Large datasets (5000+ rows)
   - Unicode characters
   - Binary data (BLOBs)
   - Numeric extremes
   - Timeouts

5. **Snapshot tests**
   - JSON output stability
   - Envelope format consistency
   - Using `insta` crate

**Total: 108 tests passing**

---

## Future Architecture Considerations

### Phase 7: MCP Server

- Implement `rmcp`-based server
- Define MCP tool schemas with `schemars`
- Ensure CLI/MCP parity

### Phase 8: Security Audit

- Review capability enforcement
- Audit credential handling
- Check error message safety

### Post-MVP Enhancements

**Potential additions (NOT in scope for MVP):**
- Prepared statement support
- Transaction management
- Batch query execution
- Additional database engines (SQL Server, MongoDB)
- Observability (structured logging, metrics)

**Guiding question for all additions:**

> **"Does this make autonomous agents safer, more deterministic, or more constrained?"**

If the answer is no, it does not belong in Plenum.

---

## Conclusion

Plenum's architecture prioritizes:

1. **Agent safety** through capability enforcement
2. **Determinism** through stateless design
3. **Explicitness** through no implicit behavior
4. **Simplicity** through minimal abstractions
5. **Isolation** through independent engine implementations

This architecture enables AI agents to interact with databases in a **safe, predictable, and constrained** manner.
