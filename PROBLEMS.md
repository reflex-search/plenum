# PROBLEMS.md

## Purpose

This document identifies architectural contradictions between PROJECT_PLAN.md and CLAUDE.md's core principles. Each issue must be resolved before implementation begins.

**Status:** All problems resolved (1-9 complete) ✅
**Last Updated:** 2026-01-07

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

### ✅ PROBLEM 5: Read-Only Flag Design Error [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 2.4, line 148

**Issue:**
The original design included a redundant `--read-only` flag when read-only is already the default.

**Resolution: Capability Flag Design + Hierarchy Definition**

**Changes Made to PROJECT_PLAN.md:**
- Phase 2.4 (lines 293-307): Removed `--read-only` flag ✅
- Phase 2.4 (line 303): Explicitly states "Read-only by default (no flag needed)" ✅
- Phase 1.1 (lines 97-98): `Capabilities` struct has no `read_only` field ✅
- Phase 1.4 (lines 157-167): Added capability hierarchy documentation ✅

**Changes Made to CLAUDE.md:**
- Added "Capability Hierarchy" section with:
  - Three-tier model: Read-only (default) → Write → DDL
  - Explicit rule: `--allow-ddl` implicitly grants write permissions
  - Explicit rule: `--allow-write` does NOT enable DDL
  - Five concrete examples showing allowed/denied combinations

**Capability Hierarchy Decision:**
**DDL implies write** (DDL is a superset of write operations)

**Final Flag Behavior:**
- **No flags** → Read-only (SELECT only)
- `--allow-write` → INSERT, UPDATE, DELETE (but NOT DDL)
- `--allow-ddl` → DDL operations AND write operations

**Rationale:**
- Maintains explicitness: Agents must explicitly request `--allow-ddl`
- Logical hierarchy: If you can DROP TABLE, you should be able to INSERT
- Agent safety: DDL is more dangerous than write, so it's a superset capability
- Aligns with CLAUDE.md principle: "DDL operations are write operations"

**Date Resolved:** 2026-01-06

---

### ✅ PROBLEM 6: MCP Research Deferred Too Late [RESOLVED]

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

**Resolution Status: RESOLVED ✅**

This problem was resolved together with PROBLEM 3. MCP research has been moved from Phase 7.1 to Phase 0.3 in PROJECT_PLAN.md.

**Changes Made to PROJECT_PLAN.md:**
- Phase 0.3 (lines 46-62): Added "CRITICAL: MCP Architecture Research (moved from Phase 7.1)" section
- Phase 0.3: Includes rmcp evaluation, stdio transport verification, reflex-search pattern review
- Phase 0.3: Documents MCP architecture decision (single crate with `plenum mcp` subcommand)
- Phase 0.3: Selects MCP dependencies (rmcp, tokio, schemars)
- Phase 7: Complete rewrite to use rmcp SDK pattern instead of custom implementation

**Benefits:**
- MCP constraints inform initial architecture design
- Prevents late-stage refactoring of core traits
- Ensures stateless design is MCP-compatible from the start
- Dependencies identified before Phase 1 begins

**Date Resolved:** 2026-01-06 (resolved together with PROBLEM 3)

---

### ✅ PROBLEM 7: Security Model Confusion [RESOLVED]

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

**Resolution Status: RESOLVED ✅**

**Changes Made to PROJECT_PLAN.md:**
- Phase 8.2 (lines 674-681): Renamed from "SQL Injection Prevention" to "Security Model Verification"
- Phase 8.2: Removed "Verify parameterized queries where applicable" (contradicted raw SQL design)
- Phase 8.2: Added tasks to verify capability enforcement and document security boundaries
- Phase 8.2: Explicitly documents that SQL injection prevention is agent's responsibility

**Changes Made to CLAUDE.md:**
- Added "Security Model" section after "Error Handling Rules" (lines 256-276)
- Clearly defines Plenum's security boundary as capability enforcement, not SQL validation
- Documents what Plenum enforces vs. what agents must handle
- States explicitly: "Plenum assumes SQL passed to it is safe"

**Security Model Summary:**

✅ **Plenum Enforces:**
- Operation type restrictions (read-only, write, DDL)
- Row limits and timeouts
- Credential security (no logging/persistence)

❌ **Plenum Does NOT Enforce:**
- SQL injection prevention
- Query semantic correctness
- Business logic constraints

**Agent Responsibility:**
Agents must sanitize inputs, validate queries, and implement application-level security controls before passing SQL to Plenum.

**Date Resolved:** 2026-01-06

---

## Moderate Issues (Should Fix Before Implementation)

### ✅ PROBLEM 8: Phase 0 Redundancy [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 0.1, lines 13-41

**Issue:**
Phase 0.1 tasks didn't accurately reflect the current repository state. Git repository was already initialized with documentation files (CLAUDE.md, PROJECT_PLAN.md, PROBLEMS.md, RESEARCH.md, README.md), but the checklist didn't acknowledge this progress.

**Additional Issues Found:**
1. README.md already exists but isn't marked as created
2. Cargo.lock incorrectly listed in .gitignore (should be committed for binary projects)
3. Existing documentation files not acknowledged

**Resolution:**

**Changes Made to PROJECT_PLAN.md Phase 0.1:**
- Added new checklist section acknowledging completed documentation files ✅
- Marked all documentation files as complete (CLAUDE.md, PROJECT_PLAN.md, PROBLEMS.md, RESEARCH.md)
- Changed "Update README.md" to "Expand README.md" to reflect it exists but needs content
- Removed Cargo.lock from .gitignore checklist with note explaining it should be committed
- Added clarifying note: "Do NOT add `Cargo.lock` (should be committed for binary projects)"

