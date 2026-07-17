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
- [Read-Only Enforcement](#read-only-enforcement)
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
- **Strict read-only enforcement**: All write and DDL operations are unconditionally rejected
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

The five core invariants governing Plenum's design are defined in [CLAUDE.md](CLAUDE.md). Their architectural implementations are described in [Read-Only Enforcement](#read-only-enforcement), [Stateless Design](#stateless-design), and [Engine Isolation](#engine-isolation).

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
│ Read-Only Check      │  BEFORE EXECUTION
│  SELECT / SHOW /     │
│  DESCRIBE / PRAGMA / │
│  EXPLAIN / txn ctrl  │
│  → permitted         │
│  anything else       │
│  → REJECTED          │
└──────────────────────┘
      │
      ├──[REJECTED]──────────────┐
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

## Read-Only Enforcement

Read-only enforcement is the **core security mechanism**. Plenum rejects all write and DDL operations unconditionally — there are no flags or capabilities to unlock them.

### Permitted Operations

| Category | Examples |
|---|---|
| SELECT queries | `SELECT * FROM users`, subqueries, read-only CTEs |
| MySQL introspection | `SHOW TABLES`, `DESCRIBE users`, `DESC users` |
| SQLite introspection | `PRAGMA table_info(users)`, `PRAGMA database_list` (allowlisted names only) |
| EXPLAIN queries | `EXPLAIN SELECT ...`, `EXPLAIN ANALYZE ...`, `EXPLAIN FORMAT=JSON ...` |
| Transaction control | `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVEPOINT`, `RELEASE` |

### Rejected Operations

Any statement not in the permitted list above is rejected with `CAPABILITY_VIOLATION`:
- `INSERT`, `UPDATE`, `DELETE` — write operations
- `CREATE`, `DROP`, `ALTER`, `TRUNCATE` — DDL operations
- `MERGE`, `REPLACE`, `COPY`, `LOCK` — MySQL/PostgreSQL writes
- `VACUUM`, `REINDEX`, `ATTACH`, `DETACH` — SQLite destructive operations
- `SELECT ... INTO` (PostgreSQL CTAS shorthand), `SELECT ... INTO OUTFILE`/`DUMPFILE`/`@var` (MySQL)
- Writable CTEs (`WITH x AS (INSERT ...) SELECT ...`) — all engines
- Any multi-statement query (rejected before read-only check)
- Unknown statements — fail-safe default-deny

### Validation Flow

```rust
pub fn validate_query(sql: &str, _caps: &Capabilities, engine: DatabaseType) -> Result<()> {
    // 1. Preprocess SQL (trim, strip comments, detect multi-statement, uppercase)
    let processed = preprocess_sql(sql)?;

    // 2. Check if query is read-only (engine-specific)
    if is_read_only(&processed, engine) {
        Ok(())
    } else {
        Err(PlenumError::capability_violation(
            "Plenum is read-only and cannot execute this query. \
             Please run this query manually:\n\n{sql}"
        ))
    }
}
```

The `_caps` parameter carries only safety constraints (`max_rows`, `timeout_ms`, `max_bytes`) — it has no `allow_write` or `allow_ddl` fields. There is no way to unlock write operations through the API.

### Engine-Specific Validation

Each engine implements its own read-only check (no shared SQL helpers):

**PostgreSQL:** Permits `SELECT` (rejecting `SELECT ... INTO new_table` via DML keyword scan), read-only CTEs, EXPLAIN, and transaction control statements.

**MySQL:** Same as PostgreSQL plus `SHOW` and `DESCRIBE`/`DESC`. Rejects `SELECT ... INTO OUTFILE`, `INTO DUMPFILE`, and `INTO @var`.

**SQLite:** Permits `SELECT`, EXPLAIN, transaction control, and an explicit allowlist of safe PRAGMAs. `PRAGMA writable_schema = 1`, `PRAGMA wal_checkpoint`, and similar destructive PRAGMAs are rejected by default-deny.

### Edge Cases Handled

- **EXPLAIN queries**: Strip EXPLAIN prefix (including all option forms), validate underlying statement
- **CTEs (WITH ... SELECT)**: Scan entire body for DML/DDL keywords — writable CTEs are rejected
- **Multi-statement queries**: Rejected before read-only check (no shared-separator bypass possible)
- **Comments**: Stripped before validation
- **Unknown statements**: Default-deny (fail-safe)

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

There are two distinct schemas depending on the file location.

**Local config** (`.plenum/config.json`) — `ProjectConfig`, already project-scoped, no `projects` wrapper:

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

**Global config** (`~/.config/plenum/connections.json`) — `ConnectionRegistry`, projects keyed by absolute path:

```json
{
  "projects": {
    "/home/user/project1": {
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
  }
}
```

### Environment Variable Support

Passwords can be stored as environment variable references.

Local config (`.plenum/config.json`):

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

Global config (`~/.config/plenum/connections.json`):

```json
{
  "projects": {
    "/home/user/myapp": {
      "connections": {
        "secure": {
          "engine": "postgres",
          "host": "db.example.com",
          "user": "admin",
          "password_env": "DB_PASSWORD",  ← References env var
          "database": "app"
        }
      },
      "default": "secure"
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
    "message": "Plenum is read-only and cannot execute this query. Please run this query manually:\n\nDROP TABLE users"
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

For the complete error code reference with usage guidance, see [Error Codes in README.md](README.md#error-codes).

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

    // 2. Build capabilities from request (safety constraints only — no write flags)
    let caps = Capabilities {
        max_rows: request.max_rows,
        timeout_ms: request.timeout_ms,
        ..Default::default()
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
  --max-rows 100                      "name": "prod",
                                      "sql": "SELECT ...",
                                      "max_rows": 100
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

Plenum's security model is based on **strict read-only enforcement**, not SQL validation.

### Security Boundaries

For the definitive list of what Plenum enforces and what it does not, see [CLAUDE.md](CLAUDE.md). The architectural implementation of read-only enforcement is described in [Read-Only Enforcement](#read-only-enforcement).

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

**Best practices** (global config, `~/.config/plenum/connections.json`):
```json
{
  "projects": {
    "/home/user/myapp": {
      "connections": {
        "prod": {
          "engine": "postgres",
          "host": "db.example.com",
          "user": "app",
          "password_env": "PROD_DB_PASSWORD",  ← Use env var
          "database": "production"
        }
      },
      "default": "prod"
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

### Why Pattern-Matching for Read-Only Validation?

**Alternatives considered:**
1. Full SQL parser (sqlparser-rs)
2. Database EXPLAIN to classify queries
3. Trial execution with rollback

**Rejected because:**
1. Parser: Too complex and heavyweight for a binary read/reject decision
2. EXPLAIN: Requires a live connection (fails on invalid credentials)
3. Trial execution: Unsafe, side effects possible even with rollback

**Pattern-matching wins:**
- Simple, explicit, and auditable
- Engine-specific (respects SQL dialects)
- No external dependencies
- Fast and deterministic
- Easy to verify: permitted set is an explicit allowlist, everything else is rejected

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

1. **Agent safety** through strict read-only enforcement
2. **Determinism** through stateless design
3. **Explicitness** through no implicit behavior
4. **Simplicity** through minimal abstractions
5. **Isolation** through independent engine implementations

This architecture enables AI agents to interact with databases in a **safe, predictable, and constrained** manner.
