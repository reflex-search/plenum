//! Query Validation for Read-Only Operations
//!
//! This module implements strict read-only query validation.
//! Plenum is a read-only tool - all write and DDL operations are rejected.
//!
//! # Validation Strategy
//! - Engine-specific pattern matching (no shared SQL helpers)
//! - Conservative approach (fail-safe defaults)
//! - Only SELECT, SHOW, DESCRIBE, PRAGMA, EXPLAIN, and transaction control statements are permitted
//! - Everything else is rejected with a helpful error message

use crate::engine::{Capabilities, DatabaseType};
use crate::error::{PlenumError, Result};

/// Validate query is read-only
///
/// This function checks if the query is a permitted read-only operation.
/// Any write or DDL operations are rejected with a helpful error message.
///
/// # Arguments
/// * `sql` - The SQL query to validate
/// * `_caps` - Capabilities (currently only used for `max_rows`/`timeout`, not for permission checks)
/// * `engine` - Database engine type
///
/// # Returns
/// * `Ok(())` if the query is read-only
/// * `Err(PlenumError)` with a helpful message if the query attempts to modify data
pub fn validate_query(sql: &str, _caps: &Capabilities, engine: DatabaseType) -> Result<()> {
    // Pre-process SQL
    let processed = preprocess_sql(sql)?;

    // Check if query is read-only (engine-specific)
    if is_read_only(&processed, engine) {
        Ok(())
    } else {
        // Reject with helpful error message
        Err(PlenumError::capability_violation(format!(
            "Plenum is read-only and cannot execute this query. Please run this query manually:\n\n{sql}"
        )))
    }
}

/// Pre-process SQL query before categorization
///
/// This function:
/// 1. Trims leading/trailing whitespace
/// 2. Strips SQL comments (-- and /* */)
/// 3. Normalizes to uppercase for pattern matching
/// 4. Detects multi-statement queries (rejects them)
fn preprocess_sql(sql: &str) -> Result<String> {
    // Trim whitespace
    let mut processed = sql.trim().to_string();

    // Check for empty query
    if processed.is_empty() {
        return Err(PlenumError::invalid_input("Query cannot be empty"));
    }

    // Strip comments
    processed = strip_comments(&processed);

    // Check for multi-statement queries
    // Conservative approach: reject any query containing semicolons
    // (except trailing semicolon)
    let trimmed_for_check = processed.trim_end_matches(';').trim();
    if trimmed_for_check.contains(';') {
        return Err(PlenumError::invalid_input("Multi-statement queries are not supported in MVP"));
    }

    // Normalize to uppercase for pattern matching
    processed = processed.to_uppercase();

    Ok(processed)
}

/// Strip SQL comments from query
///
/// Handles:
/// - Line comments: -- comment
/// - Block comments: /* comment */
fn strip_comments(sql: &str) -> String {
    let mut result = String::new();
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '-' if chars.peek() == Some(&'-') => {
                // Line comment: skip until newline
                chars.next(); // consume second '-'
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        result.push('\n'); // preserve newline
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                // Block comment: skip until */
                chars.next(); // consume '*'
                let mut prev = ' ';
                for ch in chars.by_ref() {
                    if prev == '*' && ch == '/' {
                        break;
                    }
                    prev = ch;
                }
                result.push(' '); // replace comment with space
            }
            _ => result.push(ch),
        }
    }

    result
}

/// Check if query is read-only (engine-specific)
///
/// Each engine has slightly different SQL dialects, so validation is engine-specific.
/// This is a conservative check - if uncertain, the query is rejected.
fn is_read_only(sql: &str, engine: DatabaseType) -> bool {
    match engine {
        DatabaseType::Postgres => is_read_only_postgres(sql),
        DatabaseType::MySQL => is_read_only_mysql(sql),
        DatabaseType::SQLite => is_read_only_sqlite(sql),
    }
}

/// Strip EXPLAIN/EXPLAIN ANALYZE prefix from query
fn strip_explain_prefix(sql: &str) -> String {
    let sql = sql.trim();

    // Handle EXPLAIN ANALYZE
    if let Some(stripped) = sql.strip_prefix("EXPLAIN ANALYZE") {
        return stripped.trim().to_string();
    }

    // Handle EXPLAIN
    if let Some(stripped) = sql.strip_prefix("EXPLAIN") {
        return stripped.trim().to_string();
    }

    sql.to_string()
}

/// DML/DDL keywords that must never appear in a read-only query.
///
/// Used by `is_safe_cte_query` to detect writes hidden inside CTE bodies or
/// in the trailing statement after a `WITH` clause (REF-41). Keywords are
/// matched as whole identifier tokens against the already-uppercased query.
const WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "COPY", "TRUNCATE", "DROP", "ALTER",
    "CREATE", "GRANT", "REVOKE", "RENAME", "ATTACH", "DETACH", "LOAD", "VACUUM", "REINDEX",
    "LOCK", "UNLOCK", "CALL", "INTO",
];

