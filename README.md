# Plenum

**Agent-first database control CLI with least-privilege execution**

Plenum is a lightweight, deterministic database control tool designed specifically for autonomous AI coding agents. It provides a constrained, safe execution surface for database operations with read-only defaults and explicit capability requirements.

> **This is not a human-oriented database client.** If you're looking for a tool with interactive shells, autocomplete, or pretty-printed output, Plenum is not for you.

## What is Plenum?

Plenum enables AI agents to:
- **Introspect database schemas** safely and deterministically
- **Execute constrained SQL queries** with explicit permission requirements
- **Maintain least-privilege access** with read-only mode as default
- **Produce machine-parseable output** (JSON-only to stdout)

Plenum is exposed via a local MCP (Model Context Protocol) server, making it seamlessly integrable with AI agent frameworks.

## Key Features

- **Agent-First Design**: JSON-only output, no interactive UX, deterministic behavior
- **Vendor-Specific SQL**: No query abstraction layer - PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL
- **Explicit Capabilities**: Read-only by default, writes and DDL require explicit flags
- **Stateless Execution**: No persistent connections, no caching, no implicit state
- **Three Database Engines**: PostgreSQL, MySQL, and SQLite support (first-class, equally constrained)

## Installation

### From Source

```bash
git clone https://github.com/yourusername/plenum.git
cd plenum
cargo build --release
./target/release/plenum --help
```

### System Requirements

- Rust 1.70 or later
- Supported platforms: Linux, macOS, Windows

## Usage

Plenum provides exactly three commands:

### 1. `plenum connect` - Configure Database Connections

Manage database connection configurations (interactive or non-interactive):

```bash
# Interactive connection picker
plenum connect

# Create new connection interactively
plenum connect --name prod --engine postgres --host db.example.com \
  --port 5432 --user readonly --password secret --database myapp \
  --save global

# Validate existing connection
plenum connect --name prod
```

Connection configurations are stored:
- **Local**: `.plenum/config.json` (team-shareable)
- **Global**: `~/.config/plenum/connections.json` (per-user)

### 2. `plenum introspect` - Schema Introspection

Inspect database schema and return structured JSON:

```bash
# Introspect using named connection
plenum introspect --name prod

# Introspect with explicit parameters
plenum introspect --engine postgres --host localhost --port 5432 \
  --user admin --password secret --database mydb

# Introspect specific schema
plenum introspect --name prod --schema public
```

Returns JSON with:
- Tables
- Columns (name, type, nullable)
- Primary keys
- Foreign keys
- Indexes

### 3. `plenum query` - Constrained Query Execution

Execute SQL queries with explicit capability requirements:

```bash
# Read-only query (default, no flags needed)
plenum query --name prod --sql "SELECT * FROM users WHERE id = 1"

# Write query (requires --allow-write)
plenum query --name prod --sql "UPDATE users SET name = 'Alice' WHERE id = 1" \
  --allow-write

# DDL query (requires --allow-ddl, which implies write)
plenum query --name prod --sql "CREATE TABLE test (id INT PRIMARY KEY)" \
  --allow-ddl

# With row limit and timeout
plenum query --name prod --sql "SELECT * FROM large_table" \
  --max-rows 1000 --timeout-ms 5000
```

**Capability Hierarchy:**
- **Read-only** (default): SELECT queries only
- **Write** (`--allow-write`): INSERT, UPDATE, DELETE (but NOT DDL)
- **DDL** (`--allow-ddl`): CREATE, DROP, ALTER, TRUNCATE (includes write)

## Output Format

All commands output structured JSON to stdout:

**Success:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "query",
  "data": { ... },
  "meta": {
    "execution_ms": 42,
    "rows_returned": 10
  }
}
```

**Error:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "DDL statements require --allow-ddl flag"
  }
}
```

### Error Codes

Plenum returns stable, machine-parseable error codes. Agents should check the `error.code` field for programmatic error handling:

| Code | Description | When It Occurs |
|------|-------------|----------------|
| `CAPABILITY_VIOLATION` | Operation blocked by capability constraints | Attempting write/DDL operations without appropriate flags |
| `CONNECTION_FAILED` | Database connection failed | Invalid credentials, unreachable host, or database doesn't exist |
| `QUERY_FAILED` | Query execution failed | SQL syntax errors, missing tables/columns, constraint violations |
| `INVALID_INPUT` | Malformed input or missing parameters | Missing required flags, invalid engine type, etc. |
| `ENGINE_ERROR` | Engine-specific database error | Database-specific errors wrapped for consistency |
| `CONFIG_ERROR` | Configuration file or connection registry error | Missing config file, invalid JSON, connection name not found |

