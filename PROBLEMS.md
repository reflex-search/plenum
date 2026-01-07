# PROBLEMS.md

## Purpose

This document identifies architectural contradictions between PROJECT_PLAN.md and CLAUDE.md's core principles. Each issue must be resolved before implementation begins.

**Status:** Problems 1 & 2 resolved
**Last Updated:** 2026-01-06

---

## Critical Issues (Implementation Blockers)

### ✅ PROBLEM 1: Stateful Trait Design Contradicts Stateless Requirement [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 1.1 (line 45)

**Issue:**
The original trait signature `fn connect(config: ConnectionConfig) -> Result<Self>` suggested maintaining connection state, contradicting the stateless design requirement.

**Resolution: Option A - Stateless Trait**

```rust
trait DatabaseEngine {
    fn validate_connection(config: &ConnectionConfig) -> Result<ConnectionInfo>;
    fn introspect(config: &ConnectionConfig, schema_filter: Option<&str>) -> Result<SchemaInfo>;
    fn execute(config: &ConnectionConfig, query: &str, caps: &Capabilities) -> Result<QueryResult>;
}
```

**Changes Made:**
- Updated PROJECT_PLAN.md Phase 1.1 with stateless trait design
- All trait methods are static and take `&ConnectionConfig` as parameter
- Each operation handles connection internally: connect → execute → disconnect
- No connection state stored between operations
- Enforces statelessness by design (impossible to violate)

**Benefits:**
- Perfect alignment with MCP per-invocation model
- Simple mental model: one function call = one complete operation
- No lifecycle management needed
- Prevents accidental state leakage

**Date Resolved:** 2026-01-06

---

### ✅ PROBLEM 2: Unclear Purpose of `connect` Command [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 2.2 (lines 118-129)

**Issue:**
The purpose of `plenum connect` was unclear in a stateless design where connections are not persistent.

**Resolution: Option C (Enhanced) - Configuration Management**

`plenum connect` is for **managing stored connection configurations**, not establishing persistent sessions.

**Behavior:**

1. **Interactive connection picker** (no args):
   - Lists existing named connections (local, dev, prod)
   - Shows "--- New ---" option to create new connection
   - Launches configuration wizard for new connections

2. **Interactive configuration wizard**:
   - Prompts for engine, host, port, user, password, database
   - Prompts for connection name
   - Prompts for save location (local/global)
   - Validates connection before saving

3. **Non-interactive configuration** (with flags):
   ```bash
   plenum connect --name prod --engine postgres --host prod.example.com \
     --user readonly --password secret --database myapp --save global
   ```

4. **Connection validation**:
   - Tests connectivity
   - Returns connection metadata (version, server info)
   - Does NOT maintain persistent connection

**Storage:**
- Local: `.plenum/config.json` (team-shareable)
- Global: `~/.config/plenum/connections.json` (per-user, keyed by project path)

**Changes Made:**
- Added PROJECT_PLAN.md Phase 1.5 (Configuration Management)
- Updated Phase 2.2 with interactive/non-interactive modes
- Updated Phases 2.3 & 2.4 to support `--name` flag for named connections
- Updated CLAUDE.md with "Connection Configuration" section
- Added Phase 0.3 dependencies: `dialoguer`/`inquire`, `dirs`

**Benefits:**
- Agents don't manage credentials (human configures once)
- Simple agent commands: `plenum query --name prod --sql "..."`
- Teams can share local configs (for dev environments)
- Users keep production credentials private (global config)
- Maintains stateless execution (config read on each invocation)

**Date Resolved:** 2026-01-06

---

### ✅ PROBLEM 3: MCP Server Architecture Undefined [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 7 (lines 444-487)

**Issue:**
The plan doesn't specify how the MCP server relates to the CLI. Three fundamentally different architectures are possible:

**Architecture A: Shell Execution**
```
MCP Server → shells out to → `plenum` CLI binary → JSON stdout
```

**Architecture B: Shared Library**
```
MCP Server → calls Rust lib → shared core logic ← calls Rust lib ← CLI binary
```

**Architecture C: Embedded Server Mode**
```
`plenum --server` mode → runs MCP server → uses CLI logic internally
```

