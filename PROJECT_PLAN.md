# PROJECT_PLAN.md

## Project: Plenum - Agent-First Database Control CLI

**Version:** MVP 1.0  
**Target Completion:** TBD  
**Implementation Language:** Rust

---

## Phase 0: Project Foundation

### 0.1 Repository Setup
- [ ] Initialize Rust project with Cargo
- [ ] Configure `.gitignore` for Rust projects
- [ ] Set up basic project structure
- [ ] Configure Cargo.toml with project metadata
- [ ] Add LICENSE file
- [ ] Create initial README.md with project description

### 0.2 Development Environment
- [ ] Define Rust toolchain version (stable/nightly)
- [ ] Configure rustfmt.toml for code formatting
- [ ] Configure clippy rules for linting
- [ ] Set up CI/CD pipeline configuration (GitHub Actions)
- [ ] Document build/test commands

### 0.3 Dependency Assessment
- [ ] Research and select database driver crates:
  - [ ] PostgreSQL: `tokio-postgres` or `sqlx`
  - [ ] MySQL: `mysql_async` or `sqlx`
  - [ ] SQLite: `rusqlite` or `sqlx`
- [ ] Select JSON serialization library: `serde_json`
- [ ] Select CLI framework: `clap`
- [ ] Select error handling: `thiserror` or `anyhow`
- [ ] Document dependency rationale

---

## Phase 1: Core Architecture

### 1.1 Define Core Traits
- [ ] Create `src/engine/mod.rs` with trait definitions
- [ ] Define `DatabaseEngine` trait
  - [ ] `fn connect(config: ConnectionConfig) -> Result<Self>`
  - [ ] `fn introspect(&self) -> Result<SchemaInfo>`
  - [ ] `fn execute(&self, query: Query, caps: Capabilities) -> Result<QueryResult>`
- [ ] Define `Capabilities` struct
  - [ ] `read_only: bool`
  - [ ] `allow_write: bool`
  - [ ] `allow_ddl: bool`
  - [ ] `max_rows: Option<usize>`
  - [ ] `timeout_ms: Option<u64>`
- [ ] Define `ConnectionConfig` struct
- [ ] Define `SchemaInfo` struct
- [ ] Define `QueryResult` struct

### 1.2 Define Output Envelope Types
- [ ] Create `src/output/mod.rs`
- [ ] Define `SuccessEnvelope<T>` struct
  - [ ] `ok: bool` (always true)
  - [ ] `engine: String`
  - [ ] `command: String`
  - [ ] `data: T`
  - [ ] `meta: Metadata`
- [ ] Define `ErrorEnvelope` struct
  - [ ] `ok: bool` (always false)
  - [ ] `engine: String`
  - [ ] `command: String`
  - [ ] `error: ErrorInfo`
- [ ] Define `ErrorInfo` struct
  - [ ] `code: String`
  - [ ] `message: String`
- [ ] Define `Metadata` struct
  - [ ] `execution_ms: u64`
  - [ ] `rows_returned: Option<usize>`
- [ ] Implement `Serialize` for all envelope types

### 1.3 Error Handling Infrastructure
- [ ] Create `src/error/mod.rs`
- [ ] Define `PlenumError` enum with variants:
  - [ ] `CapabilityViolation(String)`
  - [ ] `ConnectionFailed(String)`
  - [ ] `QueryFailed(String)`
  - [ ] `InvalidInput(String)`
  - [ ] `EngineError { engine: String, detail: String }`
- [ ] Implement error code mapping
- [ ] Implement conversion to `ErrorEnvelope`
- [ ] Ensure no panics across public boundaries

### 1.4 Capability Validation
- [ ] Create `src/capability/mod.rs`
- [ ] Implement capability validator
- [ ] Define SQL statement categorization:
  - [ ] Read-only: SELECT
  - [ ] Write: INSERT, UPDATE, DELETE
  - [ ] DDL: CREATE, DROP, ALTER, TRUNCATE, RENAME
- [ ] Implement pre-execution capability checks
- [ ] Handle MySQL implicit commit cases
- [ ] Add capability validation unit tests

---

## Phase 2: CLI Foundation