**Example error handling:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "Write operations require --allow-write flag"
  }
}
```

Agents should:
1. Check `ok` field first (true = success, false = error)
2. Match on `error.code` for programmatic handling
3. Use `error.message` for logging/debugging (agent-appropriate, no sensitive data)

## MCP Integration

Plenum exposes functionality via MCP (Model Context Protocol) server:

```bash
# Start MCP server (hidden command, for AI agent use)
plenum mcp
```

Configure in your MCP client:
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

Each CLI command maps to an MCP tool:
- `connect` → Validate and save database connections
- `introspect` → Retrieve schema information
- `query` → Execute constrained SQL queries

## Architecture

Plenum is built around strict architectural principles:

### Core Principles

1. **No query language abstraction** - SQL remains vendor-specific
2. **Agent-first, machine-only** - No interactive UX, JSON-only output
3. **Explicit over implicit** - No inferred values, fail-fast on missing inputs
4. **Least privilege** - Read-only default, explicit capability requirements
5. **Determinism** - Identical inputs → identical outputs

### Security Model

Plenum's security boundary is **capability-based access control**, not SQL validation.

**Plenum enforces:**
- ✅ Operation type restrictions (read-only, write, DDL)
- ✅ Row limits (`max_rows`) and query timeouts (`timeout_ms`)
- ✅ Pre-execution validation (no capability bypasses)
- ✅ Credential security (best-effort, no intentional logging)

**Plenum does NOT enforce:**
- ❌ SQL injection prevention (agent's responsibility)
- ❌ Query semantic correctness
- ❌ Business logic constraints
- ❌ Data access policies (row-level security, column masking)

**Critical**: Agents must sanitize all user inputs before constructing SQL. Plenum assumes SQL passed to it is safe and passes it verbatim to database drivers.

#### Credential Security

Credentials are stored as **plaintext JSON** in config files:
- Local: `.plenum/config.json` (team-shareable)
- Global: `~/.config/plenum/connections.json` (user-private)

**Recommendations:**
- Use `password_env` for production (environment variables)
- Secure config files with OS-level permissions (`chmod 600`)
- Avoid `--password` CLI flag (visible in process listings)

#### Security Reporting

For detailed security documentation, threat model, and vulnerability reporting, see **[SECURITY.md](SECURITY.md)**.

To report security vulnerabilities, create a GitHub issue with the `security` label.

### Database Drivers

Plenum uses native, engine-specific drivers (NOT sqlx):
- **PostgreSQL**: `tokio-postgres`
- **MySQL**: `mysql_async`
- **SQLite**: `rusqlite`

This ensures maximum isolation between engines and preserves vendor-specific behavior.

## Building from Source

```bash
# Clone repository
git clone https://github.com/yourusername/plenum.git
cd plenum

# Build
cargo build --release

# Run tests
cargo test

# Check code quality
cargo fmt --check
cargo clippy --all-targets --all-features

# Install locally
cargo install --path .
```

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines.

### Project Structure

```
plenum/
├── src/
│   ├── lib.rs           # Library API for CLI and MCP
│   ├── main.rs          # CLI entry point
│   ├── engine/          # Database engine implementations (Phase 3-5)
│   ├── capability/      # Capability validation (Phase 1.4)
│   ├── config/          # Configuration management (Phase 1.5)
│   ├── output/          # JSON output envelopes (Phase 1.2)
│   └── error/           # Error handling (Phase 1.3)
├── CLAUDE.md            # Core principles and architecture
├── PROJECT_PLAN.md      # Implementation roadmap
├── RESEARCH.md          # Design decisions and rationale
└── PROBLEMS.md          # Resolved architectural issues
```

## Documentation

- [CLAUDE.md](CLAUDE.md) - Core principles and non-negotiable requirements
- [PROJECT_PLAN.md](PROJECT_PLAN.md) - Complete implementation roadmap
- [SECURITY.md](SECURITY.md) - Security model, threat analysis, and vulnerability reporting
- [RESEARCH.md](RESEARCH.md) - Design decisions, rationale, and research
- [PROBLEMS.md](PROBLEMS.md) - Architectural issues and resolutions
- [CONTRIBUTING.md](CONTRIBUTING.md) - Development guidelines

## Roadmap

Plenum has completed **Phase 8: Security Audit**.

**Recent Accomplishments:**
- Phase 7: MCP Server implementation complete ✅
- Phase 8: Comprehensive security audit complete ✅
- Critical security fixes applied (password masking, path panic prevention)
- SECURITY.md documentation created

See [PROJECT_PLAN.md](PROJECT_PLAN.md) for the complete implementation roadmap:
- Phase 0: Project Foundation ✅
- Phase 1: Core Architecture ✅
- Phase 2: CLI Foundation ✅
- Phase 3: SQLite Engine ✅
- Phase 4: PostgreSQL Engine ✅
- Phase 5: MySQL Engine ✅
- Phase 6: Integration & Polish ✅
- Phase 7: MCP Server ✅
- Phase 8: Security Audit ✅
- Phase 9: Release Preparation ← **Next Phase**

## Contributing

Contributions must adhere to Plenum's core principles. Before adding code, ask:

> **"Does this make autonomous agents safer, more deterministic, or more constrained?"**

If the answer is no, it does not belong in Plenum.

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed guidelines.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Acknowledgements

Plenum follows the architecture pattern established by [reflex-search](https://github.com/reflex-search/reflex) for MCP integration.
