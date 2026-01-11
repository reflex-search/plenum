# CLAUDE.md

## Project Overview

Plenum is a lightweight, agent-first database control CLI designed for autonomous AI coding agents and exposed via a local MCP server.

It provides a deterministic, least-privilege execution surface for:
- schema introspection
- constrained query execution

Plenum is not a human-oriented database client.

The implementation language is Rust.

---

## Core Principles (Non-Negotiable)

1. No query language abstraction
   - SQL remains vendor-specific.
   - PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL.
   - Do NOT introduce compatibility layers or “universal SQL”.

2. Agent-first, machine-only
   - No interactive runtime UX (REPL or TUI for query execution)
   - Interactive configuration is allowed (for `plenum connect` setup)
   - No autocomplete
   - No human-friendly output
   - Stdout MUST be JSON only.

3. Explicit over implicit
   - No inferred databases, schemas, limits, or permissions.
   - No auto-commit.
   - Missing inputs MUST fail fast.

4. Least privilege
   - Read-only is the default mode.
   - Writes and DDL require explicit capabilities.
   - Capability checks occur BEFORE execution.

5. Determinism
   - Identical inputs produce identical outputs (excluding timing metadata).
   - Output schemas are stable and versioned.

---

## Non-Goals

Do NOT implement:
- ORMs
- query builders
- migrations
- interactive shells
- implicit defaults
- connection pooling across invocations
- caching
- schema inference heuristics
- human UX features

If a feature primarily benefits humans, it is out of scope.

---

## Supported Databases (MVP)

The MVP MUST support:
- PostgreSQL
- MySQL (primary target)
- SQLite

All engines are first-class and equally constrained.

---

## CLI Surface (MVP)

Exactly three commands:

**plenum connect** - Configuration management for database connections
  - Interactive picker for existing connections (no args)
  - Interactive wizard for creating new connections
  - Non-interactive connection creation (with flags)
  - Validates and stores connection details locally or globally
  - Does NOT maintain persistent connections

**plenum introspect** - Schema introspection
  - Uses stored connections or explicit connection flags
  - Returns schema information as JSON

**plenum query** - Constrained query execution
  - Uses stored connections or explicit connection flags
  - Requires explicit capability flags for writes/DDL
  - Returns query results as JSON

No aliases.
No shorthand flags.
No nested command trees.

---

## Connection Configuration

Connection details can be stored for agent convenience:

**Storage Locations:**
- Local: `.plenum/config.json` (team-shareable, per-project)
- Global: `~/.config/plenum/connections.json` (per-user)

**Storage Structure:**
Connections are organized by project path, with named connections and an explicit default pointer:
```json
{
  "projects": {
    "/home/user/project1": {
      "connections": {
        "local": { ... },
        "staging": { ... },
        "prod": { ... }
      },
      "default": "local"
    },
    "/home/user/project2": {
      "connections": {
        "main": { ... }
      },
      "default": "main"
    }
  }
}
```

**Auto-Discovery:**
- When no `--name` or `--project-path` is specified, Plenum uses the current working directory as the project path
- Uses the project's default connection if `--name` is not provided
- The first connection created for a project is automatically set as the default
- Example: Running `plenum query --sql "SELECT ..."` from `/home/user/project1` automatically uses the connection specified by `/home/user/project1`'s `default` pointer

**Resolution Precedence:**
1. Explicit CLI flags (highest priority)
2. Local config file (`.plenum/config.json`)
3. Global config file (`~/.config/plenum/connections.json`)
4. Error if no connection available

**Named Connections:**
Connections are organized by project path, with named connections and a default pointer:
- `plenum query --sql "..."` → uses current project's default connection (specified by `default` pointer)
- `plenum query --name staging --sql "..."` → uses current project's "staging" connection
- `plenum query --project-path /other/project --sql "..."` → uses other project's default connection
- `plenum query --project-path /other/project --name prod --sql "..."` → uses other project's "prod" connection

**Security:**
- Credentials stored as plain JSON (user responsibility for machine security)
- Support for environment variable references (`password_env` field)
- Local configs can be committed to version control for team sharing
- Global configs remain user-private


**Stateless Execution:**
Despite stored configurations, each command invocation:
- Reads config from disk
- Opens connection
- Executes operation
- Closes connection
- Returns result

No connections persist between invocations.

---

## Output Contract

All output is machine-parseable JSON.

### Success Envelope

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

