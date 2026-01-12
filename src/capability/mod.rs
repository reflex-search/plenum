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

// PostgreSQL read-only check
fn is_read_only_postgres(sql: &str) -> bool {
    // Strip EXPLAIN/EXPLAIN ANALYZE prefix
    let sql = strip_explain_prefix(sql);
    let sql = sql.trim();

    sql.starts_with("SELECT ")
        || sql.starts_with("WITH ")  // CTEs are allowed (conservative: assume read-only)
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

    sql.starts_with("SELECT ")
        || sql.starts_with("WITH ")  // CTEs are allowed (conservative: assume read-only)
        || sql.starts_with("SHOW ")
        || sql.starts_with("DESCRIBE ")
        || sql.starts_with("DESC ")
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("START TRANSACTION")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE")
}

// SQLite read-only check
fn is_read_only_sqlite(sql: &str) -> bool {
    // Strip EXPLAIN prefix
    let sql = strip_explain_prefix(sql);
    let sql = sql.trim();

    sql.starts_with("SELECT ")
        || sql.starts_with("WITH ")  // CTEs are allowed (conservative: assume read-only)
        || sql.starts_with("PRAGMA ")
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
}