### 2.1 CLI Structure
- [ ] Create `src/main.rs` with CLI entry point
- [ ] Set up `clap` with three subcommands:
  - [ ] `connect`
  - [ ] `introspect`
  - [ ] `query`
- [ ] Define common flags:
  - [ ] `--engine <postgres|mysql|sqlite>`
  - [ ] Connection string parameters
- [ ] Ensure stdout is JSON-only
- [ ] Redirect logs to stderr (if needed for debugging)

### 2.2 Connect Command
- [ ] Define `connect` subcommand arguments:
  - [ ] `--engine <ENGINE>` (required)
  - [ ] `--host <HOST>` (for postgres/mysql)
  - [ ] `--port <PORT>` (for postgres/mysql)
  - [ ] `--user <USER>` (for postgres/mysql)
  - [ ] `--password <PASSWORD>` (for postgres/mysql)
  - [ ] `--database <DATABASE>` (for postgres/mysql)
  - [ ] `--file <PATH>` (for sqlite)
- [ ] Implement connection validation
- [ ] Return JSON success/error envelope
- [ ] Do NOT maintain persistent connections

### 2.3 Introspect Command
- [ ] Define `introspect` subcommand arguments:
  - [ ] Same connection parameters as `connect`
  - [ ] `--schema <SCHEMA>` (optional filter)
- [ ] Implement schema introspection orchestration
- [ ] Return JSON with schema information:
  - [ ] Tables
  - [ ] Columns (name, type, nullable)
  - [ ] Primary keys
  - [ ] Foreign keys
  - [ ] Indexes
- [ ] Include execution metadata

### 2.4 Query Command
- [ ] Define `query` subcommand arguments:
  - [ ] Same connection parameters as `connect`
  - [ ] `--sql <SQL>` or `--file <PATH>` (required)
  - [ ] `--read-only` (default: true)
  - [ ] `--allow-write` (explicit flag)
  - [ ] `--allow-ddl` (explicit flag)
  - [ ] `--max-rows <N>` (optional)
  - [ ] `--timeout-ms <MS>` (optional)
- [ ] Validate capabilities before execution
- [ ] Execute query through engine trait
- [ ] Return JSON with query results
- [ ] Include execution metadata

---

## Phase 3: PostgreSQL Engine

### 3.1 PostgreSQL Connection
- [ ] Create `src/engine/postgres/mod.rs`
- [ ] Implement `DatabaseEngine` trait for PostgreSQL
- [ ] Implement connection establishment
- [ ] Handle connection errors with proper wrapping
- [ ] Detect and include PostgreSQL version in metadata

### 3.2 PostgreSQL Introspection
- [ ] Query `information_schema.tables`
- [ ] Query `information_schema.columns`
- [ ] Query primary key information
- [ ] Query foreign key information
- [ ] Query index information
- [ ] Format results as `SchemaInfo`
- [ ] Handle PostgreSQL-specific edge cases

### 3.3 PostgreSQL Query Execution
- [ ] Implement query execution with capability checks
- [ ] Parse result sets into JSON-safe format
- [ ] Handle PostgreSQL data types:
  - [ ] Numeric types
  - [ ] String types
  - [ ] Date/time types
  - [ ] Boolean types
  - [ ] NULL values
  - [ ] Arrays (as JSON arrays)
  - [ ] JSON/JSONB (as nested JSON)
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time

### 3.4 PostgreSQL Testing
- [ ] Set up test database (in-memory or docker)
- [ ] Write capability enforcement tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 4: MySQL Engine

### 4.1 MySQL Connection
- [ ] Create `src/engine/mysql/mod.rs`
- [ ] Implement `DatabaseEngine` trait for MySQL
- [ ] Implement connection establishment
- [ ] Detect MySQL version explicitly
- [ ] Handle MariaDB detection and versioning
- [ ] Handle connection errors with proper wrapping

### 4.2 MySQL Introspection
- [ ] Query `information_schema.tables`
- [ ] Query `information_schema.columns`
- [ ] Query primary key information
- [ ] Query foreign key information
- [ ] Query index information
- [ ] Avoid non-standard INFORMATION_SCHEMA extensions
- [ ] Format results as `SchemaInfo`
- [ ] Handle MySQL-specific edge cases
- [ ] Handle storage engine variations