**Contradiction with CLAUDE.md:**
CLAUDE.md states "The Plenum CLI remains the execution boundary" (line 160) but doesn't clarify the architectural relationship. This is not a detail—it's a foundational decision.

**Why This Matters:**
- **Testing strategy** differs (integration vs unit tests)
- **Error handling** differs (parsing JSON vs native errors)
- **Dependency management** differs (single binary vs library crate)
- **Development workflow** differs (can MCP be tested independently?)

**Proposed Solution:**

**Recommendation: Architecture B (Shared Library)**

```
plenum/
├── plenum-core/        # Library crate with all logic
│   ├── engine/         # Database engines
│   ├── capability/     # Capability checking
│   └── output/         # JSON envelope types
├── plenum-cli/         # CLI binary
│   └── main.rs         # Calls plenum-core, outputs JSON
└── plenum-mcp/         # MCP server binary
    └── main.rs         # Calls plenum-core, wraps in MCP protocol
```

**Benefits:**
- CLI and MCP server share identical logic (determinism guaranteed)
- Both can be tested independently
- "CLI remains the execution boundary" because both CLI and MCP call the same library
- No JSON parsing overhead
- Clean separation of concerns

**Alternative: Architecture A (Shell Execution)**

Simpler to implement but:
- Requires parsing JSON output
- Process spawning overhead
- Harder to test MCP server independently

**Dependencies:**
- Must be decided in Phase 0 (not Phase 7)
- Affects project structure (lines 13-19)
- Impacts Cargo.toml configuration
- Determines testing strategy

**Action Required:**
1. Choose architecture in Phase 0
2. Move "Research MCP server implementation" to Phase 0.3
3. Restructure project as workspace if using Architecture B
4. Update PROJECT_PLAN.md Phase 7 to reflect architectural decision

**Resolution: Single Crate with MCP Subcommand (Modified Architecture C)**

**Architecture Decision:**
```
plenum/ (single crate)
├── src/
│   ├── main.rs       # Routes to CLI or MCP subcommand
│   ├── lib.rs        # Exports public API for both CLI and MCP
│   ├── cli.rs        # CLI command handling
│   ├── mcp.rs        # MCP server using rmcp SDK
│   └── [engine/, capability.rs, config.rs, output.rs, error.rs]
└── Cargo.toml
```

