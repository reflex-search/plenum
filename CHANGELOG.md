# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Nothing yet.

---

## [2.1.1] - 2026-10-30

### Security

- MySQL: bump `mysql_async` to `0.37` so it no longer pulls the vulnerable transitive `zstd` 0.13 / `zstd-safe` 7 crates; this clears the `cargo audit` failure ([RUSTSEC-2026-0391](https://rustsec.org/advisories/RUSTSEC-2026-0391), [RUSTSEC-2026-0398](https://rustsec.org/advisories/RUSTSEC-2026-0398), [RUSTSEC-2026-0403](https://rustsec.org/advisories/RUSTSEC-2026-0403))

### Changed

- DuckDB: unpinned to `1.20000.1` now that duckdb-rs #923 fixes aarch64 static-extension linking; 2.1.0 pinned `=1.10506.0` to dodge the broken 1.20000.0 ([duckdb-rs #920](https://github.com/duckdb/duckdb-rs/issues/920))

---

## [2.1.0] - 2026-10-27

### Added

- `plenum mcp` connection binding at server registration: `--project-path`, `--name`, and `--dsn-env` decouple resolution from the launcher's cwd ([REF-297](/REF/issues/REF-297))
- MCP `connect` save mode (`save`: `"local"` | `"global"`, `project_path`, `name`) that persists connections by credential reference; `password_env` and `keychain_service`/`keychain_account` accepted on `connect`, `introspect`, and `query` ([REF-298](/REF/issues/REF-298))
- `plenum connect` wizard offers plaintext, env var, shell command, and OS keychain credential sources; `mcp_stanza` in `--save` output provides a ready-to-paste `mcpServers` block ([REF-299](/REF/issues/REF-299))
- JSON Schema `schemas/connect_save_success.json` for the `connect --save` envelope, covered by schema drift ([REF-304](/REF/issues/REF-304))

### Fixed

- Capability: close read-only bypasses reachable from an accepted `SELECT`. PostgreSQL `dblink`/`dblink_exec`, `lo_import`, and `lo_export` are now rejected. MySQL `/*! */` and `/*+ */` executable comments are now rejected. The comment stripper is now quote-aware, so a string literal can no longer hide SQL from the classifier. Live side-effect tests on PostgreSQL 16 and MySQL 8.0/8.4 prove database state is unchanged
- `plenum connect --project-path X --save local` reported success but persisted nothing when run from another directory; it now writes `X/.plenum/config.json` ([REF-303](/REF/issues/REF-303))
- DuckDB: compile against `duckdb` 1.10506.0, where `Value` is `#[non_exhaustive]`; `UHUGEINT` converts to a string, `GEOMETRY` (WKB) to Base64, and an unknown value type is a structured `QUERY_FAILED` error instead of a guess
- Build/lint: satisfy Rust 1.99 `clippy` lints (`unused_async_trait_impl`, `assert_is_empty`)
- Build: pin DuckDB to `=1.10506.0` so `aarch64-unknown-linux-gnu` release binaries link; a newer duckdb-rs regressed aarch64 static-extension linking under GNU ld ([duckdb-rs #920](https://github.com/duckdb/duckdb-rs/issues/920))

### Security

- All MCP tools reject `password_command` with `CAPABILITY_VIOLATION`: it executes a shell command, so accepting it from tool arguments gave agents arbitrary command execution. The CLI flag and wizard still offer it
- MCP `connect` rejects inline plaintext `password` with `CAPABILITY_VIOLATION` ([REF-298](/REF/issues/REF-298))

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