### 4.3 MySQL Query Execution
- [ ] Implement query execution with capability checks
- [ ] Handle implicit commit detection (DDL statements)
- [ ] Parse result sets into JSON-safe format
- [ ] Handle MySQL data types:
  - [ ] Numeric types (INT, DECIMAL, FLOAT, etc.)
  - [ ] String types (VARCHAR, TEXT, CHAR, etc.)
  - [ ] Date/time types (DATE, DATETIME, TIMESTAMP)
  - [ ] Boolean/TINYINT(1)
  - [ ] NULL values
  - [ ] ENUM and SET types
  - [ ] Binary types
  - [ ] JSON type (MySQL 5.7+)
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time
- [ ] Surface version-specific behaviors in metadata

### 4.4 MySQL Testing
- [ ] Set up test database (docker with specific versions)
- [ ] Test against multiple MySQL versions
- [ ] Write capability enforcement tests
- [ ] Write DDL implicit commit tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 5: SQLite Engine

### 5.1 SQLite Connection
- [ ] Create `src/engine/sqlite/mod.rs`
- [ ] Implement `DatabaseEngine` trait for SQLite
- [ ] Implement file-based connection
- [ ] Implement in-memory connection (`:memory:`)
- [ ] Detect SQLite version
- [ ] Handle connection errors with proper wrapping

### 5.2 SQLite Introspection
- [ ] Query `sqlite_master` table
- [ ] Use `PRAGMA table_info()` for column information
- [ ] Use `PRAGMA foreign_key_list()` for foreign keys
- [ ] Use `PRAGMA index_list()` for indexes
- [ ] Format results as `SchemaInfo`
- [ ] Handle SQLite-specific edge cases

### 5.3 SQLite Query Execution
- [ ] Implement query execution with capability checks
- [ ] Parse result sets into JSON-safe format
- [ ] Handle SQLite data types (dynamic typing):
  - [ ] INTEGER
  - [ ] REAL
  - [ ] TEXT
  - [ ] BLOB (as base64 or hex)
  - [ ] NULL
- [ ] Implement timeout enforcement
- [ ] Implement row limit enforcement
- [ ] Track execution time

### 5.4 SQLite Testing
- [ ] Set up test database (in-memory)
- [ ] Write capability enforcement tests
- [ ] Write introspection tests
- [ ] Write query execution tests
- [ ] Write error handling tests
- [ ] Write JSON output snapshot tests

---

## Phase 6: Integration & Polish

### 6.1 Cross-Engine Testing
- [ ] Create integration test suite
- [ ] Test identical queries across all engines
- [ ] Verify JSON output consistency
- [ ] Test capability enforcement across engines
- [ ] Test error handling across engines
- [ ] Verify no cross-engine behavior leaks

### 6.2 Output Validation
- [ ] Verify all stdout is valid JSON
- [ ] Verify no logs appear on stdout
- [ ] Verify success envelope schema
- [ ] Verify error envelope schema
- [ ] Verify metadata consistency
- [ ] Create JSON schema files for validation

### 6.3 Edge Case Handling
- [ ] Test empty result sets
- [ ] Test very large result sets
- [ ] Test malformed SQL
- [ ] Test connection failures
- [ ] Test timeout scenarios
- [ ] Test max_rows enforcement
- [ ] Test invalid capability combinations
- [ ] Test NULL handling across all engines
- [ ] Test special characters in data

### 6.4 Performance Baseline
- [ ] Benchmark connection time for each engine
- [ ] Benchmark introspection time for each engine
- [ ] Benchmark query execution for each engine
- [ ] Document performance characteristics
- [ ] Identify performance bottlenecks (if any)

### 6.5 Documentation
- [ ] Update README.md with:
  - [ ] Project overview
  - [ ] Installation instructions
  - [ ] Usage examples for each command
  - [ ] Capability model explanation
  - [ ] Error code reference
