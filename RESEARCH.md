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
