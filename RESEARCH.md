# RESEARCH.md

## Executive Summary

Plenum is a lightweight, agent-first database control CLI designed specifically for autonomous AI coding agents. The project aims to provide a deterministic, least-privilege execution surface for database operations, exposed via a local MCP (Model Context Protocol) server. This is not a human-oriented database client.

**Implementation Language:** Rust

---

## Key Design Principles

### 1. Agent-First Philosophy
- **Machine-only interface**: No interactive UX, REPL, TUI, or autocomplete
- **JSON-only output**: All stdout must be structured, machine-parseable JSON
- **Deterministic behavior**: Identical inputs produce identical outputs (excluding timing metadata)
- **No human conveniences**: Features that primarily benefit humans are explicitly out of scope

### 2. Security & Safety Model
- **Least privilege by default**: Read-only is the default operational mode
- **Explicit capabilities**: All operations require explicit permission flags
  - `read_only` (default)
  - `allow_write` (explicit opt-in)
  - `allow_ddl` (explicit opt-in)
  - `max_rows` (explicit limit)
  - `timeout_ms` (explicit timeout)
- **Pre-execution validation**: Capability checks occur BEFORE query execution
- **Fail-fast philosophy**: Missing inputs or capability violations fail immediately

### 3. No Abstraction Over SQL
- **Vendor-specific SQL**: PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL
- **No compatibility layers**: No "universal SQL" or query language abstraction
- **No ORMs or query builders**: Direct SQL execution only
- **Engine quirks remain isolated**: Database-specific behavior stays within engine modules

### 4. Explicitness Over Convenience
- **No inferred values**: No implicit databases, schemas, limits, or permissions
- **No auto-commit**: Transaction control must be explicit
- **No defaults**: All required parameters must be provided
- **Stable output schemas**: JSON output format is versioned and stable

---

## Target Database Engines (MVP)

Three first-class, equally constrained database engines:

1. **PostgreSQL**
2. **MySQL** (primary target)
3. **SQLite**

### MySQL-Specific Considerations
- Server version must be detected explicitly
- Avoid non-standard INFORMATION_SCHEMA extensions
- Implicit commits (e.g., DDL statements) treated as write operations
- Version-specific behavior must be surfaced in metadata
- No MySQL-specific behavior may leak into core logic

---

## CLI Architecture

### Command Surface (MVP)
Exactly three commands with no aliases or shortcuts:

1. **`plenum connect`** - Establish database connection
2. **`plenum introspect`** - Schema introspection
3. **`plenum query`** - Execute constrained queries

### Output Contract

All output follows a standardized JSON envelope:

**Success Envelope:**
```json
{
  "ok": true,
  "engine": "mysql",
  "command": "query",
  "data": {},
  "meta": {
    "execution_ms": 14,
    "rows_returned": 25
  }
}
```

**Error Envelope:**
```json
{
  "ok": false,
  "engine": "mysql",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "DDL statements are not permitted"
  }
}
```

**Critical Rules:**
- Stdout MUST NOT include logs or diagnostic text
- All errors are structured JSON
- No panics across CLI boundaries
- No silent fallbacks
- Capability violations are first-class errors

---

## MCP Integration Strategy

Plenum exposes its functionality through a local MCP server:

- **Stateless design**: No persistent sessions between invocations
- **Tool mapping**: Each CLI command maps to a single MCP tool
- **Per-invocation credentials**: Credentials passed with each invocation
- **No shared state**: No global state across invocations
- **CLI as boundary**: The Plenum CLI remains the execution boundary

---

## Rust Architecture Principles

### Trait-Based Design
- **Engine-agnostic core**: Core logic does not depend on specific database engines
- **Strict trait boundaries**: Each engine implements defined interfaces
- **Isolated implementations**: Each engine implements only:
  - Schema introspection
  - Constrained query execution
- **No shared SQL helpers**: Engine quirks stay inside engine modules

### Error Handling
- All errors are structured JSON
- Driver errors are wrapped and normalized
- No panics across CLI boundaries
- Capability violations are first-class errors

### Database Driver Selection Strategy

**Decision:** Use native, engine-specific drivers. Do NOT use sqlx or any cross-database abstraction layer.

**Selected Drivers:**
- **PostgreSQL**: `tokio-postgres` - Official PostgreSQL Rust driver
- **MySQL**: `mysql_async` - Purpose-built MySQL async driver
- **SQLite**: `rusqlite` - Official SQLite wrapper for Rust

**Rationale:**

1. **Maximum Isolation Between Engines**
   - Each engine module uses a completely different driver crate
   - No shared types or traits between engine implementations
   - Impossible for cross-engine behavior to leak through shared abstractions