/// Scan an uppercase, comment-stripped SQL string for any DML/DDL keyword,
/// respecting `'...'`, `"..."`, and `` `...` `` quoted regions so that a
/// keyword inside a string literal or quoted identifier does not match.
///
/// Returns the first offending keyword, if any. This is the core defence
/// against `WITH`-CTE DML bypasses (REF-41) — a CTE body or trailing
/// statement containing `INSERT`/`UPDATE`/`DELETE`/`COPY`/etc. is rejected.
fn scan_for_write_keyword(sql: &str) -> Option<&'static str> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            // Single-quoted string literal. Honor SQL `''` escape.
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // Double-quoted identifier (Postgres/SQLite) or string (MySQL ANSI_QUOTES off).
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // Backtick-quoted identifier (MySQL).
            b'`' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'`' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            // Identifier/keyword token: [A-Z_][A-Z0-9_]*. The token start cannot
            // be a digit, because `0DELETE` is not a valid identifier and would
            // not be interpreted as the DELETE keyword by any engine.
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let token = &sql[start..i];
                if let Some(kw) = WRITE_KEYWORDS.iter().find(|k| **k == token) {
                    return Some(*kw);
                }
            }
            _ => {
                i += 1;
            }
        }
    }
    None
}

/// Validate a `WITH`-prefixed (CTE) query for read-only safety.
///
/// Returns `true` only when:
/// 1. The query starts with `WITH ` (the bare CTE prefix), and
/// 2. No DML/DDL keyword appears anywhere in the (already comment-stripped,
///    uppercased) body — including inside CTE bodies or in the trailing
///    statement after the CTE definitions.
///
/// This closes the WITH-CTE DML bypass class for all three engines (REF-41):
/// PostgreSQL writable CTEs (`WITH x AS (INSERT...) SELECT...`), MySQL/SQLite
/// `WITH ... INSERT/UPDATE/DELETE ...` and similar trailing-DML forms.
fn is_safe_cte_query(sql: &str) -> bool {
    sql.starts_with("WITH ") && scan_for_write_keyword(sql).is_none()
}

// PostgreSQL read-only check
fn is_read_only_postgres(sql: &str) -> bool {
    // Strip EXPLAIN/EXPLAIN ANALYZE prefix
    let sql = strip_explain_prefix(sql);
    let sql = sql.trim();

    if sql.starts_with("SELECT ") {
        // PostgreSQL `SELECT ... INTO new_table` (incl. INTO TEMP /
        // TEMPORARY / UNLOGGED [TABLE] variants) is shorthand for
        // `CREATE TABLE AS SELECT` — a DDL write that the bare SELECT
        // prefix would otherwise admit (REF-42). Reuse the same
        // quote-aware DML/DDL keyword scan that protects WITH-CTE
        // queries (REF-41) so both read prefixes share one line of
        // defense against hidden writes. `INTO` is listed in
        // WRITE_KEYWORDS, and the scanner walks past `'...'` and
        // `"..."` quoted regions so `INTO` inside a string literal
        // or quoted identifier is not a false positive.
        return scan_for_write_keyword(sql).is_none();
    }

    is_safe_cte_query(sql)
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("START TRANSACTION")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE")
}

// MySQL read-only check
fn is_read_only_mysql(sql: &str) -> bool {
    // Strip EXPLAIN prefix
    let sql = strip_explain_prefix(sql);
    let sql = sql.trim();

    let allowed_prefix = sql.starts_with("SELECT ")
        || is_safe_cte_query(sql)
        || sql.starts_with("SHOW ")
        || sql.starts_with("DESCRIBE ")
        || sql.starts_with("DESC ")
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("START TRANSACTION")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE");

    if !allowed_prefix {
        return false;
    }

    // `SELECT ... INTO OUTFILE` / `INTO DUMPFILE` writes query results to the
    // MySQL server's filesystem; `SELECT ... INTO @var` assigns to a session
    // variable. All three pass the `SELECT ` prefix check above but are not
    // read-only (REF-43). The `WITH`-CTE path is already covered by
    // `is_safe_cte_query`'s `INTO` keyword scan (REF-41), so we only need to
    // patch the bare `SELECT ` path here.
    if sql.starts_with("SELECT ") {
        let normalized = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalized.contains(" INTO OUTFILE")
            || normalized.contains(" INTO DUMPFILE")
            || normalized.contains(" INTO @")
        {
            return false;
        }
    }

    true
}

/// SQLite PRAGMAs that are read-only when invoked in argument form
/// (`PRAGMA name(arg)`). These either always require an argument
/// (e.g. `table_info`) or treat the argument as a query parameter rather than
/// a setter value, so the parenthesized form does not write state.
///
/// Names are uppercase to match the preprocessed SQL produced by
/// `preprocess_sql`. See REF-44.
const READ_ONLY_SQLITE_PRAGMAS_WITH_ARGS: &[&str] = &[
    "TABLE_INFO",
    "TABLE_XINFO",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "FOREIGN_KEY_CHECK",
    "INTEGRITY_CHECK",
    "QUICK_CHECK",
];