### Error Envelope

{
  "ok": false,
  "engine": "mysql",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "DDL statements are not permitted"
  }
}

Stdout MUST NOT include logs or diagnostic text.

---

## Read-Only Mode

**Plenum is strictly read-only.** All write and DDL operations are prohibited.

Permitted operations:
- `SELECT` queries
- `SHOW`, `DESCRIBE`, `PRAGMA` statements (database-specific introspection)
- `EXPLAIN` and `EXPLAIN ANALYZE`
- Transaction control statements (BEGIN, COMMIT, ROLLBACK, SAVEPOINT, RELEASE)

Rejected operations:
- `INSERT`, `UPDATE`, `DELETE` (write operations)
- `CREATE`, `DROP`, `ALTER`, `TRUNCATE` (DDL operations)
- Any other statement that modifies data or schema

### Safety Constraints

While Plenum does not allow write operations, it provides safety constraints for read operations:
- `max_rows` (limits result set size to prevent overwhelming MCP token limits)
- `timeout_ms` (limits execution time to prevent long-running queries)

**Examples:**
- `plenum query --sql "SELECT * FROM users" --max-rows 100` → allowed
- `plenum query --sql "INSERT INTO users ..." ` → DENIED (Plenum is read-only)
- `plenum query --sql "CREATE TABLE ..." ` → DENIED (Plenum is read-only)
- `plenum query --sql "SHOW TABLES"` → allowed
- `plenum query --sql "EXPLAIN SELECT * FROM users"` → allowed

### Agent Workflow for Write Operations

When an agent determines that a write or DDL operation is needed:
1. Use `plenum introspect` to understand the schema
2. Use `plenum query` to read current data if needed
3. Construct the appropriate SQL statement
4. **Present the SQL to the user** in the response for manual execution

Plenum will never execute write operations - this ensures all data modifications remain under human control.

---

## MySQL-Specific Constraints

Because MySQL behavior varies by version and storage engine:

- The engine implementation MUST:
  - detect server version explicitly
  - avoid reliance on non-standard INFORMATION_SCHEMA extensions
  - treat implicit commits (e.g. DDL) as write operations requiring capability flags
- No MySQL-specific behavior may leak into core logic.

If behavior differs across MySQL versions, it MUST be surfaced explicitly in metadata.

---

## MCP Integration

Plenum is exposed via a local MCP server.

- Each CLI command maps to a single MCP tool
- Credentials are passed per invocation
- No persistent sessions
- No shared global state

The Plenum CLI remains the execution boundary.

---

## Rust Architecture Expectations

The codebase is structured around strict trait boundaries:

- Core logic is engine-agnostic
- Each engine implements only:
  - schema introspection
  - constrained query execution
- No shared SQL helpers across engines

Engine quirks stay inside engine modules.

---

## Error Handling Rules

- All errors are structured JSON
- No panics across CLI boundaries
- No silent fallbacks
- Capability violations are first-class errors
- Driver errors are wrapped and normalized

---

## Security Model

Plenum's security boundary is **read-only enforcement**, not SQL validation.

### Plenum Enforces:
- Strict read-only operation (rejects all write/DDL operations)
- Row limits and timeouts (for safety constraints)
- Credential security (no logging/persistence in error messages or logs)

### Plenum Does NOT Enforce:
- SQL injection prevention
- Query semantic correctness
- Business logic constraints
- Row-level security or access control

### Agent Responsibility:
The calling agent MUST:
- Sanitize user inputs before constructing SQL
- Validate queries for safety before passing to Plenum
- Implement application-level security controls
- Present write operations to users instead of attempting to execute them

**Plenum assumes SQL passed to it is safe for reading.** It provides read-only enforcement and safety constraints, not query validation.

---

## Testing Expectations

- Capability enforcement tests
- JSON output snapshot tests
- Engine-specific tests for PostgreSQL, MySQL, and SQLite
- No tests requiring external cloud services

Tests MUST be deterministic.

---

## Contribution Rules for AI Agents

When contributing:
- Do NOT broaden scope
- Do NOT add abstractions without explicit justification
- Do NOT introduce implicit behavior
- Prefer deletion over generalization
- Ask before adding dependencies

When in doubt, choose the simplest explicit implementation.

---

## Guiding Question

Before adding code, ask:

Does this make autonomous agents safer, more deterministic, or more constrained?

If not, it does not belong in Plenum.