2. **Vendor-Specific Behavior Preserved**
   - PostgreSQL quirks stay in PostgreSQL driver
   - MySQL quirks stay in MySQL driver
   - SQLite quirks stay in SQLite driver
   - No normalization or abstraction layer to hide differences

3. **No Risk of Abstraction Leakage**
   - Using sqlx with multiple feature flags would create a unified interface
   - Unified interfaces hide vendor differences (explicitly forbidden by CLAUDE.md)
   - Native drivers prevent accidental SQL compatibility assumptions

4. **Aligns with Core Principles**
   - "No compatibility layers" (CLAUDE.md line 22)
   - "No shared SQL helpers across engines" (CLAUDE.md line 172)
   - "Engine quirks stay inside engine modules" (CLAUDE.md line 174)

**What This Means for Implementation:**

Each engine module (`src/engine/postgres/`, `src/engine/mysql/`, `src/engine/sqlite/`) will:
- Import only its own native driver
- Handle connection management independently
- Implement data type conversion independently
- Handle errors using driver-specific error types
- Have zero shared database code with other engines

The only shared code across engines is:
- The `DatabaseEngine` trait definition (interface contract)
- JSON envelope types (output format)
- Capability checking logic (permission enforcement)
- Error code mapping (for normalized JSON errors)

**Forbidden:**
- ❌ sqlx (provides unified interface across databases)
- ❌ Diesel (ORM with cross-database abstraction)
- ❌ SeaORM (ORM with cross-database abstraction)
- ❌ Any crate that abstracts over multiple database engines
- ❌ Shared query building or SQL generation helpers

---

## SQL Categorization Strategy

**Decision:** Use regex-based SQL categorization with engine-specific implementations. Do NOT use external SQL parser libraries.

**Context:**
Plenum enforces capability constraints (read-only, write, DDL) by categorizing SQL statements **before execution**. The implementation strategy must:
- Align with "simplest explicit implementation" principle
- Respect vendor-specific SQL differences (PostgreSQL ≠ MySQL ≠ SQLite)
- Not introduce cross-engine abstraction layers
- Enable fast pre-execution validation
- Be deterministic and testable

**Evaluated Approaches:**

1. **Regex Pattern Matching** ✅ **SELECTED**
   - Simple, no external dependencies
   - Engine-specific implementations possible
   - Fast pre-execution checks
   - Requires careful edge case handling

2. **SQL Parser Library (sqlparser crate)** ❌ **REJECTED**
   - Adds external dependency
   - Generic parser may not handle vendor-specific SQL
   - Creates abstraction layer (violates "no shared SQL helpers")
   - More robust but conflicts with "simplest explicit implementation"

3. **Database-Specific Query Analysis** ❌ **REJECTED**
   - Requires database connection just for validation
   - Defeats "capability checks occur BEFORE execution"
   - Far too complex for MVP

**Implementation Architecture:**

Each engine implements its own `categorize_query(sql: &str) -> Result<QueryCategory>` logic:

```rust
// src/capability/mod.rs
pub enum QueryCategory {
    ReadOnly,
    Write,
    DDL,
}

// Each engine has its own categorization
// src/engine/postgres/mod.rs
impl PostgresEngine {
    fn categorize_query(sql: &str) -> Result<QueryCategory> { ... }
}

// src/engine/mysql/mod.rs
impl MySQLEngine {
    fn categorize_query(sql: &str) -> Result<QueryCategory> { ... }
}

// src/engine/sqlite/mod.rs
impl SQLiteEngine {
    fn categorize_query(sql: &str) -> Result<QueryCategory> { ... }
}
```

**Pre-Processing Steps** (before categorization):
1. Trim leading/trailing whitespace
2. Strip SQL comments: `--` line comments and `/* */` block comments
3. Normalize to uppercase for pattern matching
4. Detect multi-statement queries (presence of `;` separators)
5. **Reject multi-statement queries in MVP** (safest approach)

**SQL Statement Categorization:**

| Category | Statements |
|----------|-----------|
| **Read-only** | SELECT, WITH ... SELECT (CTEs) |
| **Write** | INSERT, UPDATE, DELETE, CALL/EXEC (stored procedures) |
| **DDL** | CREATE, DROP, ALTER, TRUNCATE, RENAME |
| **Transaction Control** | BEGIN, COMMIT, ROLLBACK (treated as read-only) |

**Edge Case Handling:**

1. **Multi-statement queries** (e.g., `SELECT * FROM users; DROP TABLE users;`)
   - **MVP approach**: Reject entirely (safest)
   - **Rationale**: Prevents SQL injection via statement chaining
   - **Post-MVP**: Could analyze each statement and require highest capability

2. **EXPLAIN queries** (e.g., `EXPLAIN SELECT * FROM users`)
   - Strip EXPLAIN prefix, categorize underlying statement
   - `EXPLAIN SELECT` → read-only (no execution)
   - `EXPLAIN ANALYZE UPDATE` → write (actually executes in PostgreSQL)
   - Engine-specific handling required

