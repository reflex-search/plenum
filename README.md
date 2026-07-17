# Plenum

**Agent-first database control CLI with least-privilege execution**

Plenum is a lightweight, deterministic database control tool designed specifically for autonomous AI coding agents. It provides a constrained, safe execution surface for database operations with read-only defaults and explicit capability requirements.

> **This is not a human-oriented database client.** If you're looking for a tool with interactive shells, autocomplete, or pretty-printed output, Plenum is not for you.

## What is Plenum?

Plenum enables AI agents to:
- **Introspect database schemas** safely and deterministically
- **Execute read-only SQL queries** with safety constraints
- **Ensure strict read-only access** - all write and DDL operations are rejected
- **Produce machine-parseable output** (JSON-only to stdout)

Plenum is exposed via a local MCP (Model Context Protocol) server, making it seamlessly integrable with AI agent frameworks.

## Key Features

- **Agent-First Design**: JSON-only output, no interactive UX, deterministic behavior
- **Vendor-Specific SQL**: No query abstraction layer - PostgreSQL SQL ≠ MySQL SQL ≠ SQLite SQL
- **Strictly Read-Only**: All write and DDL operations are rejected - guaranteed safe for AI agents
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

Manage database connection configurations (interactive or non-interactive).

#### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--list` | — | List saved connections for the project as JSON (no secrets emitted) |
| `--name <NAME>` | `"default"` | Connection name |
| `--project-path <PATH>` | current directory | Project path for connection lookup |
| `--engine <ENGINE>` | — | Database engine: `postgres`, `mysql`, or `sqlite` |
| `--host <HOST>` | — | Hostname (postgres/mysql) |
| `--port <PORT>` | — | Port (postgres/mysql) |
| `--user <USER>` | — | Username (postgres/mysql) |
| `--password <PASSWORD>` | — | Password (postgres/mysql; visible in process list — prefer `--password-env`) |
| `--password-env <VAR>` | — | Name of an environment variable whose value is the password |
| `--password-command <CMD>` | — | Shell command (`sh -c`) whose stdout is used as the password at connection time |
| `--keychain-service <SVC>` | — | OS keychain service name (pair with `--keychain-account`) |
| `--keychain-account <ACCT>` | — | OS keychain account name (pair with `--keychain-service`) |
| `--database <DATABASE>` | — | Database name (postgres/mysql) |
| `--file <FILE>` | — | SQLite file path |
| `--save <LOCATION>` | — | Save location: `local` (`.plenum/config.json`) or `global` (`~/.config/plenum/connections.json`) |
| `--ssl-mode <MODE>` | — | TLS/SSL mode: `disable`, `require`, `verify-ca`, or `verify-full` (postgres/mysql) |
| `--ssl-ca <PATH>` | — | PEM CA certificate for TLS verification (required for `verify-ca`/`verify-full`) |
| `--ssl-cert <PATH>` | — | PEM client certificate for mTLS (must be paired with `--ssl-key`) |
| `--ssl-key <PATH>` | — | PEM client private key for mTLS (must be paired with `--ssl-cert`) |
| `--test` | — | Test connection liveness and return server metadata without saving config |

#### Examples

```bash
# List saved connections for the current project
plenum connect --list

# Save a PostgreSQL connection globally (password via env var — recommended)
plenum connect --name prod --engine postgres --host db.example.com \
  --port 5432 --user readonly --password-env DB_PASSWORD \
  --database myapp --save global

# Save a PostgreSQL connection with full TLS verification
plenum connect --name prod-tls --engine postgres --host db.example.com \
  --user readonly --password-env DB_PASSWORD --database myapp \
  --ssl-mode verify-full --ssl-ca /etc/ssl/certs/ca.pem --save global

# Save a connection using OS keychain for the password
plenum connect --name staging --engine mysql --host staging.db.internal \
  --user agent --keychain-service MyApp --keychain-account db-staging \
  --database app --save local

# Save a connection using a shell command to retrieve the password (e.g. 1Password CLI)
plenum connect --name vault --engine postgres --host localhost \
  --user app --password-command "op read op://vault/db/password" \
  --database mydb --save global

# Save a SQLite connection
plenum connect --name dev --engine sqlite --file ./dev.db --save local

# Test a connection without saving
plenum connect --engine postgres --host localhost --user dev \
  --password-env DEV_DB_PASSWORD --database mydb --test
```

