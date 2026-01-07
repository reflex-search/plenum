//! Capability Validation and SQL Categorization
//!
//! This module implements capability-based query validation.
//! All queries are categorized (Read-only, Write, DDL) and checked against
//! the provided capabilities BEFORE execution.
//!
//! # SQL Categorization Strategy
//! - Regex-based pattern matching
//! - Engine-specific implementations (no shared SQL helpers)
//! - Conservative approach (fail-safe defaults)
//!
//! # Capability Hierarchy
//! - Read-only: Default mode, SELECT queries only
//! - Write: Requires `allow_write`, enables INSERT/UPDATE/DELETE
//! - DDL: Requires `allow_ddl`, enables DDL operations (implies write)

use crate::engine::{Capabilities, DatabaseType};
use crate::error::{PlenumError, Result};

/// SQL query category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryCategory {
    /// Read-only query (SELECT, WITH...SELECT, EXPLAIN SELECT)
    ReadOnly,
    /// Write query (INSERT, UPDATE, DELETE, stored procedure calls)
    Write,
    /// DDL query (CREATE, DROP, ALTER, TRUNCATE, RENAME)
    DDL,
}

/// Validate query against capabilities
///
/// This function categorizes the query and checks if the capabilities permit it.
/// Returns an error if the query violates capability constraints.
pub fn validate_query(
    sql: &str,
    caps: &Capabilities,
    engine: DatabaseType,
) -> Result<QueryCategory> {
    // Pre-process SQL
    let processed = preprocess_sql(sql)?;

    // Categorize query (engine-specific)
    let category = categorize_query(&processed, engine)?;

    // Check capabilities
    match category {
        QueryCategory::ReadOnly => {
            // Always permitted
            Ok(category)
        }
        QueryCategory::Write => {
            // Requires allow_write OR allow_ddl (DDL implies write)
            if caps.can_write() {
                Ok(category)
            } else {
                Err(PlenumError::capability_violation(
                    "Write operations require --allow-write flag",
                ))
            }
        }
        QueryCategory::DDL => {
            // Requires explicit allow_ddl flag
            if caps.can_ddl() {
                Ok(category)
            } else {
                Err(PlenumError::capability_violation(
                    "DDL operations require --allow-ddl flag",
                ))
            }
        }
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
        return Err(PlenumError::invalid_input(
            "Multi-statement queries are not supported in MVP",
        ));
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
                while let Some(ch) = chars.next() {
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
                while let Some(ch) = chars.next() {
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

/// Categorize SQL query (engine-specific)
///
/// Each engine has slightly different SQL dialects, so categorization
/// is engine-specific.
fn categorize_query(sql: &str, engine: DatabaseType) -> Result<QueryCategory> {
    match engine {
        DatabaseType::Postgres => categorize_postgres(sql),
        DatabaseType::MySQL => categorize_mysql(sql),
        DatabaseType::SQLite => categorize_sqlite(sql),
    }
}

/// Categorize PostgreSQL query
fn categorize_postgres(sql: &str) -> Result<QueryCategory> {
    // Strip EXPLAIN/EXPLAIN ANALYZE prefix
    let sql = strip_explain_prefix(sql);

    // Check for DDL operations
    if is_ddl_postgres(&sql) {
        return Ok(QueryCategory::DDL);
    }

    // Check for write operations
    if is_write_postgres(&sql) {
        return Ok(QueryCategory::Write);
    }

    // Check for read operations
    if is_read_only_postgres(&sql) {
        return Ok(QueryCategory::ReadOnly);
    }

    // Unknown statement type: default to DDL (most restrictive, fail-safe)
    Ok(QueryCategory::DDL)
}

/// Categorize MySQL query
fn categorize_mysql(sql: &str) -> Result<QueryCategory> {
    // Strip EXPLAIN prefix
    let sql = strip_explain_prefix(sql);

    // Check for DDL operations (MySQL has implicit commits for DDL)
    if is_ddl_mysql(&sql) {
        return Ok(QueryCategory::DDL);
    }

    // Check for write operations
    if is_write_mysql(&sql) {
        return Ok(QueryCategory::Write);
    }

    // Check for read operations
    if is_read_only_mysql(&sql) {
        return Ok(QueryCategory::ReadOnly);
    }

    // Unknown statement type: default to DDL (most restrictive, fail-safe)
    Ok(QueryCategory::DDL)
}

/// Categorize SQLite query
fn categorize_sqlite(sql: &str) -> Result<QueryCategory> {
    // Strip EXPLAIN prefix
    let sql = strip_explain_prefix(sql);

    // Check for DDL operations
    if is_ddl_sqlite(&sql) {
        return Ok(QueryCategory::DDL);
    }

    // Check for write operations
    if is_write_sqlite(&sql) {
        return Ok(QueryCategory::Write);
    }

    // Check for read operations
    if is_read_only_sqlite(&sql) {
        return Ok(QueryCategory::ReadOnly);
    }

    // Unknown statement type: default to DDL (most restrictive, fail-safe)
    Ok(QueryCategory::DDL)
}

/// Strip EXPLAIN/EXPLAIN ANALYZE prefix from query
fn strip_explain_prefix(sql: &str) -> String {
    let sql = sql.trim();

    // Handle EXPLAIN ANALYZE
    if sql.starts_with("EXPLAIN ANALYZE") {
        return sql[15..].trim().to_string();
    }

    // Handle EXPLAIN
    if sql.starts_with("EXPLAIN") {
        return sql[7..].trim().to_string();
    }

    sql.to_string()
}

// PostgreSQL categorization helpers

fn is_ddl_postgres(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("CREATE ")
        || sql.starts_with("DROP ")
        || sql.starts_with("ALTER ")
        || sql.starts_with("TRUNCATE ")
        || sql.starts_with("RENAME ")
}

fn is_write_postgres(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("INSERT ")
        || sql.starts_with("UPDATE ")
        || sql.starts_with("DELETE ")
        || sql.starts_with("CALL ")
        || (sql.starts_with("WITH ") && contains_write_cte(sql))
}

fn is_read_only_postgres(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("SELECT ")
        || (sql.starts_with("WITH ") && !contains_write_cte(sql))
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("START TRANSACTION")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE")
}

// MySQL categorization helpers

fn is_ddl_mysql(sql: &str) -> bool {
    let sql = sql.trim();
    // MySQL DDL statements (cause implicit commit)
    sql.starts_with("CREATE ")
        || sql.starts_with("DROP ")
        || sql.starts_with("ALTER ")
        || sql.starts_with("TRUNCATE ")
        || sql.starts_with("RENAME ")
        || sql.starts_with("LOCK TABLES")
        || sql.starts_with("UNLOCK TABLES")
}

fn is_write_mysql(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("INSERT ")
        || sql.starts_with("UPDATE ")
        || sql.starts_with("DELETE ")
        || sql.starts_with("REPLACE ")
        || sql.starts_with("CALL ")
        || sql.starts_with("EXEC ")
        || (sql.starts_with("WITH ") && contains_write_cte(sql))
}

fn is_read_only_mysql(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("SELECT ")
        || (sql.starts_with("WITH ") && !contains_write_cte(sql))
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

// SQLite categorization helpers

fn is_ddl_sqlite(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("CREATE ")
        || sql.starts_with("DROP ")
        || sql.starts_with("ALTER ")
        || sql.starts_with("RENAME ")
        || sql.starts_with("VACUUM")
        || sql.starts_with("REINDEX")
        || sql.starts_with("ATTACH ")
        || sql.starts_with("DETACH ")
}

fn is_write_sqlite(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("INSERT ")
        || sql.starts_with("UPDATE ")
        || sql.starts_with("DELETE ")
        || sql.starts_with("REPLACE ")
        || (sql.starts_with("WITH ") && contains_write_cte(sql))
}

fn is_read_only_sqlite(sql: &str) -> bool {
    let sql = sql.trim();
    sql.starts_with("SELECT ")
        || (sql.starts_with("WITH ") && !contains_write_cte(sql))
        || sql.starts_with("PRAGMA ")
        || sql.starts_with("BEGIN")
        || sql.starts_with("COMMIT")
        || sql.starts_with("ROLLBACK")
        || sql.starts_with("SAVEPOINT")
        || sql.starts_with("RELEASE")
}

// Helper: Check if CTE contains write operations
fn contains_write_cte(sql: &str) -> bool {
    // Simple heuristic: check if CTE contains INSERT/UPDATE/DELETE keywords
    // This is conservative - it may misclassify some queries as write
    sql.contains(" INSERT ") || sql.contains(" UPDATE ") || sql.contains(" DELETE ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Preprocessing tests

    #[test]
    fn test_preprocess_empty_query() {
        let result = preprocess_sql("");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Query cannot be empty"));
    }

    #[test]
    fn test_preprocess_whitespace_trimming() {
        let result = preprocess_sql("  SELECT * FROM users  ").unwrap();
        assert!(result.starts_with("SELECT"));
        assert!(!result.starts_with(' '));
    }

    #[test]
    fn test_preprocess_line_comments() {
        let result = preprocess_sql("SELECT * FROM users -- this is a comment\nWHERE id = 1").unwrap();
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

    // PostgreSQL categorization tests

    #[test]
    fn test_postgres_select() {
        let caps = Capabilities::read_only();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_postgres_insert_without_write_capability() {
        let caps = Capabilities::read_only();
        let result = validate_query("INSERT INTO users (name) VALUES ('test')", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Write operations require --allow-write"));
    }

    #[test]
    fn test_postgres_insert_with_write_capability() {
        let caps = Capabilities::with_write();
        let result = validate_query("INSERT INTO users (name) VALUES ('test')", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::Write);
    }

    #[test]
    fn test_postgres_create_without_ddl_capability() {
        let caps = Capabilities::with_write();
        let result = validate_query("CREATE TABLE test (id INT)", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("DDL operations require --allow-ddl"));
    }

    #[test]
    fn test_postgres_create_with_ddl_capability() {
        let caps = Capabilities::with_ddl();
        let result = validate_query("CREATE TABLE test (id INT)", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::DDL);
    }

    #[test]
    fn test_postgres_explain_select() {
        let caps = Capabilities::read_only();
        let result = validate_query("EXPLAIN SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_postgres_explain_analyze() {
        let caps = Capabilities::read_only();
        let result = validate_query("EXPLAIN ANALYZE SELECT * FROM users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_postgres_transaction_control() {
        let caps = Capabilities::read_only();
        assert!(validate_query("BEGIN", &caps, DatabaseType::Postgres).is_ok());
        assert!(validate_query("COMMIT", &caps, DatabaseType::Postgres).is_ok());
        assert!(validate_query("ROLLBACK", &caps, DatabaseType::Postgres).is_ok());
    }

    #[test]
    fn test_postgres_cte_read_only() {
        let caps = Capabilities::read_only();
        let result = validate_query("WITH cte AS (SELECT * FROM users) SELECT * FROM cte", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    // MySQL categorization tests

    #[test]
    fn test_mysql_select() {
        let caps = Capabilities::read_only();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::MySQL);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_mysql_replace() {
        let caps = Capabilities::with_write();
        let result = validate_query("REPLACE INTO users (id, name) VALUES (1, 'test')", &caps, DatabaseType::MySQL);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::Write);
    }

    #[test]
    fn test_mysql_lock_tables_requires_ddl() {
        let caps = Capabilities::with_write();
        let result = validate_query("LOCK TABLES users WRITE", &caps, DatabaseType::MySQL);
        assert!(result.is_err());
    }

    #[test]
    fn test_mysql_show_statement() {
        let caps = Capabilities::read_only();
        let result = validate_query("SHOW TABLES", &caps, DatabaseType::MySQL);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    // SQLite categorization tests

    #[test]
    fn test_sqlite_select() {
        let caps = Capabilities::read_only();
        let result = validate_query("SELECT * FROM users", &caps, DatabaseType::SQLite);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_sqlite_pragma() {
        let caps = Capabilities::read_only();
        let result = validate_query("PRAGMA table_info(users)", &caps, DatabaseType::SQLite);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_sqlite_vacuum_requires_ddl() {
        let caps = Capabilities::with_write();
        let result = validate_query("VACUUM", &caps, DatabaseType::SQLite);
        assert!(result.is_err());
    }

    // Capability hierarchy tests

    #[test]
    fn test_ddl_implies_write() {
        let caps = Capabilities::with_ddl();
        // DDL capability should allow write operations
        let result = validate_query("INSERT INTO users (name) VALUES ('test')", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_does_not_imply_ddl() {
        let caps = Capabilities::with_write();
        // Write capability should NOT allow DDL operations
        let result = validate_query("CREATE TABLE test (id INT)", &caps, DatabaseType::Postgres);
        assert!(result.is_err());
    }

    // Edge case tests

    #[test]
    fn test_case_insensitivity() {
        let caps = Capabilities::read_only();
        let result = validate_query("select * from users", &caps, DatabaseType::Postgres);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), QueryCategory::ReadOnly);
    }

    #[test]
    fn test_mixed_case_with_comments() {
        let caps = Capabilities::read_only();
        let result = validate_query(
            "-- Query users\nSeLeCt * FrOm UsErS -- get all",
            &caps,
            DatabaseType::Postgres
        );
        assert!(result.is_ok());
    }
}
