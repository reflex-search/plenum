# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `plenum mcp` connection binding at server registration: `--project-path`, `--name`, and `--dsn-env` decouple resolution from the launcher's cwd ([REF-297](/REF/issues/REF-297))
- MCP `connect` save mode (`save`: `"local"` | `"global"`, `project_path`, `name`) that persists connections by credential reference; `password_env` and `keychain_service`/`keychain_account` accepted on `connect`, `introspect`, and `query` ([REF-298](/REF/issues/REF-298))
- `plenum connect` wizard offers plaintext, env var, shell command, and OS keychain credential sources ([REF-299](/REF/issues/REF-299))
- `mcp_stanza` in `plenum connect --save` output: a ready-to-paste `mcpServers` block for the saved connection ([REF-299](/REF/issues/REF-299))
- JSON Schema `schemas/connect_save_success.json` for the `connect --save` envelope, covered by schema drift ([REF-304](/REF/issues/REF-304))
- `--diff-against` flag for `plenum introspect` — read-only schema diff against a prior snapshot ([REF-281](/REF/issues/REF-281))
- Live-DB test harness: Docker Compose fixtures, vendor seed SQL per engine, gated test suites, and `scripts/test-live.sh` orchestration ([REF-275](/REF/issues/REF-275))
- MySQL 8.0 and 8.4 live test matrix: connect, introspect, query, safety, and envelope coverage ([REF-276](/REF/issues/REF-276))
- PostgreSQL 16 live test matrix: connect, introspect, query, read-only safety, and output envelope coverage ([REF-277](/REF/issues/REF-277))
- SQLite offline parity test suite ([REF-278](/REF/issues/REF-278))
- CI job `live-db-tests` wired via `scripts/test-live.sh` ([REF-279](/REF/issues/REF-279))
- `--dsn` one-off connection flag for `plenum query` and `plenum introspect` ([REF-272](/REF/issues/REF-272))
- `password_command` and `keychain_entry` credential sources on stored connections ([REF-271](/REF/issues/REF-271))
- `max_bytes` byte-budget guard on query results for MCP token-limit safety ([REF-269](/REF/issues/REF-269))
- `--list` flag on `plenum connect` to enumerate saved connections as JSON ([REF-268](/REF/issues/REF-268))
- `--test` ping flag on `plenum connect` for connection validation ([REF-267](/REF/issues/REF-267))
- Versioned JSON Schemas (Draft 7) for all output envelopes in `schemas/`; schema drift test fails CI on divergence ([REF-265](/REF/issues/REF-265))
- Column comments and estimated row counts in `plenum introspect` output ([REF-263](/REF/issues/REF-263))
- Session-level read-only enforcement at the database-driver level for all engines ([REF-261](/REF/issues/REF-261))
- `--password-env` end-to-end support on `plenum connect` ([REF-36](/REF/issues/REF-36))

### Fixed

- DuckDB: compile against `duckdb` 1.10506.0, where `Value` is `#[non_exhaustive]`; `UHUGEINT` converts to a string, `GEOMETRY` (WKB) to Base64, and an unknown value type is a structured `QUERY_FAILED` error instead of a guess
- `plenum connect --project-path X --save local` reported success but persisted nothing when run from another directory; it now writes `X/.plenum/config.json` ([REF-303](/REF/issues/REF-303))
- PostgreSQL: correct NULL detection, composite foreign-key introspection, and view definitions ([REF-277](/REF/issues/REF-277))
- MySQL: route text-protocol statements correctly; classify `EXPLAIN`; surface timeout as a first-class error ([REF-258](/REF/issues/REF-258))
- Build: vendor OpenSSL for hermetic release builds ([REF-258](/REF/issues/REF-258))
- Capability: reject `WITH`-CTE DML bypasses across all engines ([REF-41](/REF/issues/REF-41))

### Security

- MCP `connect` rejects inline plaintext `password` with `CAPABILITY_VIOLATION` ([REF-298](/REF/issues/REF-298))
- All MCP tools reject `password_command` with `CAPABILITY_VIOLATION`: it executes a shell command, so accepting it from tool arguments gave agents arbitrary command execution. The CLI flag and wizard still offer it.

### Changed

- SQLite: tighten read-only `PRAGMA` allowlist ([REF-44](/REF/issues/REF-44))
- Code style: `rustfmt` and `clippy` pedantic lint compliance enforced across the tree ([REF-258](/REF/issues/REF-258))
- Docs: live-DB testing directives added to `CLAUDE.md` ([REF-275](/REF/issues/REF-275))

---

## [0.1.0] - 2026-01-09

### Added

- `plenum connect` command: interactive wizard and non-interactive flag-driven connection configuration; local (`.plenum/config.json`) and global (`~/.config/plenum/connections.json`) storage; named connections with per-project default pointer
- `plenum introspect` command: schema introspection returning tables, columns, primary keys, foreign keys, and indexes as JSON
- `plenum query` command: constrained read-only SQL execution with `--max-rows` and `--timeout-ms` safety guards
- `plenum mcp` command: MCP server exposing all three commands as tools
- PostgreSQL, MySQL, and SQLite engine implementations behind a shared trait; no cross-engine SQL sharing
- Capability enforcement model: write and DDL operations rejected before execution
- JSON-only stdout contract: success and error envelopes with stable `ok`, `engine`, `command`, `data`, and `meta` fields
- Session-level `readonly` flag on `StoredConnection`
- Wildcard database support for MySQL and PostgreSQL engines
- Execution time (`execution_ms`) field in query result metadata
- Row representation as ordered `Vec<(String, Value)>` across all engines
- Release pipeline: crates.io and npm publishing workflows