/// SQLite PRAGMAs whose bare form (`PRAGMA name`) is a pure read.
///
/// Several entries (`JOURNAL_MODE`, `USER_VERSION`, `PAGE_SIZE`, …) are
/// settable PRAGMAs: their `= value` and `(value)` invocation forms mutate
/// state. Only the bare query form is admitted here; the assignment form is
/// rejected by `is_safe_pragma`, and the parenthesized form is rejected
/// because these names are not in `READ_ONLY_SQLITE_PRAGMAS_WITH_ARGS`.
///
/// See REF-44.
const READ_ONLY_SQLITE_PRAGMAS_BARE: &[&str] = &[
    // Pure introspection
    "TABLE_LIST",
    "DATABASE_LIST",
    "SCHEMA_VERSION",
    "DATA_VERSION",
    "COMPILE_OPTIONS",
    "COLLATION_LIST",
    "FUNCTION_LIST",
    "MODULE_LIST",
    "PRAGMA_LIST",
    "INTEGRITY_CHECK",
    "QUICK_CHECK",
    "FREELIST_COUNT",
    "PAGE_COUNT",
    // Settable PRAGMAs — read-only only in the bare form
    "JOURNAL_MODE",
    "USER_VERSION",
    "APPLICATION_ID",
    "ENCODING",
    "PAGE_SIZE",
    "AUTO_VACUUM",
    "MAX_PAGE_COUNT",
    "FOREIGN_KEYS",
    "SYNCHRONOUS",
    "CACHE_SIZE",
];

/// Validate a `PRAGMA …` statement against the SQLite read-only allowlist.
///
/// Closes the write-PRAGMA bypass (REF-44) where the previous blanket
/// `starts_with("PRAGMA ")` check admitted destructive PRAGMAs such as
/// `PRAGMA writable_schema = 1`, `PRAGMA wal_checkpoint`, `PRAGMA optimize`,
/// and `PRAGMA page_size = 4096`.
///
/// Accepts only:
/// * `PRAGMA name` — bare read form where `name` is in
///   [`READ_ONLY_SQLITE_PRAGMAS_BARE`].
/// * `PRAGMA name(arg)` — argument form where `name` is in
///   [`READ_ONLY_SQLITE_PRAGMAS_WITH_ARGS`].
///
/// Rejects:
/// * Any query containing `=` (the `PRAGMA name = value` setter, including
///   `writable_schema = 1`, `journal_mode = WAL`, `auto_vacuum = FULL`).
/// * `PRAGMA name(arg)` for any name outside the argument-form allowlist
///   (e.g. `wal_checkpoint(TRUNCATE)`, `journal_mode(WAL)`).
/// * Any name not present in either allowlist (e.g. `wal_checkpoint`,
///   `optimize`, `incremental_vacuum`).
fn is_safe_pragma(sql: &str) -> bool {
    let Some(rest) = sql.strip_prefix("PRAGMA ") else {
        return false;
    };
    let rest = rest.trim().trim_end_matches(';').trim();
    if rest.is_empty() {
        return false;
    }

    // `=` always indicates the assignment / setter form. Reject unconditionally
    // — there are no read-only PRAGMAs that use `=` in their syntax.
    if rest.contains('=') {
        return false;
    }

    // Split into `name` and optional `(args)`.
    let (name, has_args) = match rest.find('(') {
        Some(open) => {
            if !rest.ends_with(')') {
                return false;
            }
            (rest[..open].trim(), true)
        }
        None => (rest, false),
    };

    // The PRAGMA name must be a single identifier — anything else (trailing
    // tokens like `PRAGMA journal_mode WAL`) is rejected.
    if name.is_empty() || name.contains(|c: char| c.is_whitespace()) {
        return false;
    }

    if has_args {
        READ_ONLY_SQLITE_PRAGMAS_WITH_ARGS.contains(&name)
    } else {
        READ_ONLY_SQLITE_PRAGMAS_BARE.contains(&name)
    }
}