**Rationale:**
- **Git repository**: Already initialized ✅
- **Documentation**: Core planning docs already created ✅
- **Cargo.lock handling**: For binary/application projects (like plenum), Cargo.lock should be committed to ensure reproducible builds
- **Clarity**: Phase 0.1 now clearly shows what's done vs. what remains

**Current State After Resolution:**
Phase 0.1 accurately reflects repository state and specifies remaining tasks:
- Rust project initialization (Cargo.toml, src/ structure)
- .gitignore configuration
- LICENSE file creation
- README.md expansion with project description and build instructions

**Date Resolved:** 2026-01-07

---

### ✅ PROBLEM 9: SQL Parsing Strategy Undefined [RESOLVED]

**Location:** PROJECT_PLAN.md Phase 1.4, lines 156-212

**Issue:**
The plan didn't specify **how** SQL would be categorized into read-only/write/DDL before execution. Three approaches were considered:
- **Option A:** Regex pattern matching (simple, no dependencies)
- **Option B:** SQL parser library (robust but adds dependency, may not handle vendor-specific SQL)
- **Option C:** Database-specific query analysis (complex, requires connectivity just for validation)

**Resolution: Option A - Regex-based with Engine-Specific Implementations**

**Decision:**
Use regex-based SQL categorization with engine-specific implementations (no shared SQL helpers).

**Key Specifications:**
1. **Engine-specific implementations**: Each engine (PostgreSQL/MySQL/SQLite) implements its own `categorize_query(sql: &str) -> Result<QueryCategory>` logic
2. **Pre-processing steps**:
   - Strip SQL comments (`--` and `/* */`)
   - Normalize whitespace and case
   - Detect multi-statement queries
3. **Multi-statement handling**: Reject multi-statement queries in MVP (safest approach)
4. **Edge case handling**:
   - EXPLAIN queries: Strip prefix, categorize underlying statement
   - CTEs (WITH): Match final statement type
   - Transaction control (BEGIN/COMMIT/ROLLBACK): Treat as read-only
   - Stored procedures (CALL/EXEC): Treat as write (conservative)
   - Unknown statements: Treat as DDL (fail-safe, most restrictive)
5. **MySQL-specific**: Maintain explicit list of implicit commit DDL statements
6. **Test coverage**: Comprehensive edge case matrix per engine

**Rationale:**
- ✅ Aligns with "simplest explicit implementation" (CLAUDE.md:300)
- ✅ No external dependencies needed (uses stdlib regex)
- ✅ Respects "no shared SQL helpers across engines" (CLAUDE.md:240)
- ✅ Fast pre-execution validation
- ✅ Deterministic and testable
- ✅ Fail-safe defaults protect agent safety
- ✅ Can evolve to sqlparser post-MVP if regex proves insufficient

**Trade-offs Accepted:**
- Some edge cases may require iteration (but comprehensive tests will catch them)
- Regex can be fooled by complex patterns (but fail-safe defaults protect safety)

**Changes Made to PROJECT_PLAN.md:**
- Phase 1.4 (lines 156-212): Expanded with comprehensive SQL categorization strategy
  - Added regex-based approach with engine-specific implementations
  - Documented pre-processing steps (comment stripping, whitespace normalization)
  - Specified multi-statement query rejection for MVP
  - Listed edge cases: EXPLAIN, CTEs, transaction control, stored procedures, unknown statements
  - Added comprehensive test matrix requirements per engine
  - Documented MySQL implicit commit handling

**Changes Made to RESEARCH.md:**
- Added "SQL Categorization Strategy" section with:
  - Decision rationale (regex vs parser library vs database-specific analysis)
  - Engine-specific implementation approach
  - Pre-processing and edge case handling details
  - Known limitations and accepted trade-offs
  - Comprehensive test coverage requirements
  - Post-MVP evolution criteria (when to consider sqlparser)

**Date Resolved:** 2026-01-07

---

## Resolution Checklist

Before proceeding to implementation, verify all problems are resolved:

### Critical Issues
- [x] **PROBLEM 1:** Trait design rewritten to be stateless ✅ (2026-01-06)
- [x] **PROBLEM 2:** `connect` command purpose clarified as configuration management ✅ (2026-01-06)
- [x] **PROBLEM 3:** MCP architecture chosen and documented ✅ (2026-01-06)
- [x] **PROBLEM 4:** SQLx removed; native drivers mandated ✅ (2026-01-06)
- [x] **PROBLEM 5:** `--read-only` flag removed; capability hierarchy defined ✅ (2026-01-06)
- [x] **PROBLEM 6:** MCP research moved to Phase 0 ✅ (resolved with PROBLEM 3, 2026-01-06)
- [x] **PROBLEM 7:** Security model clarified in plan ✅ (2026-01-06)

### Moderate Issues
- [x] **PROBLEM 8:** Phase 0 updated to reflect repo state ✅ (2026-01-07)
- [x] **PROBLEM 9:** SQL parsing strategy explicitly chosen ✅ (2026-01-07)

### Documentation Updates Required
- [x] Update PROJECT_PLAN.md with resolutions (Phases 0.3, 1.1, 1.4, 1.5, 2.2, 2.3, 2.4, 8.2) ✅
- [x] Update CLAUDE.md with capability hierarchy ✅
- [x] Update CLAUDE.md with security model (PROBLEM 7) ✅
- [x] Update RESEARCH.md with architectural decisions (native driver strategy, SQL categorization strategy) ✅

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
