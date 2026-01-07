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
- Global: `~/.config/plenum/connections.json` (per-user, keyed by project path)

**Resolution Precedence:**
1. Explicit CLI flags (highest priority)
2. Local config file
3. Global config file
4. Error if no connection available

**Named Connections:**
Connections are stored as named profiles (e.g., "local", "dev", "prod").
Agents can reference connections by name: `plenum query --name prod --sql "..."`

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

## Capability Model

Read-only is the default mode (SELECT queries only).

Operations requiring higher privileges are gated by explicit capability flags:
- allow_write (enables INSERT, UPDATE, DELETE)
- allow_ddl (enables CREATE, DROP, ALTER, etc.)
- max_rows (limits result set size)
- timeout_ms (limits execution time)

Violations MUST fail before query execution.

Capabilities are NEVER inferred.

### Capability Hierarchy

Capabilities follow a strict hierarchy:
- **Read-only** (default): No flags needed, SELECT queries only
- **Write**: Requires `--allow-write` flag, enables INSERT, UPDATE, DELETE
- **DDL**: Requires `--allow-ddl` flag, enables DDL operations AND write operations

**Important rules:**
- `--allow-ddl` implicitly grants write permissions (DDL is a superset of write)
- `--allow-write` does NOT enable DDL operations (DDL requires explicit flag)
- Agents must explicitly request `--allow-ddl` even if write is already enabled

**Examples:**
- `plenum query --sql "SELECT ..."` → allowed (read-only default)
- `plenum query --sql "INSERT ..." --allow-write` → allowed
- `plenum query --sql "CREATE TABLE ..." --allow-write` → DENIED (needs --allow-ddl)
- `plenum query --sql "CREATE TABLE ..." --allow-ddl` → allowed (DDL implies write)
- `plenum query --sql "INSERT ..." --allow-ddl` → allowed (DDL implies write)

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

Plenum's security boundary is **capability enforcement**, not SQL validation.

### Plenum Enforces:
- Operation type restrictions (read-only, write, DDL)
- Row limits and timeouts
- Credential security (no logging/persistence in error messages or logs)

### Plenum Does NOT Enforce:
- SQL injection prevention
- Query semantic correctness
- Business logic constraints

### Agent Responsibility:
The calling agent MUST:
- Sanitize user inputs before constructing SQL
- Validate queries for safety before passing to Plenum
- Implement application-level security controls

**Plenum assumes SQL passed to it is safe.** It provides capability constraints, not query validation.

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