**Pattern:** Follows reflex-search implementation (https://github.com/reflex-search/reflex)

**Key Characteristics:**
- Single binary with `plenum mcp` hidden subcommand
- Uses `rmcp` crate (official Rust MCP SDK) with `#[tool]` macros
- Stdio transport for JSON-RPC over stdin/stdout
- Both CLI and MCP call same internal library functions (determinism guaranteed)
- No workspace needed (simpler project structure)

**MCP Tools Mapping:**
- `connect` tool → calls same logic as `plenum connect` CLI
- `introspect` tool → calls same logic as `plenum introspect` CLI
- `query` tool → calls same logic as `plenum query` CLI

**Benefits:**
- ✅ Proven pattern (used by reflex-search)
- ✅ Uses standard tooling (rmcp SDK)
- ✅ Simpler than workspace approach
- ✅ Maintains determinism (shared code paths)
- ✅ Easy to test and distribute (single binary)
- ✅ Aligns with CLAUDE.md ("CLI remains the execution boundary")

**Changes Made to PROJECT_PLAN.md:**
- Phase 0.1: Updated repository setup for binary + library targets
- Phase 0.3: Moved MCP research from Phase 7.1 to Phase 0 (resolves PROBLEM 6)
- Phase 0.3: Added rmcp, tokio, schemars dependencies
- Phase 1.6: Added new phase for library module structure
- Phase 2.1: Added fourth subcommand `mcp`
- Phase 7: Complete rewrite to use rmcp SDK pattern

**Date Resolved:** 2026-01-06

---

### 🚨 PROBLEM 4: SQLx Suggestion Violates Isolation Principle

**Location:** PROJECT_PLAN.md lines 515-523, 550

**Issue:**
The dependency checklist lists both native drivers AND `sqlx` as options:
```
- [ ] PostgreSQL driver (choose one):
  - [ ] `tokio-postgres`
  - [ ] `sqlx` with postgres feature
```

Risk mitigation (line 550) suggests: *"Consider using `sqlx` for unified interface"*

**Contradiction with CLAUDE.md:**
- "No compatibility layers" (CLAUDE.md line 22)
- "No shared SQL helpers across engines" (CLAUDE.md line 172)
- "Engine quirks stay inside engine modules" (CLAUDE.md line 174)

**Why This Matters:**
Using `sqlx` across all engines creates a shared abstraction layer that:
- Provides a "unified interface" (explicitly forbidden)
- May normalize behaviors across engines (breaks vendor-specific SQL)
- Could leak cross-engine behavior through shared types
- Contradicts the isolation principle

**Proposed Solution:**

**Mandate native drivers per engine:**
- PostgreSQL: `tokio-postgres` (official PostgreSQL driver)
- MySQL: `mysql_async` (purpose-built MySQL driver)
- SQLite: `rusqlite` (official SQLite wrapper)

**Why Native Drivers:**
- Maximum isolation between engines
- Vendor-specific behavior is preserved
- No risk of abstraction leakage
- Each engine handles its own quirks

**Do NOT use:**
- `sqlx` (provides unified interface)
- Any driver that abstracts across multiple databases
- Shared query building helpers

**Exception:**
If `sqlx` is used, it must be used **only for a single engine** with that engine's feature flag, never as a cross-engine abstraction. But native drivers are preferred.

**Dependencies:**
- Affects Phase 0.3 (dependency assessment)
- Impacts Phase 3, 4, 5 (implementation)
- Influences testing strategy

**Action Required:**
1. Remove `sqlx` from dependency options
2. Remove "Consider using sqlx for unified interface" from risk mitigation
3. Update Phase 0.3 to mandate native drivers
4. Document rationale in RESEARCH.md

**Resolution Status: RESOLVED ✅**

**Changes Made to PROJECT_PLAN.md:**
- Phase 0.3 (lines 64-74): Explicitly mandates native drivers with "NOT sqlx" warnings
- Dependencies Checklist (line 778): Header updated to "Database Drivers (native drivers only, NO sqlx)"
- Risk Mitigation (line 805): Updated to mandate native drivers for maximum engine isolation

**Changes Made to RESEARCH.md:**
- Added "Database Driver Selection Strategy" section documenting:
  - Decision: Native drivers only (tokio-postgres, mysql_async, rusqlite)
  - Rationale: Maximum isolation, vendor-specific behavior preservation, no abstraction leakage
  - Implementation implications: Each engine module is completely independent
  - Forbidden approaches: sqlx, Diesel, SeaORM, any cross-database abstraction

**Date Resolved:** 2026-01-06

---

### 🚨 PROBLEM 5: Read-Only Flag Design Error

**Location:** PROJECT_PLAN.md Phase 2.4, line 148

**Issue:**
```
- [ ] `--read-only` (default: true)
- [ ] `--allow-write` (explicit flag)
- [ ] `--allow-ddl` (explicit flag)
```

If read-only is the **default**, why have a `--read-only` flag?

**Contradiction with CLAUDE.md:**
- "Read-only is the default mode" (CLAUDE.md line 37)
- "Capabilities are NEVER inferred" (CLAUDE.md line 133)
- "Least privilege" principle (CLAUDE.md line 36-39)

**Why This Matters:**
Having a `--read-only` flag implies read-only is **optional** rather than the **default**. It creates ambiguity about the capability model:
- Does omitting all flags mean read-only? (should be yes)
- Can you pass `--read-only` with `--allow-write`? (contradiction)
- Does `--read-only` do anything if it's the default? (redundant)

**Proposed Solution:**

**Correct capability flags:**
- **No `--read-only` flag** (it's the immutable default)
- `--allow-write` (opts into write operations: INSERT, UPDATE, DELETE)
- `--allow-ddl` (opts into DDL operations: CREATE, DROP, ALTER)

**Capability matrix:**
```bash
# Default: read-only (SELECT queries only)
plenum query --sql "SELECT * FROM users"

# Explicit write permission
plenum query --sql "UPDATE users SET ..." --allow-write

# DDL permission (implies write)
plenum query --sql "CREATE TABLE ..." --allow-ddl

# Invalid: DDL requires explicit flag even with --allow-write
plenum query --sql "DROP TABLE ..." --allow-write  # SHOULD FAIL
```

**Implementation detail:**
`--allow-ddl` should imply `--allow-write` (DDL operations are inherently write operations), but both must be explicit.

**Dependencies:**
- Affects Phase 2.4 (query command design)
- Impacts Phase 1.2 (Capabilities struct)
- Influences capability validation (Phase 1.4)

**Action Required:**
1. Remove `--read-only` flag from Phase 2.4
2. Clarify that default is read-only (no flag needed)
3. Define capability hierarchy (does DDL imply write?)
4. Update CLAUDE.md if hierarchy needs documentation

---

### 🚨 PROBLEM 6: MCP Research Deferred Too Late

**Location:** PROJECT_PLAN.md Phase 7.1, lines 354, 526-527

**Issue:**
```
### 7.1 MCP Server Foundation
- [ ] Research MCP server implementation in Rust
```

MCP research is scheduled for Phase 7, after:
- All core architecture (Phase 1)
- All CLI implementation (Phase 2)
- All three database engines (Phases 3-5)
- Integration testing (Phase 6)

**Contradiction with CLAUDE.md:**
CLAUDE.md presents MCP integration as a **core requirement**: "Plenum is exposed via a local MCP server" (line 153). It's not an add-on feature.

**Why This Matters:**
If MCP research in Phase 7 reveals that:
- The trait design doesn't fit MCP's tool model
- The JSON output format isn't MCP-compatible
- The stateless design requires different error handling
- Available Rust MCP libraries have constraints

...then you'd need to refactor everything built in Phases 1-6.

**Proposed Solution:**

**Move MCP research to Phase 0.3:**
```
### 0.3 Dependency Assessment
- [ ] Research and select database driver crates
- [ ] Research MCP server libraries for Rust
  - [ ] Evaluate available MCP server implementations
  - [ ] Verify compatibility with stateless design
  - [ ] Confirm JSON output can be wrapped in MCP protocol
  - [ ] Identify any architectural constraints
- [ ] Select JSON serialization library: `serde_json`
- [ ] Document MCP architecture decision (see PROBLEM 3)
```

**Why This Matters:**
MCP integration constraints must inform the initial architecture, not be retrofitted later.

**Dependencies:**
- Must happen before Phase 1 (core architecture)
- Informs trait design decisions
- Affects JSON envelope structure (Phase 1.2)
- Determines project structure (workspace vs single crate)

**Action Required:**
1. Move MCP research from Phase 7.1 to Phase 0.3
2. Make MCP architecture decision before Phase 1 begins
3. Document MCP constraints in RESEARCH.md
4. Ensure Phase 1 trait design is MCP-compatible

---

### 🚨 PROBLEM 7: Security Model Confusion

**Location:** PROJECT_PLAN.md Phase 8.2, lines 408-411

**Issue:**
```
### 8.2 SQL Injection Prevention
- [ ] Verify parameterized queries where applicable
- [ ] Document SQL injection surface area
- [ ] Note that Plenum passes raw SQL (by design)
- [ ] Document agent responsibility for SQL safety
```

**Contradiction:**
Bullets 1 and 3 contradict each other. If "Plenum passes raw SQL (by design)," then there are **no** parameterized queries in Plenum's code.

**Why This Matters:**
This reveals confusion about Plenum's security model. Plenum's security responsibility is:

✅ **Plenum IS responsible for:**
- Capability enforcement (read/write/DDL checks)
- Preventing capability violations
- Timeout enforcement
- Row limit enforcement
- Connection credential handling (not logging/persisting)

❌ **Plenum is NOT responsible for:**
- SQL injection prevention
- SQL query validation (beyond capability categorization)
- Query optimization
- Query sanitization

**Proposed Solution:**

**Rewrite Phase 8.2 as "Security Model Verification":**
```
### 8.2 Security Model Verification
- [ ] Verify capability enforcement prevents unauthorized operations
- [ ] Verify DDL detection catches all DDL statement types
- [ ] Verify write detection catches all write operations
- [ ] Document that SQL injection prevention is the agent's responsibility
- [ ] Document that Plenum passes SQL verbatim to the database
- [ ] Verify Plenum does not modify, sanitize, or interpret SQL content
- [ ] Document security boundaries clearly in README
```

**Security Model Documentation:**
Add to RESEARCH.md or README:

```
## Security Model

Plenum's security boundary is **capability enforcement**, not SQL validation.

### Plenum Enforces:
- Operation type restrictions (read-only, write, DDL)
- Row limits and timeouts
- Credential security (no logging/persistence)

### Plenum Does NOT Enforce:
- SQL injection prevention
- Query semantic correctness
- Business logic constraints

### Agent Responsibility:
The calling agent MUST:
- Sanitize user inputs before constructing SQL
- Validate queries for safety before passing to Plenum
- Implement application-level security controls

Plenum assumes SQL passed to it is safe. It provides capability
constraints, not query validation.
```

**Dependencies:**
- Affects security audit scope (Phase 8)
- Influences documentation (Phase 6.5)
- Should be documented in CLAUDE.md

**Action Required:**
1. Rewrite Phase 8.2 to focus on capability enforcement
2. Remove "parameterized queries" references
3. Add security model documentation to RESEARCH.md
4. Consider adding security model section to CLAUDE.md

---

## Moderate Issues (Should Fix Before Implementation)

### ⚠️ PROBLEM 8: Phase 0 Redundancy

**Location:** PROJECT_PLAN.md Phase 0.1, lines 13-19

**Issue:**
```
### 0.1 Repository Setup
- [ ] Initialize Rust project with Cargo
- [ ] Configure `.gitignore` for Rust projects
- [ ] Set up basic project structure
- [ ] Add LICENSE file
- [ ] Create initial README.md with project description
```

**Current State:**
Git status shows repository already initialized with commits:
- `036a688 Initial commit`
- `333061d Add CLAUDE.md`
- `4393759 Initial plan`

**Proposed Solution:**
Rewrite Phase 0.1 to reflect current state:
```
### 0.1 Repository Setup
- [x] Initialize Rust project with Cargo
- [ ] Initialize Cargo project structure
  - [ ] Run `cargo init --name plenum`
  - [ ] Configure Cargo.toml with metadata
  - [ ] Set up workspace structure (if using Architecture B from PROBLEM 3)
- [ ] Configure `.gitignore` for Rust builds
  - [ ] Add `/target`
  - [ ] Add `Cargo.lock` (if library)
- [ ] Add LICENSE file (choose license)
- [ ] Update README.md with build instructions
```

**Dependencies:**
- Depends on PROBLEM 3 resolution (workspace structure)

**Action Required:**
Update Phase 0.1 tasks to reflect repository's current state.

---

### ⚠️ PROBLEM 9: SQL Parsing Strategy Undefined

**Location:** PROJECT_PLAN.md Phase 1.4, lines 91-100

**Issue:**
```
### 1.4 Capability Validation
- [ ] Implement capability validator
- [ ] Define SQL statement categorization:
  - [ ] Read-only: SELECT
  - [ ] Write: INSERT, UPDATE, DELETE
  - [ ] DDL: CREATE, DROP, ALTER, TRUNCATE, RENAME
```

**Missing Detail:**
The plan doesn't specify **how** SQL will be categorized. Options include:

**Option A: Regex Pattern Matching**
```rust
fn is_ddl(sql: &str) -> bool {
    let sql_upper = sql.trim().to_uppercase();
    sql_upper.starts_with("CREATE ")
        || sql_upper.starts_with("DROP ")
        || sql_upper.starts_with("ALTER ")
        // ... etc
}
```

**Pros:** Simple, no dependencies
**Cons:** Brittle, can be fooled by comments or whitespace

**Option B: SQL Parser Library**
```rust
use sqlparser::parser::Parser;
use sqlparser::dialect::GenericDialect;

fn categorize_query(sql: &str) -> QueryCategory {
    let dialect = GenericDialect {};
    let ast = Parser::parse_sql(&dialect, sql)?;
    // Analyze AST
}
```

**Pros:** Robust, handles edge cases
**Cons:** Adds dependency, may not support all vendor-specific SQL

**Option C: Database-Specific Query Analysis**
Let each database engine categorize queries using its own parser:
```rust
// PostgreSQL might use EXPLAIN without execution
// MySQL might use query attributes
// SQLite might parse with its own tokenizer
```

**Pros:** Most accurate for vendor-specific SQL
**Cons:** Complex, requires database connectivity just for validation

**Proposed Solution:**

**Recommendation: Start with Option A (Regex), Plan for Option B**

Phase 1 implementation:
- Use simple regex-based categorization
- Document known limitations
- Add comprehensive test suite for edge cases
- Ensure false positives fail-safe (e.g., unknown query → treated as DDL)

Post-MVP:
- Consider adding `sqlparser` if regex proves insufficient
- Evaluate per-database parsing if vendor-specific issues arise

**Edge Cases to Handle:**
```sql
-- Comments before statements
/* multi-line comment */ SELECT ...

-- Multiple statements
SELECT * FROM users; DROP TABLE users;

-- Whitespace variations
  \n  \t  CREATE  TABLE ...

-- CTEs that look like DDL
WITH RECURSIVE cte AS (...) SELECT ...

-- MySQL implicit commit DDL list (comprehensive)
```

**Dependencies:**
- Affects Phase 0.3 (dependency selection)
- Impacts Phase 1.4 (capability validation)
- Influences testing strategy (Phase 3.4, 4.4, 5.4)

**Action Required:**
1. Explicitly choose parsing strategy in Phase 1.4
2. Add dependency to Phase 0.3 if using parser library
3. Create comprehensive test matrix for statement categorization
4. Document edge cases and failure modes in RESEARCH.md

---

## Resolution Checklist

Before proceeding to implementation, verify all problems are resolved:

### Critical Issues
- [x] **PROBLEM 1:** Trait design rewritten to be stateless ✅ (2026-01-06)
- [x] **PROBLEM 2:** `connect` command purpose clarified as configuration management ✅ (2026-01-06)
- [x] **PROBLEM 3:** MCP architecture chosen and documented ✅ (2026-01-06)
- [x] **PROBLEM 4:** SQLx removed; native drivers mandated ✅ (2026-01-06)
- [ ] **PROBLEM 5:** `--read-only` flag removed from design (✅ partially done in Phase 2.4)
- [x] **PROBLEM 6:** MCP research moved to Phase 0 ✅ (resolved with PROBLEM 3, 2026-01-06)
- [ ] **PROBLEM 7:** Security model clarified in plan

### Moderate Issues
- [ ] **PROBLEM 8:** Phase 0 updated to reflect repo state
- [ ] **PROBLEM 9:** SQL parsing strategy explicitly chosen

### Documentation Updates Required
- [x] Update PROJECT_PLAN.md with resolutions (Phases 0.3, 1.1, 1.5, 2.2, 2.3, 2.4) ✅
- [x] Update CLAUDE.md if command surface changes ✅
- [x] Update RESEARCH.md with architectural decisions (native driver strategy documented) ✅
- [ ] Document security model clearly

---

## Next Steps

1. **Review Session:** Discuss each problem and proposed solution
2. **Make Decisions:** Choose between solution options where multiple exist
3. **Update PROJECT_PLAN.md:** Incorporate all resolutions
4. **Update CLAUDE.md:** Reflect any changes to core requirements
5. **Begin Phase 0:** Start implementation only after all critical issues resolved

---

## Notes

These problems were identified through rigorous analysis of alignment between PROJECT_PLAN.md and CLAUDE.md's non-negotiable principles. Resolving them before implementation prevents costly refactoring and ensures the final codebase embodies the agent-first, stateless, vendor-specific philosophy that defines Plenum.

**Remember the guiding question:**
> "Does this make autonomous agents safer, more deterministic, or more constrained?"

Every resolution should answer "yes" to this question.