Connection configurations are stored:
- **Local**: `.plenum/config.json` (team-shareable, per-project)
- **Global**: `~/.config/plenum/connections.json` (per-user)

These files use **different schemas**:

**Local** (`.plenum/config.json`) — flat `ProjectConfig`, already scoped to this directory:
```json
{
  "connections": {
    "local": { "engine": "sqlite", "file": "./app.db" },
    "prod":  { "engine": "postgres", "host": "db.example.com", "port": 5432,
               "user": "readonly", "database": "myapp", "password_env": "PROD_DB_PASSWORD" }
  },
  "default": "local"
}
```

**Global** (`~/.config/plenum/connections.json`) — `ConnectionRegistry`, projects keyed by absolute path:
```json
{
  "projects": {
    "/home/user/myapp": {
      "connections": {
        "local": { "engine": "sqlite", "file": "./app.db" },
        "prod":  { "engine": "postgres", "host": "db.example.com", "port": 5432,
                   "user": "readonly", "database": "myapp", "password_env": "PROD_DB_PASSWORD" }
      },
      "default": "local"
    }
  }
}
```

The first connection created for a project is automatically set as the default. Run `plenum connect --list` to see the current registry.

### 2. `plenum introspect` - Schema Introspection

Inspect database schema and return structured JSON.

#### Connection flags

| Flag | Default | Description |
|------|---------|-------------|
| `--dsn <DSN>` | — | One-off connection URL (mutually exclusive with `--name` and explicit flags). Accepted schemes: `postgres://`, `postgresql://`, `mysql://`, `sqlite:` |
| `--name <NAME>` | `"default"` | Named connection from the saved registry |
| `--project-path <PATH>` | current directory | Project path for connection lookup |
| `--engine <ENGINE>` | — | Engine override: `postgres`, `mysql`, or `sqlite` |
| `--host <HOST>` | — | Host override |
| `--port <PORT>` | — | Port override |
| `--user <USER>` | — | Username override |
| `--password <PASSWORD>` | — | Password override |
| `--database <DATABASE>` | — | Database override |
| `--file <FILE>` | — | SQLite file override |
| `--ssl-mode <MODE>` | — | TLS/SSL mode: `disable`, `require`, `verify-ca`, or `verify-full` (postgres/mysql) |
| `--ssl-ca <PATH>` | — | PEM CA certificate (required for `verify-ca`/`verify-full`) |
| `--ssl-cert <PATH>` | — | PEM client certificate for mTLS (pair with `--ssl-key`) |
| `--ssl-key <PATH>` | — | PEM client private key for mTLS (pair with `--ssl-cert`) |

#### Operation flags

| Flag | Default | Description |
|------|---------|-------------|
| `--list-databases` | — | List all databases (requires a wildcard/no-database connection) |
| `--list-schemas` | — | List all schemas (PostgreSQL only) |
| `--list-tables` | — | List all table names |
| `--list-views` | — | List all view names |
| `--list-indexes [TABLE]` | — | List all indexes, optionally filtered to a single table |
| `--table <TABLE>` | — | Return full details for a specific table |
| `--view <VIEW>` | — | Return details for a specific view |
| `--target-database <DB>` | — | Switch to a different database before introspecting |
| `--schema <SCHEMA>` | — | Filter results to a specific schema (PostgreSQL/MySQL only) |
| `--diff-against <NAME>` | — | Structural schema diff against another named connection. Mutually exclusive with all other operation flags. Returns tables/views added, removed, and changed (columns, indexes, foreign keys, primary keys) |
| `--diff-against-project-path <PATH>` | current project | Project path for the `--diff-against` connection (for cross-project comparison) |

#### Detail flags (apply when using `--table`)

