# Security Documentation

## Table of Contents

1. [Security Model](#security-model)
2. [Threat Model](#threat-model)
3. [Capability Enforcement](#capability-enforcement)
4. [Credential Security](#credential-security)
5. [SQL Injection & Query Validation](#sql-injection--query-validation)
6. [Known Security Issues](#known-security-issues)
7. [Security Recommendations](#security-recommendations)
8. [Reporting Security Vulnerabilities](#reporting-security-vulnerabilities)

---

## Security Model

### What Plenum Enforces

Plenum's security boundary is **capability-based access control**:

- ✅ **Operation type restrictions** (read-only, write, DDL)
- ✅ **Row limits** (`max_rows`)
- ✅ **Query timeouts** (`timeout_ms`)
- ✅ **Pre-execution validation** (no capability bypasses)

### What Plenum Does NOT Enforce

Plenum is designed as a **constrained execution layer**, not a security sandbox:

- ❌ **SQL injection prevention** (agent's responsibility)
- ❌ **Query semantic correctness**
- ❌ **Business logic constraints**
- ❌ **Data access policies** (row-level security, column masking)
- ❌ **Rate limiting**
- ❌ **Audit logging**

**Design Principle**: Plenum assumes SQL passed to it is safe. It provides capability constraints, not query validation.

---

## Threat Model

### Assumptions

1. **Trusted Agent**: The calling AI agent is assumed to be trustworthy
2. **Untrusted User Input**: User inputs must be sanitized by the agent before constructing SQL
3. **Local Machine Security**: The host machine is assumed to be secured at the OS level
4. **Single-User Environment**: Plenum is designed for development/automation, not multi-tenant production

### Attack Vectors

**In Scope:**
- Capability bypass attempts
- Privilege escalation via SQL (mitigated by capability enforcement)
- Resource exhaustion (mitigated by `max_rows` and `timeout_ms`)

**Out of Scope:**
- SQL injection via user inputs (agent must sanitize)
- Credential theft from config files (OS-level protection required)
- Side-channel attacks
- Network-level attacks (MCP over stdio only)

---

## Capability Enforcement

### Capability Hierarchy

```
┌─────────────────────────────────────────┐
│  DDL (--allow-ddl)                      │
│  - CREATE, DROP, ALTER, TRUNCATE         │
│  - Implicitly grants write permissions  │
│                                         │
│  ┌───────────────────────────────────┐  │
│  │  Write (--allow-write)             │  │
│  │  - INSERT, UPDATE, DELETE          │  │
│  │  - Does NOT enable DDL             │  │
│  │                                    │  │
│  │  ┌─────────────────────────────┐  │  │
│  │  │  Read-Only (default)        │  │  │
│  │  │  - SELECT only              │  │  │
│  │  │  - No flags required        │  │  │
│  │  └─────────────────────────────┘  │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

### Enforcement Points

Capability validation occurs at **two entry points**:

1. **CLI** (`src/main.rs:782`): Validates before calling engine
2. **MCP Server** (`src/mcp.rs:485`): Validates before calling engine

Every query execution path calls `validate_query()` before database interaction:

- **SQLite** (`src/engine/sqlite/mod.rs:138`)
- **PostgreSQL** (`src/engine/postgres/mod.rs:145`)
- **MySQL** (`src/engine/mysql/mod.rs:165`)

**No bypass paths exist.** Capability enforcement has been audited and verified complete.

### Validation Process

```
User SQL → validate_query() → Categorize → Check Capabilities → Engine
                               ↓
                    (Read-Only/Write/DDL)
                               ↓
                    Capability Check (fail-fast)
```

---

## Credential Security

### Storage Locations

Credentials are stored as **plaintext JSON**:

- **Local**: `.plenum/config.json` (team-shareable, project-specific)
- **Global**: `~/.config/plenum/connections.json` (user-private)

**Security Responsibility**: The user is responsible for securing these files at the OS level (file permissions, disk encryption, etc.).

### Environment Variable Support

For production use, passwords can be stored in environment variables:

```json
{
  "connections": {
    "prod": {
      "engine": "postgres",
      "host": "db.example.com",
      "port": 5432,
      "user": "app_user",
      "password_env": "DB_PASSWORD"
    }
  }
}
```

### CLI Password Visibility

**Warning**: Passwords passed via `--password` flag are visible in:
- Process listings (`ps aux`)
- Shell history
- System logs

**Recommendation**: Use `password_env` for automation, or interactive prompts for manual use.

### MCP Credential Passing

When using the MCP server, credentials are passed per-invocation via JSON-RPC:

```json
{
  "name": "query",
  "arguments": {
    "sql": "SELECT * FROM users",
    "engine": "postgres",
    "host": "localhost",
    "port": 5432,
    "user": "app_user",
    "password": "secret"
  }
}
```

**Security Note**: MCP communication over stdio is local-only (no network exposure).

### MCP Credential References

The MCP `connect` tool rejects an inline plaintext `password` with `CAPABILITY_VIOLATION`. Agents reference the secret instead:

- `password_env` — the name of an environment variable that holds the password.
- `keychain_service` + `keychain_account` — an OS keychain entry (both are required together).

**`password_command` is rejected on every MCP tool** (`connect`, `introspect`, `query`) with `CAPABILITY_VIOLATION`. It executes a shell command via `sh -c`. Accepting it from tool arguments would give any connected agent arbitrary command execution on the host. The CLI flag `--password-command` and the `plenum connect` wizard still offer it, because a human types the command there.

**Residual risk — saved configs:** a connection already saved with `password_command` still runs that command when the MCP server resolves it. The command comes from a config file, not from tool arguments. Review `password_command` entries in `.plenum/config.json` before you commit or pull that file, because it is team-shareable.

### MCP Connection Binding

`plenum mcp` binds its default connection at server start:

- `--project-path <path>` — resolve config for that project, independent of the server's working directory.
- `--name <connection>` — select a named connection within that project.
- `--dsn-env <VAR>` — read the DSN from exactly the named environment variable. Plenum never reads `DATABASE_URL`, `PGPASSWORD`, or any other ambient variable on its own.

`--dsn-env` cannot be combined with `--project-path` or `--name`. Connection fields in a tool call override the binding.

---

## SQL Injection & Query Validation

### Agent Responsibility

**Plenum does NOT validate SQL for safety.** SQL is passed verbatim to database drivers.

The calling agent MUST:
1. Sanitize all user inputs before constructing SQL
2. Use parameterized queries where possible
3. Validate query semantics before passing to Plenum
4. Apply business logic constraints

### SQL Processing

Plenum's `validate_query()` function:
- ✅ Categorizes queries (read-only/write/DDL)
- ✅ Enforces capability constraints
- ❌ Does NOT sanitize SQL
- ❌ Does NOT prevent SQL injection
- ❌ Does NOT modify SQL (passed verbatim to drivers)

**Example** (unsafe agent code):
```rust
// UNSAFE: User input directly in SQL
let sql = format!("SELECT * FROM users WHERE name = '{}'", user_input);
plenum query --sql "$sql"
```

**Example** (safe agent code):
```rust
// SAFE: Use database-specific parameterized queries
let sql = "SELECT * FROM users WHERE name = $1";
// Then sanitize/validate before calling Plenum
```

---

## Known Security Issues

### CRITICAL Issues

#### 1. Interactive Password Not Hidden
**Location**: `src/main.rs:503-506`

**Issue**: Interactive password prompt uses `.interact_text()` instead of `.interact_password()`, causing passwords to echo to the screen.

**Impact**: Passwords visible in terminal, screen recordings, shoulder surfing.

**Mitigation**: Use CLI flag with environment variable, or fix by using `.interact_password()`.

**Status**: Identified, pending fix.

---

#### 2. SQLite Path Panic Risk
**Location**: `src/engine/sqlite/mod.rs:49, 89, 147`

**Issue**: Code uses `file_path.to_str().unwrap()` which panics if the file path contains non-UTF-8 characters.

```rust
let conn = open_connection(file_path.to_str().unwrap(), true)?; // PANICS on non-UTF-8
```

**Impact**: CLI crashes on Windows file paths with special characters, emoji, or certain Unicode characters.

**Mitigation**: Avoid non-UTF-8 file paths, or fix by handling `Option<&str>` properly.

**Status**: Identified, pending fix.

---

### HIGH Risk Issues

#### 3. PostgreSQL Connection Error Leakage
**Location**: `src/engine/postgres/mod.rs:56, 125, 158`

**Issue**: PostgreSQL driver errors are logged to stderr via `eprintln!()` and can contain connection strings with credentials.

```rust
eprintln!("PostgreSQL connection error: {}", e);
```

**Impact**: Credentials may appear in stderr output, logs, or terminal scrollback.

**Mitigation**: Sanitize error messages before logging, or disable stderr output.

**Status**: Identified, pending fix.

---

#### 4. Database Driver Errors Expose Credentials
**Location**: All three engines (postgres/mysql/sqlite)

**Issue**: Database driver errors are wrapped with `format!("Failed to connect: {}", e)` and returned in JSON output. Driver errors can contain:
- Connection strings with passwords
- Host/port information
- Database/user names

**Example**:
```json
{
  "ok": false,
  "error": {
    "code": "CONNECTION_FAILED",
    "message": "Failed to connect to PostgreSQL: FATAL: password authentication failed for user 'admin' (connection: 'postgresql://admin:SECRET@host:5432/db')"
  }
}
```

**Impact**: Credentials exposed in JSON error output (stdout, logs, MCP responses).

**Mitigation**: Sanitize driver errors to remove connection details before wrapping in `PlenumError`.

**Status**: Identified, pending fix.

---

#### 5. MCP Server Error Leakage
**Location**: `src/main.rs:851`

**Issue**: MCP server errors are logged to stderr via `eprintln!()` without sanitization.

```rust
eprintln!("MCP server error: {}", e);
```

**Impact**: Errors from MCP tools (including credential-related errors) may leak to stderr.

**Mitigation**: Sanitize errors before logging.

**Status**: Identified, pending fix.

---

### MEDIUM Risk Issues

#### 6. Config Resolution Error Leakage
**Location**: `src/config/mod.rs:235`

**Issue**: Connection resolution errors are logged to stderr and may contain credential information.

```rust
eprintln!("Warning: Could not resolve connection '{}': {}", name, e.message());
```

**Impact**: Environment variable resolution errors or config parsing errors may expose credential metadata.

**Mitigation**: Sanitize error messages before logging.

**Status**: Identified, pending fix.

---

#### 7. HashMap unwrap() Fragility
**Location**: `src/engine/sqlite/mod.rs:279-280`, `src/engine/postgres/mod.rs:399`, `src/engine/mysql/mod.rs:431`

**Issue**: Code uses `HashMap::get_mut().unwrap()` after `or_insert_with()`, which is logically safe but fragile and unclear to the compiler.

```rust
fk_map.entry(id).or_insert_with(|| (ref_table.clone(), Vec::new(), Vec::new()));
fk_map.get_mut(&id).unwrap().1.push(from_col); // Fragile
```

**Impact**: Code is hard to refactor and may panic if HashMap implementation changes.

**Mitigation**: Use Entry API pattern or pattern matching instead of unwrap().

**Status**: Identified, low priority.

---

## Security Recommendations

### For Users

1. **Use Environment Variables for Production**:
   ```json
   {
     "connections": {
       "prod": {
         "password_env": "DB_PASSWORD"
       }
     }
   }
   ```

2. **Secure Config Files**:
   ```bash
   chmod 600 ~/.config/plenum/connections.json
   chmod 600 .plenum/config.json
   ```

3. **Use Read-Only by Default**:
   ```bash
   plenum query --sql "SELECT * FROM users"  # Safe (read-only)
   plenum query --sql "DELETE FROM users" --allow-write  # Explicit
   ```

4. **Avoid CLI Passwords**:
   ```bash
   # Bad (visible in ps/history)
   plenum query --password "secret" --sql "..."

   # Good (environment variable)
   export DB_PASSWORD="secret"
   plenum query --password-env DB_PASSWORD --sql "..."
   ```

5. **Limit Exposure**:
   - Use `--max-rows` for unknown queries
   - Use `--timeout-ms` to prevent long-running operations
   - Start with read-only, escalate only when needed

---

### For Agent Developers

1. **Sanitize All User Inputs**:
   ```python
   # UNSAFE
   sql = f"SELECT * FROM users WHERE name = '{user_input}'"

   # SAFE
   sql = "SELECT * FROM users WHERE name = $1"  # PostgreSQL
   params = [user_input]  # Pass separately, let agent validate
   ```

2. **Use Least Privilege**:
   ```python
   # Default: read-only
   result = plenum.query(sql="SELECT * FROM users")

   # Explicit escalation
   result = plenum.query(sql="INSERT INTO logs ...", allow_write=True)
   ```

3. **Validate Before Execution**:
   ```python
   # Check query intent before calling Plenum
   if is_destructive(sql):
       confirm_with_user()

   result = plenum.query(sql=sql, allow_write=True)
   ```

4. **Handle Errors Securely**:
   ```python
   try:
       result = plenum.query(...)
   except PlenumError as e:
       # Don't log raw error messages (may contain credentials)
       log.error(f"Query failed with error code: {e.code}")
   ```

---

## Reporting Security Vulnerabilities

If you discover a security vulnerability in Plenum, please report it responsibly:

**Contact**: [Create a GitHub Issue](https://github.com/anthropics/plenum/issues) with the `security` label

**Information to Include**:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if applicable)

**Response Timeline**:
- **Acknowledgment**: Within 48 hours
- **Initial Assessment**: Within 1 week
- **Fix Timeline**: Depends on severity (critical: <7 days, high: <30 days)

---

## Security Testing

### Current Test Coverage

**Capability Enforcement**: ✅ Comprehensive
- 25+ tests in `src/capability/mod.rs`
- All capability bypass scenarios covered
- Engine-specific validation tests

**Panic Safety**: 🔄 In Progress
- Unwrap/expect usage audited
- Known panic risks identified
- Fixes pending

**Credential Handling**: ⚠️ Needs Improvement
- Credential storage tested
- Environment variable resolution tested
- Leakage prevention NOT tested

**SQL Injection**: ❌ Not Applicable
- Plenum does not validate SQL safety
- Agent's responsibility to test

### Running Security Tests

```bash
# All tests
cargo test --all-features

# Capability tests only
cargo test --lib capability

# Integration tests (requires database servers)
cargo test --features postgres,mysql -- --ignored
```

---

## Changelog

### Phase 8: Security Audit (2025-01-08)

**Completed**:
- ✅ Capability enforcement audit (no bypass paths found)
- ✅ Panic safety audit (7 issues identified)
- ✅ Credential handling audit (5 issues identified)
- ✅ Error message disclosure audit (4 issues identified)
- ✅ SQL verbatim verification (confirmed)

**Findings**:
- 2 CRITICAL issues (interactive password, SQLite panic)
- 3 HIGH issues (PostgreSQL leakage, driver error leakage, MCP leakage)
- 2 MEDIUM issues (config error leakage, HashMap unwrap fragility)

**Next Steps**:
- Fix identified security issues
- Add credential leakage tests
- Implement error message sanitization
- Add security section to README

---

**Last Updated**: 2025-01-08
**Security Audit Status**: Phase 8 Complete, Fixes Pending