- [ ] Create EXAMPLES.md with:
  - [ ] Connect examples for each engine
  - [ ] Introspect examples
  - [ ] Query examples (read-only, write, DDL)
  - [ ] Error handling examples
- [ ] Create ARCHITECTURE.md with:
  - [ ] System architecture diagram
  - [ ] Trait hierarchy
  - [ ] Data flow diagrams
  - [ ] Engine isolation explanation

---

## Phase 7: MCP Server

### 7.1 MCP Server Foundation
- [ ] Research MCP server implementation in Rust
- [ ] Create `src/mcp/mod.rs`
- [ ] Define MCP server structure
- [ ] Implement server initialization
- [ ] Configure local socket/port

### 7.2 Tool Mapping
- [ ] Map `plenum connect` to MCP tool
  - [ ] Define tool schema
  - [ ] Pass connection parameters
  - [ ] Return JSON response
- [ ] Map `plenum introspect` to MCP tool
  - [ ] Define tool schema
  - [ ] Pass connection + introspection parameters
  - [ ] Return JSON response
- [ ] Map `plenum query` to MCP tool
  - [ ] Define tool schema
  - [ ] Pass connection + query + capability parameters
  - [ ] Return JSON response

### 7.3 Credential Handling
- [ ] Implement per-invocation credential passing
- [ ] Ensure credentials are not logged
- [ ] Ensure credentials are not persisted
- [ ] Validate credential formats
- [ ] Handle missing credentials gracefully

### 7.4 Stateless Design
- [ ] Verify no shared state between invocations
- [ ] Verify no persistent connections
- [ ] Verify no caching
- [ ] Document stateless design decisions

### 7.5 MCP Testing
- [ ] Create MCP client test harness
- [ ] Test tool invocation for each command
- [ ] Test error propagation through MCP
- [ ] Test concurrent tool invocations
- [ ] Test credential passing security
- [ ] Verify JSON response format

---

## Phase 8: Security Audit

### 8.1 Capability Enforcement Audit
- [ ] Audit all capability check points
- [ ] Verify checks occur before execution
- [ ] Test capability bypass attempts
- [ ] Verify DDL detection across engines
- [ ] Verify write detection across engines
- [ ] Document capability enforcement guarantees

### 8.2 SQL Injection Prevention
- [ ] Verify parameterized queries where applicable
- [ ] Document SQL injection surface area
- [ ] Note that Plenum passes raw SQL (by design)
- [ ] Document agent responsibility for SQL safety

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
- [ ] `clap` - CLI framework
- [ ] `serde` - Serialization framework
- [ ] `serde_json` - JSON serialization
- [ ] `thiserror` - Error handling
- [ ] `tokio` - Async runtime (if using async drivers)

### Database Drivers
- [ ] PostgreSQL driver (choose one):
  - [ ] `tokio-postgres`
  - [ ] `sqlx` with postgres feature
- [ ] MySQL driver (choose one):
  - [ ] `mysql_async`
  - [ ] `sqlx` with mysql feature
- [ ] SQLite driver (choose one):
  - [ ] `rusqlite`
  - [ ] `sqlx` with sqlite feature

### MCP Server
- [ ] Research available Rust MCP server libraries
- [ ] Document MCP server dependency

### Testing
- [ ] `criterion` - Benchmarking
- [ ] `pretty_assertions` - Better test output
- [ ] `insta` - Snapshot testing

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
- [ ] Test with multiple database versions
- [ ] Document version requirements
- [ ] Isolate driver-specific code
- [ ] Consider using `sqlx` for unified interface

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
- [ ] Keep MCP layer thin
- [ ] Maintain CLI as primary interface
- [ ] Test MCP and CLI independently
- [ ] Document integration boundaries

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
- **Phase 3:** 1-2 weeks (PostgreSQL)
- **Phase 4:** 1-2 weeks (MySQL)
- **Phase 5:** 1 week (SQLite)
- **Phase 6:** 1-2 weeks (Integration)
- **Phase 7:** 1-2 weeks (MCP Server)
- **Phase 8:** 1 week (Security Audit)
- **Phase 9:** 1 week (Release Prep)

**Total Estimated Timeline:** 10-14 weeks

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