| Flag | Default | Description |
|------|---------|-------------|
| `--columns <true\|false>` | `true` | Include column details |
| `--primary-key <true\|false>` | `true` | Include primary key details |
| `--foreign-keys <true\|false>` | `true` | Include foreign key details |
| `--indexes <true\|false>` | `true` | Include index details |

#### Examples

```bash
# Full schema introspection using the default saved connection
plenum introspect --name prod

# One-off introspection via DSN (no saved config needed)
plenum introspect --dsn "postgres://user:pass@localhost/mydb"

# List all databases on a server
plenum introspect --name dev --list-databases

# List all schemas (PostgreSQL)
plenum introspect --name prod --list-schemas

# List tables in a specific schema
plenum introspect --name prod --list-tables --schema public

# List views
plenum introspect --name prod --list-views

# List indexes for a specific table
plenum introspect --name prod --list-indexes users

# Get full details for a table (columns, PKs, FKs, indexes)
plenum introspect --name prod --table users

# Get table details without indexes
plenum introspect --name prod --table users --indexes false

# Get details for a view
plenum introspect --name prod --view active_users

# Diff current schema against a saved baseline (useful in CI)
plenum introspect --name prod --diff-against baseline

# Cross-project schema diff
plenum introspect --name prod \
  --diff-against staging \
  --diff-against-project-path /other/project
```

### 3. `plenum query` - Read-Only Query Execution

Execute read-only SQL queries with safety constraints:

```bash
# Read-only query
plenum query --name prod --sql "SELECT * FROM users WHERE id = 1"

# With row limit (recommended for large tables)
plenum query --name prod --sql "SELECT * FROM large_table" \
  --max-rows 100 --timeout-ms 5000

# Introspection queries
plenum query --name prod --sql "SHOW TABLES"
plenum query --name prod --sql "DESCRIBE users"

# Complex query with joins
plenum query --name prod --sql "
  SELECT u.name, o.total
  FROM users u
  JOIN orders o ON u.id = o.user_id
  WHERE o.status = 'completed'
" --max-rows 50
```

**Read-Only Enforcement:**
- ✅ SELECT queries are permitted
- ✅ SHOW, DESCRIBE, PRAGMA statements are permitted
- ✅ EXPLAIN queries are permitted
- ❌ INSERT, UPDATE, DELETE operations are **rejected**
- ❌ CREATE, DROP, ALTER operations are **rejected**

**For write operations:** Plenum will reject the query with a helpful error message. Construct the SQL and present it to the user for manual execution.

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
    "message": "Plenum is read-only and cannot execute this query. Please run this query manually:\n\nCREATE TABLE users (id INT)"
  }
}
```

### JSON Schemas

Machine-consumable JSON Schema (Draft 7) files for every output envelope are checked into [`schemas/`](schemas/):

| File | Validates |
|------|-----------|
| [`schemas/error_envelope.json`](schemas/error_envelope.json) | All error responses |
| [`schemas/connect_success.json`](schemas/connect_success.json) | `plenum connect` success response |
| [`schemas/introspect_success.json`](schemas/introspect_success.json) | `plenum introspect` success response |
| [`schemas/query_success.json`](schemas/query_success.json) | `plenum query` success response |

All schemas include `meta.contract_version` — agents should check this field to guard against silent breaking changes.

Schemas are generated directly from the Rust types via `schemars`. To regenerate after a type change:

```bash
cargo run --bin generate-schemas
```

A drift test (`tests/schema_drift.rs`) fails CI if checked-in schemas diverge from the live types.

### Error Codes

Plenum returns stable, machine-parseable error codes. Agents should check the `error.code` field for programmatic error handling:

| Code | Description | When It Occurs |
|------|-------------|----------------|
| `CAPABILITY_VIOLATION` | Operation blocked - Plenum is read-only | Attempting any write or DDL operations (INSERT, UPDATE, DELETE, CREATE, DROP, ALTER, etc.) |
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
    "message": "Plenum is read-only and cannot execute this query. Please run this query manually:\n\nINSERT INTO users (name) VALUES ('Alice')"
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

The five core invariants governing all design decisions are defined in [CLAUDE.md](CLAUDE.md).

### Security Model

Plenum's security boundary is **strict read-only enforcement**, not SQL validation. For the full security model — what Plenum enforces, agent responsibilities, and what falls outside Plenum's scope — see [CLAUDE.md](CLAUDE.md).

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
│   ├── bin/
│   │   └── generate_schemas.rs  # Schema generation binary
│   ├── engine/          # Database engine implementations
│   ├── capability/      # Capability validation
│   ├── config/          # Configuration management
│   ├── output/          # JSON output envelopes
│   └── error/           # Error handling
├── schemas/             # JSON Schema files (generated, checked in)
│   ├── error_envelope.json
│   ├── connect_success.json
│   ├── introspect_success.json
│   └── query_success.json
├── tests/
│   ├── schema_drift.rs  # Fails if schemas diverge from types
│   └── ...
├── CLAUDE.md            # Canonical agent rules, invariants, and non-negotiable requirements
├── PROJECT_PLAN.md      # Implementation roadmap
├── RESEARCH.md          # Design decisions and rationale
└── PROBLEMS.md          # Resolved architectural issues
```