3. **CTEs (Common Table Expressions)** (e.g., `WITH cte AS (...) SELECT ...`)
   - Match final statement type
   - `WITH ... SELECT` → read-only
   - `WITH ... INSERT` → write
   - `WITH ... CREATE` → DDL

4. **Transaction control** (e.g., `BEGIN; COMMIT; ROLLBACK;`)
   - Treat as read-only operations
   - No-ops without write capability
   - Not dangerous in isolation

5. **Stored procedure calls** (e.g., `CALL my_procedure();`)
   - Treat as write operations (conservative approach)
   - Procedures can do anything internally
   - Cannot inspect procedure body for categorization

6. **Unknown statement types**
   - Treat as DDL (fail-safe, most restrictive)
   - Better to deny safe operations than allow dangerous ones
   - Error messages guide agents to use correct flags

7. **Empty queries**
   - Return error immediately
   - No execution allowed

8. **Parsing errors**
   - Return error immediately
   - Do not proceed to execution

**Engine-Specific Considerations:**

**PostgreSQL:**
- Standard SQL categorization
- EXPLAIN ANALYZE executes query (categorize underlying statement)
- Support for CTEs with DML (e.g., `WITH ... INSERT ... RETURNING`)

**MySQL:**
- Maintain explicit list of **implicit commit DDL statements**:
  - CREATE/ALTER/DROP TABLE/DATABASE/INDEX
  - TRUNCATE TABLE
  - RENAME TABLE
  - LOCK TABLES
  - SET autocommit = 1
- These cause implicit commits even within transactions
- Document in MySQL engine module
- Surface in error messages if capability violation occurs

**SQLite:**
- SQLite-specific DDL handling (PRAGMA statements, ATTACH DATABASE)
- Simpler transaction model
- No stored procedures (CALL not applicable)

**Test Coverage Requirements:**

Each engine must have comprehensive edge case tests:
- ✅ Comment variations: `--`, `/* */`, mixed, nested
- ✅ Whitespace variations: leading, trailing, tabs, newlines
- ✅ Case sensitivity: lowercase, uppercase, MixedCase
- ✅ CTE queries: `WITH ... SELECT`, `WITH ... INSERT`, `WITH ... CREATE`
- ✅ EXPLAIN queries: with and without ANALYZE keyword
- ✅ Transaction control: BEGIN, COMMIT, ROLLBACK, START TRANSACTION
- ✅ Multi-statement detection: reject `SELECT ...; DROP ...;`
- ✅ Unknown statement types: should default to DDL
- ✅ Empty queries: should return error
- ✅ Stored procedure calls: CALL (MySQL/PostgreSQL), EXEC (SQL Server - not MVP)
- ✅ Engine-specific quirks: PostgreSQL CTEs, MySQL implicit commits, SQLite PRAGMAs

**Known Limitations & Accepted Trade-offs:**

1. **Regex can be fooled by complex patterns**
   - Mitigation: Comprehensive test suite catches edge cases
   - Mitigation: Fail-safe defaults (unknown → DDL) protect safety

2. **Some edge cases may require iteration**
   - MVP ships with known limitations documented
   - Post-MVP iteration based on real-world agent usage

3. **Complex nested queries may be mis-categorized**
   - Agents receive clear error messages
   - Error messages guide to correct capability flags
   - Better to be conservative (deny) than permissive (allow dangerous ops)

**Post-MVP Evolution Criteria:**

Consider migrating to `sqlparser` crate if:
1. Regex approach proves insufficient after real-world agent usage
2. Edge cases become too numerous to handle with regex
3. Agents frequently encounter false positives/negatives
4. Vendor-specific SQL support in sqlparser improves
5. Benefits outweigh cost of adding external dependency

**Do NOT migrate to sqlparser if:**
- It creates cross-engine abstraction layer
- It normalizes vendor-specific SQL behavior
- It violates "no shared SQL helpers" principle

**Rationale for Decision:**

✅ **Aligns with Core Principles:**
- "Simplest explicit implementation" (CLAUDE.md:300)
- "No shared SQL helpers across engines" (CLAUDE.md:240)
- "No abstractions without justification" (CLAUDE.md:295)

✅ **Technical Benefits:**
- No external dependencies (uses stdlib regex)
- Fast pre-execution validation
- Engine-specific implementations respect vendor differences
- Deterministic and testable
- Fail-safe defaults protect agent safety

✅ **Answers Guiding Question:**
"Does this make autonomous agents safer, more deterministic, or more constrained?"
- **Safer**: Fail-safe defaults, multi-statement rejection
- **More deterministic**: Same query always categorized the same way
- **More constrained**: Conservative approach (deny when uncertain)