// SQLite read-only check
fn is_read_only_sqlite(sql: &str) -> bool {
    // Strip EXPLAIN prefix
    let sql = strip_explain_prefix(sql);
    let sql = sql.trim();

    sql.starts_with("SELECT ")
        || is_safe_cte_query(sql)
        || is_safe_pragma(sql)
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Preprocessing tests

    #[test]
    fn test_preprocess_empty_query() {
        let result = preprocess_sql("");
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Query cannot be empty"));
    }

    #[test]
    fn test_preprocess_whitespace_trimming() {
        let result = preprocess_sql("  SELECT * FROM users  ").unwrap();
        assert!(result.starts_with("SELECT"));
        assert!(!result.starts_with(' '));
    }

    #[test]
    fn test_preprocess_line_comments() {
        let result =
            preprocess_sql("SELECT * FROM users -- this is a comment\nWHERE id = 1").unwrap();
        assert!(result.contains("SELECT"));
        assert!(result.contains("WHERE"));
        assert!(!result.contains("this is a comment"));
    }

    #[test]
    fn test_preprocess_block_comments() {
        let result = preprocess_sql("SELECT * /* block comment */ FROM users").unwrap();
        assert!(result.contains("SELECT"));
        assert!(result.contains("FROM"));
        assert!(!result.contains("block comment"));
    }

    #[test]
    fn test_preprocess_multi_statement_detection() {
        let result = preprocess_sql("SELECT * FROM users; DROP TABLE users;");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Multi-statement queries are not supported"));
    }

    #[test]
    fn test_preprocess_trailing_semicolon_allowed() {
        let result = preprocess_sql("SELECT * FROM users;");
        assert!(result.is_ok());
    }

    #[test]
    fn test_preprocess_uppercase_normalization() {
        let result = preprocess_sql("select * from users").unwrap();
        assert!(result.starts_with("SELECT"));
        assert!(result.contains("FROM"));
    }

    // PostgreSQL read-only tests

    #[test]
    fn test_postgres_select() {
        let caps = Capabilities::default();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
    }

    #[test]
    fn test_postgres_insert_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "INSERT INTO users (name) VALUES ('test')",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_err());
        let error_message = result.unwrap_err().message();
        assert!(error_message.contains("Plenum is read-only"));
        assert!(error_message.contains("Please run this query manually"));
    }

    #[test]
    fn test_postgres_update_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "UPDATE users SET name = 'test' WHERE id = 1",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_postgres_delete_rejected() {
        let caps = Capabilities::default();
        let result =
            validate_query("DELETE FROM users WHERE id = 1", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_postgres_create_table_rejected() {
        let caps = Capabilities::default();
        let result = validate_query("CREATE TABLE test (id INT)", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_postgres_drop_table_rejected() {
        let caps = Capabilities::default();
        let result = validate_query("DROP TABLE users", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_postgres_explain_select() {
        let caps = Capabilities::default();
        let result = validate_query("EXPLAIN SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
    }

    #[test]
    fn test_postgres_explain_analyze() {
        let caps = Capabilities::default();
        let result =
            validate_query("EXPLAIN ANALYZE SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
    }

    #[test]
    fn test_postgres_transaction_control() {
        let caps = Capabilities::default();
        assert!(validate_query("BEGIN", &caps, DatabaseType::Postgres).is_ok());
        assert!(validate_query("COMMIT", &caps, DatabaseType::Postgres).is_ok());
        assert!(validate_query("ROLLBACK", &caps, DatabaseType::Postgres).is_ok());
    }

    #[test]
    fn test_postgres_cte_allowed() {
        let caps = Capabilities::default();
        let result = validate_query(
            "WITH cte AS (SELECT * FROM users) SELECT * FROM cte",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_ok());
    }

    // REF-42: PostgreSQL `SELECT ... INTO new_table` is a DDL shorthand for
    // `CREATE TABLE AS SELECT`. The bare `SELECT ` prefix check previously
    // admitted it. Verify all documented variants (plain, TEMP, TEMPORARY,
    // UNLOGGED, with the optional TABLE keyword) are rejected.

    fn assert_postgres_rejected_as_read_only(sql: &str) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, DatabaseType::Postgres);
        assert!(result.is_err(), "expected rejection: {sql}");
        assert!(
            result.unwrap_err().message().contains("Plenum is read-only"),
            "wrong error message for: {sql}"
        );
    }

    #[test]
    fn test_postgres_select_into_rejected() {
        assert_postgres_rejected_as_read_only("SELECT * FROM users INTO backup");
    }

    #[test]
    fn test_postgres_select_into_temp_rejected() {
        assert_postgres_rejected_as_read_only("SELECT id FROM users INTO TEMP backup");
    }

    #[test]
    fn test_postgres_select_into_temporary_rejected() {
        assert_postgres_rejected_as_read_only("SELECT id FROM users INTO TEMPORARY backup");
    }

    #[test]
    fn test_postgres_select_into_unlogged_rejected() {
        assert_postgres_rejected_as_read_only("SELECT id FROM users INTO UNLOGGED backup");
    }

    #[test]
    fn test_postgres_select_into_temp_table_rejected() {
        // Multi-word variant: INTO TEMP TABLE. Tokenized scan still finds INTO.
        assert_postgres_rejected_as_read_only("SELECT * FROM users INTO TEMP TABLE temp_users");
    }

    #[test]
    fn test_postgres_select_into_column_filtered_rejected() {
        assert_postgres_rejected_as_read_only("SELECT id, email FROM users INTO email_dump");
    }

    // Acceptance criteria: a subquery-style `IN (SELECT ...)` is not a SELECT
    // INTO and must remain accepted. The token `IN` is distinct from `INTO`,
    // so the scan must not false-positive here.
    #[test]
    fn test_postgres_subquery_in_select_allowed() {
        let caps = Capabilities::default();
        let result = validate_query(
            "SELECT * FROM users WHERE id IN (SELECT id FROM admins)",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_ok());
    }

    // INTO embedded inside a string literal is not a real INTO clause. The
    // quote-aware scanner must skip it to avoid rejecting legitimate reads.
    #[test]
    fn test_postgres_select_into_inside_string_literal_allowed() {
        let caps = Capabilities::default();
        let result = validate_query(
            "SELECT 'data flowing into target' FROM events",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_ok());
    }

    // Defense in depth: a CTE wrapper must not launder a SELECT INTO. The
    // `WITH`-prefix path is handled by `is_safe_cte_query` (REF-41), which
    // also scans for `INTO` — assert it explicitly here so a future
    // refactor that splits the two paths can't silently regress this.
    #[test]
    fn test_postgres_with_cte_select_into_rejected() {
        assert_postgres_rejected_as_read_only(
            "WITH cte AS (SELECT * FROM users) SELECT * FROM cte INTO backup",
        );
    }

    // MySQL read-only tests

    #[test]
    fn test_mysql_select() {
        let caps = Capabilities::default();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::MySQL);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mysql_replace_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "REPLACE INTO users (id, name) VALUES (1, 'test')",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_mysql_lock_tables_rejected() {
        let caps = Capabilities::default();
        let result = validate_query("LOCK TABLES users WRITE", &caps, DatabaseType::MySQL);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_mysql_show_statement() {
        let caps = Capabilities::default();
        let result = validate_query("SHOW TABLES", &caps, DatabaseType::MySQL);
        assert!(result.is_ok());
    }

    // REF-43: `SELECT ... INTO OUTFILE` writes query results to the MySQL
    // server's filesystem. It passes the bare `SELECT ` prefix check but is
    // not a read-only operation.
    #[test]
    fn test_mysql_select_into_outfile_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "SELECT * FROM users INTO OUTFILE '/tmp/users_dump.csv'",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    // `SELECT ... INTO OUTFILE` is commonly split across lines with field/line
    // delimiter clauses. Whitespace normalization must catch the multi-line
    // form too.
    #[test]
    fn test_mysql_select_into_outfile_multiline_rejected() {
        let caps = Capabilities::default();
        let sql = "SELECT id, email FROM users\n  INTO OUTFILE '/var/lib/mysql-files/dump.csv'\n  FIELDS TERMINATED BY ','";
        let result = validate_query(sql, &caps, DatabaseType::MySQL);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    // `SELECT ... INTO DUMPFILE` writes a single row to the server's
    // filesystem as a binary blob.
    #[test]
    fn test_mysql_select_into_dumpfile_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "SELECT col FROM t WHERE id = 1 INTO DUMPFILE '/tmp/blob.bin'",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    // `SELECT ... INTO @var` assigns the query result to a session variable.
    // Lower severity than file writes, but still a session-scoped write.
    #[test]
    fn test_mysql_select_into_user_variable_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "SELECT COUNT(*) INTO @user_count FROM users",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    // Case-insensitivity sanity: lower-case `into outfile` must still trip
    // the check, because preprocessing uppercases the query.
    #[test]
    fn test_mysql_select_into_outfile_lowercase_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "select * from users into outfile '/tmp/x'",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
    }

    // Regression guard: a normal SELECT against a table named `outfile`
    // (no INTO clause) must still be permitted.
    #[test]
    fn test_mysql_plain_select_still_allowed() {
        let caps = Capabilities::default();
        assert!(validate_query("SELECT * FROM users", &caps, DatabaseType::MySQL).is_ok());
        assert!(
            validate_query("SELECT * FROM outfile_log", &caps, DatabaseType::MySQL).is_ok()
        );
    }

    // WITH-CTE path is covered by `is_safe_cte_query`'s INTO keyword scan
    // (REF-41), but pin the behavior here for the MySQL OUTFILE flavor so a
    // future refactor of the CTE check can't silently re-open this bypass.
    #[test]
    fn test_mysql_with_cte_into_outfile_rejected() {
        let caps = Capabilities::default();
        let result = validate_query(
            "WITH cte AS (SELECT * FROM users) SELECT * FROM cte INTO OUTFILE '/tmp/x'",
            &caps,
            DatabaseType::MySQL,
        );
        assert!(result.is_err());
    }

    // SQLite read-only tests

    #[test]
    fn test_sqlite_select() {
        let caps = Capabilities::default();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::SQLite);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sqlite_pragma() {
        let caps = Capabilities::default();
        let result = validate_query("PRAGMA table_info(users)", &caps, DatabaseType::SQLite);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sqlite_vacuum_rejected() {
        let caps = Capabilities::default();
        let result = validate_query("VACUUM", &caps, DatabaseType::SQLite);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    #[test]
    fn test_sqlite_insert_rejected() {
        let caps = Capabilities::default();
        let result =
            validate_query("INSERT INTO users (name) VALUES ('test')", &caps, DatabaseType::SQLite);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Plenum is read-only"));
    }

    // SQLite PRAGMA allowlist tests (REF-44).
    //
    // The blanket `starts_with("PRAGMA ")` check used to admit any PRAGMA,
    // including `writable_schema = 1` (the classic sqlite_master escape) and
    // `wal_checkpoint`/`optimize` (state-mutating PRAGMAs). The fix tightens
    // SQLite read-only validation to a two-form allowlist; these tests pin the
    // exact bypasses called out in the issue.

    fn assert_sqlite_rejected(sql: &str) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, DatabaseType::SQLite);
        assert!(result.is_err(), "expected SQLite to reject: {sql}");
        let err = result.unwrap_err();
        assert!(
            err.message().contains("Plenum is read-only"),
            "expected read-only rejection for {sql}, got: {}",
            err.message()
        );
    }

    fn assert_sqlite_allowed(sql: &str) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, DatabaseType::SQLite);
        assert!(
            result.is_ok(),
            "expected SQLite to allow: {sql}, got: {}",
            result.unwrap_err().message()
        );
    }

    // --- Accepted: read-only PRAGMAs (must keep working) ---

    #[test]
    fn test_sqlite_pragma_table_info_allowed() {
        assert_sqlite_allowed("PRAGMA table_info(users)");
    }

    #[test]
    fn test_sqlite_pragma_database_list_allowed() {
        assert_sqlite_allowed("PRAGMA database_list");
    }

    #[test]
    fn test_sqlite_pragma_table_list_allowed() {
        assert_sqlite_allowed("PRAGMA table_list");
    }

    #[test]
    fn test_sqlite_pragma_index_list_allowed() {
        assert_sqlite_allowed("PRAGMA index_list(users)");
    }

    #[test]
    fn test_sqlite_pragma_foreign_key_list_allowed() {
        assert_sqlite_allowed("PRAGMA foreign_key_list(users)");
    }

    #[test]
    fn test_sqlite_pragma_integrity_check_allowed() {
        assert_sqlite_allowed("PRAGMA integrity_check");
    }

    #[test]
    fn test_sqlite_pragma_compile_options_allowed() {
        assert_sqlite_allowed("PRAGMA compile_options");
    }

    #[test]
    fn test_sqlite_pragma_journal_mode_bare_allowed() {
        // Bare query form: returns current journal mode, does not set it.
        assert_sqlite_allowed("PRAGMA journal_mode");
    }

    #[test]
    fn test_sqlite_pragma_user_version_bare_allowed() {
        assert_sqlite_allowed("PRAGMA user_version");
    }

    #[test]
    fn test_sqlite_pragma_schema_version_bare_allowed() {
        assert_sqlite_allowed("PRAGMA schema_version");
    }

    #[test]
    fn test_sqlite_pragma_case_insensitive_allowed() {
        // preprocess_sql uppercases the entire query, so lowercase PRAGMA
        // names must still flow through the allowlist correctly.
        assert_sqlite_allowed("pragma table_info(users)");
    }

    // --- Rejected: writable_schema (the most dangerous bypass) ---

    #[test]
    fn test_sqlite_pragma_writable_schema_eq_1_rejected() {
        // The classic sqlite_master escape: enables arbitrary modification of
        // the schema table on the next query. Must be rejected.
        assert_sqlite_rejected("PRAGMA writable_schema = 1");
    }

    #[test]
    fn test_sqlite_pragma_writable_schema_eq_on_rejected() {
        assert_sqlite_rejected("PRAGMA writable_schema = ON");
    }

    #[test]
    fn test_sqlite_pragma_writable_schema_eq_0_rejected() {
        // Even resetting writable_schema requires write capability we don't grant.
        assert_sqlite_rejected("PRAGMA writable_schema = 0");
    }

    #[test]
    fn test_sqlite_pragma_writable_schema_paren_rejected() {
        // Parenthesized setter form is also a write.
        assert_sqlite_rejected("PRAGMA writable_schema(1)");
    }

    #[test]
    fn test_sqlite_pragma_writable_schema_bare_rejected() {
        // Not in the bare allowlist — even querying it shouldn't be needed
        // and is rejected by default-deny.
        assert_sqlite_rejected("PRAGMA writable_schema");
    }

    // --- Rejected: wal_checkpoint (modifies the -wal file) ---

    #[test]
    fn test_sqlite_pragma_wal_checkpoint_bare_rejected() {
        assert_sqlite_rejected("PRAGMA wal_checkpoint");
    }

    #[test]
    fn test_sqlite_pragma_wal_checkpoint_full_rejected() {
        assert_sqlite_rejected("PRAGMA wal_checkpoint(FULL)");
    }

    #[test]
    fn test_sqlite_pragma_wal_checkpoint_restart_rejected() {
        assert_sqlite_rejected("PRAGMA wal_checkpoint(RESTART)");
    }

    #[test]
    fn test_sqlite_pragma_wal_checkpoint_truncate_rejected() {
        assert_sqlite_rejected("PRAGMA wal_checkpoint(TRUNCATE)");
    }

    // --- Rejected: optimize and vacuum-adjacent PRAGMAs ---

    #[test]
    fn test_sqlite_pragma_optimize_rejected() {
        assert_sqlite_rejected("PRAGMA optimize");
    }

    #[test]
    fn test_sqlite_pragma_incremental_vacuum_rejected() {
        assert_sqlite_rejected("PRAGMA incremental_vacuum");
    }

    #[test]
    fn test_sqlite_pragma_auto_vacuum_eq_full_rejected() {
        assert_sqlite_rejected("PRAGMA auto_vacuum = FULL");
    }

    #[test]
    fn test_sqlite_pragma_page_size_eq_rejected() {
        assert_sqlite_rejected("PRAGMA page_size = 4096");
    }

    #[test]
    fn test_sqlite_pragma_journal_mode_eq_wal_rejected() {
        // Settable PRAGMAs: bare form is allowed (read), `= value` and `(value)` are not.
        assert_sqlite_rejected("PRAGMA journal_mode = WAL");
    }

    #[test]
    fn test_sqlite_pragma_journal_mode_paren_wal_rejected() {
        assert_sqlite_rejected("PRAGMA journal_mode(WAL)");
    }

    #[test]
    fn test_sqlite_pragma_user_version_eq_rejected() {
        assert_sqlite_rejected("PRAGMA user_version = 42");
    }

    // --- Rejected: unknown / fail-safe default ---

    #[test]
    fn test_sqlite_pragma_unknown_name_rejected() {
        // Fail-safe default: anything not on the allowlist is rejected, so
        // future SQLite versions that introduce new write PRAGMAs are safe
        // until the allowlist is explicitly updated.
        assert_sqlite_rejected("PRAGMA something_that_does_not_exist");
    }

    #[test]
    fn test_sqlite_pragma_empty_rejected() {
        assert_sqlite_rejected("PRAGMA ");
    }

    #[test]
    fn test_sqlite_pragma_trailing_token_rejected() {
        // `PRAGMA journal_mode WAL` (no `=`, no parens) is malformed in SQLite
        // syntactically but also clearly an attempt to set; reject it.
        assert_sqlite_rejected("PRAGMA journal_mode WAL");
    }

    // Edge case tests

    #[test]
    fn test_case_insensitivity() {
        let caps = Capabilities::default();
        let result = validate_query("select * from users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
    }

    #[test]
    fn test_mixed_case_with_comments() {
        let caps = Capabilities::default();
        let result = validate_query(
            "-- Query users\nSeLeCt * FrOm UsErS -- get all",
            &caps,
            DatabaseType::Postgres,
        );
        assert!(result.is_ok());
    }

    // Multi-statement rejection through the public `validate_query` API.
    // Multi-statement preprocessing is engine-agnostic, but agents reach it
    // through engine-specific code paths — assert all three engines reject
    // identically so a future engine-specific bypass would be caught here.

    fn assert_multi_statement_rejected(sql: &str, engine: DatabaseType) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, engine);
        assert!(
            result.is_err(),
            "expected {engine:?} to reject multi-statement query: {sql}"
        );
        let err = result.unwrap_err();
        assert_eq!(err.error_code(), "INVALID_INPUT", "engine={engine:?}");
        assert!(
            err.message().contains("Multi-statement queries are not supported"),
            "engine={engine:?} message={}",
            err.message()
        );
    }

    #[test]
    fn test_multi_statement_rejected_postgres() {
        assert_multi_statement_rejected("SELECT 1; SELECT 2", DatabaseType::Postgres);
    }

    #[test]
    fn test_multi_statement_rejected_mysql() {
        assert_multi_statement_rejected("SELECT 1; SELECT 2", DatabaseType::MySQL);
    }

    #[test]
    fn test_multi_statement_rejected_sqlite() {
        assert_multi_statement_rejected("SELECT 1; SELECT 2", DatabaseType::SQLite);
    }

    #[test]
    fn test_multi_statement_with_ddl_rejected_all_engines() {
        // Classic SQL-injection-style payload: read followed by DDL.
        // All three engines must reject before any engine-specific dispatch.
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_multi_statement_rejected("SELECT * FROM users; DROP TABLE users", engine);
        }
    }

    // CTE DML-bypass tests (REF-41).
    //
    // The prior `WITH ` prefix allowlist let attackers smuggle writes inside
    // CTE bodies (Postgres writable CTEs) or in the trailing statement after
    // the CTE definition (MySQL/SQLite). These tests cover every variant from
    // the audit report and lock in the fix for all three engines.

    fn assert_cte_rejected(sql: &str, engine: DatabaseType) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, engine);
        assert!(result.is_err(), "expected {engine:?} to reject CTE-DML: {sql}");
        let err = result.unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION", "engine={engine:?}");
        assert!(
            err.message().contains("Plenum is read-only"),
            "engine={engine:?} message={}",
            err.message()
        );
    }

    fn assert_cte_allowed(sql: &str, engine: DatabaseType) {
        let caps = Capabilities::default();
        let result = validate_query(sql, &caps, engine);
        assert!(
            result.is_ok(),
            "expected {engine:?} to allow CTE: {sql} (err={:?})",
            result.err()
        );
    }

    // Postgres writable CTE: INSERT inside CTE body.
    #[test]
    fn test_cte_with_insert_rejected_all_engines() {
        let sql =
            "WITH x AS (INSERT INTO users (name) VALUES ('hacked') RETURNING id) SELECT * FROM x";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // Postgres writable CTE: UPDATE inside CTE body.
    #[test]
    fn test_cte_with_update_rejected_all_engines() {
        let sql =
            "WITH x AS (UPDATE users SET name = 'hacked' WHERE id = 1 RETURNING *) SELECT * FROM x";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // Postgres writable CTE: DELETE inside CTE body.
    #[test]
    fn test_cte_with_delete_rejected_all_engines() {
        let sql = "WITH x AS (DELETE FROM users WHERE id = 1 RETURNING *) SELECT * FROM x";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // MySQL 8.0 / SQLite: CTE followed by INSERT.
    #[test]
    fn test_cte_then_insert_rejected_all_engines() {
        let sql = "WITH cte AS (SELECT id FROM users WHERE active = 1) INSERT INTO archive SELECT * FROM cte";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // MySQL 8.0: CTE followed by UPDATE.
    #[test]
    fn test_cte_then_update_rejected_all_engines() {
        let sql = "WITH cte AS (SELECT id FROM users WHERE active = 1) UPDATE users SET flagged = 1 WHERE id IN (SELECT id FROM cte)";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // SQLite: CTE followed by DELETE.
    #[test]
    fn test_cte_then_delete_rejected_all_engines() {
        let sql = "WITH cte AS (SELECT id FROM users LIMIT 10) DELETE FROM users WHERE id IN (SELECT id FROM cte)";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_rejected(sql, engine);
        }
    }

    // Postgres COPY hidden inside CTE.
    #[test]
    fn test_cte_with_copy_rejected_postgres() {
        let sql = "WITH x AS (COPY users FROM '/tmp/data.csv') SELECT 1";
        assert_cte_rejected(sql, DatabaseType::Postgres);
    }

    // Legitimate read-only CTEs must still pass on every engine.
    #[test]
    fn test_cte_with_select_allowed_all_engines() {
        let sql = "WITH cte AS (SELECT * FROM users) SELECT * FROM cte";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_allowed(sql, engine);
        }
    }

    // Recursive read-only CTE — common legitimate pattern.
    #[test]
    fn test_cte_recursive_select_allowed_all_engines() {
        let sql = "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM numbers WHERE n < 10) SELECT n FROM numbers";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_allowed(sql, engine);
        }
    }

    // The tokenizer must not match keywords that occur inside string literals.
    #[test]
    fn test_cte_with_keyword_in_string_literal_allowed() {
        let sql = "WITH cte AS (SELECT * FROM users WHERE note = 'INSERT INTO denied') SELECT * FROM cte";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_allowed(sql, engine);
        }
    }

    // Identifier substrings must not match the keyword token (e.g., `delete_at`).
    #[test]
    fn test_cte_with_keyword_substring_identifier_allowed() {
        let sql = "WITH cte AS (SELECT delete_at, updated_count FROM audit) SELECT * FROM cte";
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_cte_allowed(sql, engine);
        }
    }

    // Backtick-quoted identifiers in MySQL must not trip the scanner.
    #[test]
    fn test_cte_with_backtick_identifier_allowed_mysql() {
        let sql = "WITH cte AS (SELECT `delete`, `update` FROM `audit`) SELECT * FROM cte";
        assert_cte_allowed(sql, DatabaseType::MySQL);
    }

    // EXPLAIN-prefixed CTEs are exercised through the existing EXPLAIN strip.
    #[test]
    fn test_explain_cte_with_insert_rejected_postgres() {
        let sql = "EXPLAIN WITH x AS (INSERT INTO users (name) VALUES ('hacked') RETURNING id) SELECT * FROM x";
        assert_cte_rejected(sql, DatabaseType::Postgres);
    }

    #[test]
    fn test_explain_cte_with_select_allowed_postgres() {
        let sql = "EXPLAIN WITH cte AS (SELECT * FROM users) SELECT * FROM cte";
        assert_cte_allowed(sql, DatabaseType::Postgres);
    }

    // REF-266: --check-only / check_only pre-execution validation tests.
    //
    // validate_query is the authorization gate for --check-only. The CLI and MCP
    // layers call it first; if it returns Ok(()) the response is
    // { would_execute: true, category: "read" } with no DB call.
    // If it returns Err the standard CAPABILITY_VIOLATION error envelope is emitted.
    // These tests pin the expected per-engine behavior.

    #[test]
    fn test_check_only_read_permitted_postgres() {
        let caps = Capabilities::default();
        assert!(validate_query("SELECT id, email FROM users", &caps, DatabaseType::Postgres).is_ok());
    }

    #[test]
    fn test_check_only_write_rejected_postgres() {
        let caps = Capabilities::default();
        let err = validate_query("INSERT INTO users (name) VALUES ('x')", &caps, DatabaseType::Postgres).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_ddl_rejected_postgres() {
        let caps = Capabilities::default();
        let err = validate_query("CREATE TABLE t (id INT)", &caps, DatabaseType::Postgres).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_read_permitted_mysql() {
        let caps = Capabilities::default();
        assert!(validate_query("SELECT id FROM users", &caps, DatabaseType::MySQL).is_ok());
    }

    #[test]
    fn test_check_only_write_rejected_mysql() {
        let caps = Capabilities::default();
        let err = validate_query("DELETE FROM users WHERE id = 1", &caps, DatabaseType::MySQL).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_ddl_rejected_mysql() {
        let caps = Capabilities::default();
        let err = validate_query("DROP TABLE users", &caps, DatabaseType::MySQL).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_read_permitted_sqlite() {
        let caps = Capabilities::default();
        assert!(validate_query("SELECT * FROM items", &caps, DatabaseType::SQLite).is_ok());
    }

    #[test]
    fn test_check_only_write_rejected_sqlite() {
        let caps = Capabilities::default();
        let err = validate_query("UPDATE items SET qty = 0", &caps, DatabaseType::SQLite).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_ddl_rejected_sqlite() {
        let caps = Capabilities::default();
        let err = validate_query("ALTER TABLE items ADD COLUMN x TEXT", &caps, DatabaseType::SQLite).unwrap_err();
        assert_eq!(err.error_code(), "CAPABILITY_VIOLATION");
    }

    #[test]
    fn test_check_only_empty_rejected_all_engines() {
        let caps = Capabilities::default();
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            let err = validate_query("", &caps, engine).unwrap_err();
            assert_eq!(err.error_code(), "INVALID_INPUT", "engine={engine:?}");
        }
    }

    #[test]
    fn test_check_only_multi_statement_rejected_all_engines() {
        let caps = Capabilities::default();
        for engine in [DatabaseType::Postgres, DatabaseType::MySQL, DatabaseType::SQLite] {
            let err = validate_query("SELECT 1; SELECT 2", &caps, engine).unwrap_err();
            assert_eq!(err.error_code(), "INVALID_INPUT", "engine={engine:?}");
        }
    }

    // Defense in depth: a CTE that smuggles a `SELECT ... INTO new_table` write
    // (Postgres CTAS-via-INTO) must be rejected too. The `INTO` token is in
    // `WRITE_KEYWORDS` so `is_safe_cte_query` rejects this regardless of
    // engine-level INTO-OUTFILE special cases (REF-42 / REF-43).
    #[test]
    fn test_cte_with_select_into_rejected_postgres() {
        let sql = "WITH cte AS (SELECT * FROM users) SELECT * INTO archive FROM cte";
        assert_cte_rejected(sql, DatabaseType::Postgres);
    }
}