## Documentation

- [CLAUDE.md](CLAUDE.md) - **Canonical** agent rules, core invariants, and non-negotiable requirements
- [ARCHITECTURE.md](ARCHITECTURE.md) - Internal design, module structure, data flow, and design rationale
- [CONTRIBUTING.md](CONTRIBUTING.md) - Development guidelines
- [SECURITY.md](SECURITY.md) - Security model, threat analysis, and vulnerability reporting
- [PROJECT_PLAN.md](PROJECT_PLAN.md) - Complete implementation roadmap
- [RESEARCH.md](RESEARCH.md) - Design decisions, rationale, and research
- [PROBLEMS.md](PROBLEMS.md) - Architectural issues and resolutions

## Roadmap

Plenum has completed **Phase 8: Security Audit** and is actively progressing through **Phase 9: Release Preparation**.

**Completed phases:**
- Phase 0: Project Foundation ✅
- Phase 1: Core Architecture ✅
- Phase 2: CLI Foundation ✅
- Phase 3: SQLite Engine ✅
- Phase 4: PostgreSQL Engine ✅
- Phase 5: MySQL Engine ✅
- Phase 6: Integration & Polish ✅
- Phase 7: MCP Server ✅
- Phase 8: Security Audit ✅

**Phase 9: Release Preparation — In Progress**

Key deliverables shipped in this phase:

| Area | Status | Details |
|------|--------|---------|
| Live test infrastructure | ✅ Done | Docker Compose harness, vendor seeds, `test-live.sh`; MySQL 8.0/8.4 and PostgreSQL 16 matrices ([REF-275](/REF/issues/REF-275), [REF-276](/REF/issues/REF-276), [REF-277](/REF/issues/REF-277), [REF-278](/REF/issues/REF-278), [REF-279](/REF/issues/REF-279)) |
| Output schema versioning | ✅ Done | JSON Schemas (Draft 7) for all envelopes; schema-drift CI gate ([REF-265](/REF/issues/REF-265)) |
| Introspect enhancements | ✅ Done | Column comments, row estimates, `--diff-against` schema diff ([REF-263](/REF/issues/REF-263), [REF-281](/REF/issues/REF-281)) |
| Connection improvements | ✅ Done | `--dsn`, `--list`, `--test`, `password_command`, `keychain_entry` ([REF-267](/REF/issues/REF-267)–[REF-272](/REF/issues/REF-272)) |
| Query safety | ✅ Done | `max_bytes` byte-budget guard, session-level driver read-only enforcement ([REF-261](/REF/issues/REF-261), [REF-269](/REF/issues/REF-269)) |
| Security hardening | ✅ Done | CTE DML bypass rejection, SQLite PRAGMA allowlist tightened ([REF-41](/REF/issues/REF-41), [REF-44](/REF/issues/REF-44)) |
| Versioned release | ⏳ Pending | crates.io and npm publish |

See [PROJECT_PLAN.md](PROJECT_PLAN.md) for the full implementation roadmap and [CHANGELOG.md](CHANGELOG.md) for a categorized history of all changes.

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