---

## Explicit Non-Goals

The following features are explicitly OUT OF SCOPE:

### Infrastructure
- ORMs (Object-Relational Mapping)
- Query builders
- Migration systems
- Connection pooling across invocations
- Caching mechanisms
- Schema inference heuristics

### User Experience
- Interactive shells/REPLs
- Terminal User Interfaces (TUIs)
- Autocomplete features
- Human-friendly output formatting
- Implicit defaults or convenience features

**Decision Criteria:** If a feature primarily benefits humans, it does not belong in Plenum.

---

## Testing Strategy

### Test Requirements
- **Capability enforcement tests**: Verify permission checks work correctly
- **JSON output snapshot tests**: Ensure output format stability
- **Engine-specific tests**: Separate test suites for PostgreSQL, MySQL, and SQLite
- **Deterministic tests**: No reliance on external cloud services
- **Local-only**: All tests run without internet connectivity

### Testing Philosophy
- Tests must be deterministic
- No flaky tests tolerated
- Each engine tested independently
- Capability violations tested exhaustively

---

## Implementation Guidelines for AI Agents

### Contribution Rules
1. **Do NOT broaden scope** - Stay focused on core functionality
2. **Do NOT add abstractions** - Only add abstractions with explicit justification
3. **Do NOT introduce implicit behavior** - Everything must be explicit
4. **Prefer deletion over generalization** - Simplify rather than generalize
5. **Ask before adding dependencies** - Every dependency must be justified

### Guiding Question
Before adding any code, ask:

> **"Does this make autonomous agents safer, more deterministic, or more constrained?"**

If the answer is no, it does not belong in Plenum.

### Code Review Checklist
- [ ] Does it maintain vendor-specific SQL?
- [ ] Does it output only JSON to stdout?
- [ ] Does it enforce capabilities before execution?
- [ ] Does it avoid implicit behavior?
- [ ] Does it maintain determinism?
- [ ] Does it stay within the three-command CLI surface?
- [ ] Does it avoid human-oriented features?

---

## Technical Constraints Summary

| Aspect | Constraint |
|--------|-----------|
| **Language** | Rust |
| **Output Format** | JSON only (stdout) |
| **Commands** | Exactly 3: connect, introspect, query |
| **Database Engines** | PostgreSQL, MySQL, SQLite |
| **Default Mode** | Read-only |
| **State Management** | Stateless (no sessions) |
| **SQL Handling** | Vendor-specific, no abstraction |
| **Error Handling** | Structured JSON, fail-fast |
| **Capabilities** | Explicit, never inferred |
| **Testing** | Deterministic, local-only |

---

## Risk Assessment

### Potential Challenges

1. **MySQL Version Variability**
   - Different versions have different behaviors
   - Must detect and surface version-specific behavior
   - Avoid non-standard extensions

2. **Capability Enforcement Complexity**
   - Must validate capabilities before execution
   - DDL statements in MySQL trigger implicit commits
   - Write vs. read classification must be accurate

3. **Error Normalization**
   - Each database has different error formats
   - Must wrap and normalize without losing information
   - Must maintain structured JSON format

4. **MCP Integration**
   - Stateless design requires credentials per call
   - No session management increases complexity
   - Tool mapping must be 1:1 with CLI commands

### Mitigation Strategies

1. **Version Detection**: Implement explicit version detection for all engines
2. **Capability Pre-checks**: Validate all capabilities before query parsing
3. **Error Translation Layer**: Create database-agnostic error types
4. **Integration Testing**: Comprehensive tests for MCP server integration

---

## Open Questions

1. **Connection Management**: How are connection strings formatted and validated?
2. **Credential Security**: How are credentials passed securely through MCP?
3. **Query Parsing**: What level of SQL parsing is required for capability checks?
4. **Timeout Implementation**: How are query timeouts enforced across engines?
5. **Row Limiting**: How is `max_rows` enforced without modifying queries?
6. **Transaction Boundaries**: How are transaction semantics handled?

---

## Success Criteria

The MVP will be considered successful when:

1. ✅ All three commands (connect, introspect, query) work for all three engines
2. ✅ All output is valid, structured JSON
3. ✅ Capability violations are caught before execution
4. ✅ Read-only mode prevents all modifications
5. ✅ Engine-specific test suites pass
6. ✅ MCP server exposes all CLI functionality
7. ✅ No human-oriented features are present
8. ✅ Deterministic behavior is maintained

---

## References

- **Primary Document**: CLAUDE.md
- **Model Context Protocol**: [MCP Specification](https://modelcontextprotocol.io/)
- **Target Databases**:
  - PostgreSQL Documentation
  - MySQL Documentation
  - SQLite Documentation
- **Implementation Language**: Rust Programming Language
