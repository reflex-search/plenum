//! DSN/URL connection string parsing for one-off connections
//!
//! Parses per-engine connection URLs into `ConnectionConfig`.
//! No universal parser — each engine has distinct parsing logic.
//! Engine is inferred from the URL scheme; invalid/ambiguous schemes fail fast.
//! Credentials are never echoed in error output.

use std::path::PathBuf;

use crate::engine::ConnectionConfig;
use crate::error::{PlenumError, Result};

/// Parse a DSN/URL into a `ConnectionConfig`.
///
/// Supported schemes:
/// - `postgres://…` or `postgresql://…` → `PostgreSQL`
/// - `mysql://…` → `MySQL`
/// - `sqlite:…` (various path forms) → `SQLite`
/// - `duckdb:…` (various path forms) → `DuckDB`
///
/// Engine is inferred from the scheme. Error messages never echo credentials.
/// Use [`redact_dsn`] when including the original DSN string in any output.
pub fn parse_dsn(dsn: &str) -> Result<ConnectionConfig> {
    if dsn.starts_with("postgres://") || dsn.starts_with("postgresql://") {
        parse_postgres_dsn(dsn)
    } else if dsn.starts_with("mysql://") {
        parse_mysql_dsn(dsn)
    } else if dsn.starts_with("sqlite:") {
        parse_sqlite_dsn(dsn)
    } else if dsn.starts_with("duckdb:") {
        parse_duckdb_dsn(dsn)
    } else {
        Err(PlenumError::invalid_input(
            "Unrecognized DSN scheme. Use postgres://, postgresql://, mysql://, sqlite:, or duckdb: prefix",
        ))
    }
}

/// Redact the password component from a DSN for safe error/diagnostic output.
///
/// Replaces the password (between `:` and `@` in the authority section)
/// with `****`. Returns the original string unchanged when no credentials
/// are found (e.g. `SQLite` paths) or when there is no password component.
/// Never panics.
#[must_use]
pub fn redact_dsn(dsn: &str) -> String {
    let Some(scheme_end) = dsn.find("://") else {
        return dsn.to_string(); // single-colon form (sqlite:) — no credentials
    };
    let after_scheme = &dsn[scheme_end + 3..];

    // Last "@" separates credentials from host
    let Some(at_pos) = after_scheme.rfind('@') else {
        return dsn.to_string(); // no credentials
    };

    let userinfo = &after_scheme[..at_pos];
    let after_at = &after_scheme[at_pos..]; // includes the "@"
    let scheme_prefix = &dsn[..scheme_end + 3];

    if let Some(colon_pos) = userinfo.find(':') {
        let user = &userinfo[..colon_pos];
        format!("{scheme_prefix}{user}:****{after_at}")
    } else {
        // Username only — nothing to redact
        dsn.to_string()
    }
}

// ============================================================================
// Per-engine parsers
// ============================================================================

fn parse_postgres_dsn(dsn: &str) -> Result<ConnectionConfig> {
    let rest = dsn
        .strip_prefix("postgresql://")
        .or_else(|| dsn.strip_prefix("postgres://"))
        .expect("caller verified scheme");

    let (user, password, host, port, database) = parse_network_authority(rest, 5432)
        .map_err(|e| PlenumError::invalid_input(format!("Invalid PostgreSQL DSN: {e}")))?;

    Ok(ConnectionConfig::postgres(host, port, user, password, database))
}

fn parse_mysql_dsn(dsn: &str) -> Result<ConnectionConfig> {
    let rest = dsn.strip_prefix("mysql://").expect("caller verified scheme");

    let (user, password, host, port, database) = parse_network_authority(rest, 3306)
        .map_err(|e| PlenumError::invalid_input(format!("Invalid MySQL DSN: {e}")))?;

    Ok(ConnectionConfig::mysql(host, port, user, password, database))
}

fn parse_sqlite_dsn(dsn: &str) -> Result<ConnectionConfig> {
    // Supported forms (pragmatic conventions shared by most drivers):
    //   sqlite:///absolute/path.db  →  /absolute/path.db   (RFC: empty authority + absolute path)
    //   sqlite://relative/path.db   →  relative/path.db    (double-slash, path treated as-is)
    //   sqlite:/absolute/path.db    →  /absolute/path.db   (single-slash: absolute)
    //   sqlite:relative/path.db     →  relative/path.db    (colon only: relative)
    //   sqlite::memory:             →  :memory:            (SQLite in-memory database)

    let path_str: String = if let Some(p) = dsn.strip_prefix("sqlite:///") {
        // Triple-slash: RFC-standard empty authority + absolute path
        format!("/{p}")
    } else if let Some(p) = dsn.strip_prefix("sqlite://") {
        // Double-slash with non-empty authority: treat entire remainder as path
        p.to_string()
    } else if let Some(p) = dsn.strip_prefix("sqlite:/") {
        // Single-slash: absolute path
        format!("/{p}")
    } else {
        // Plain `sqlite:` prefix: relative path or special value (e.g. `:memory:`)
        dsn.strip_prefix("sqlite:").expect("caller verified prefix").to_string()
    };

    if path_str.is_empty() {
        return Err(PlenumError::invalid_input(
            "SQLite DSN missing file path (e.g. sqlite:///path/to/db.sqlite or sqlite::memory:)",
        ));
    }

    Ok(ConnectionConfig::sqlite(PathBuf::from(path_str)))
}

fn parse_duckdb_dsn(dsn: &str) -> Result<ConnectionConfig> {
    // Same pragmatic path forms as the sqlite: scheme:
    //   duckdb:///absolute/path.duckdb  →  /absolute/path.duckdb
    //   duckdb://relative/path.duckdb   →  relative/path.duckdb
    //   duckdb:/absolute/path.duckdb    →  /absolute/path.duckdb
    //   duckdb:relative/path.duckdb     →  relative/path.duckdb
    //   duckdb::memory:                 →  :memory:  (DuckDB in-memory database)

    let path_str: String = if let Some(p) = dsn.strip_prefix("duckdb:///") {
        format!("/{p}")
    } else if let Some(p) = dsn.strip_prefix("duckdb://") {
        p.to_string()
    } else if let Some(p) = dsn.strip_prefix("duckdb:/") {
        format!("/{p}")
    } else {
        dsn.strip_prefix("duckdb:").expect("caller verified prefix").to_string()
    };

    if path_str.is_empty() {
        return Err(PlenumError::invalid_input(
            "DuckDB DSN missing file path (e.g. duckdb:///path/to/db.duckdb or duckdb::memory:)",
        ));
    }

    Ok(ConnectionConfig::duckdb(PathBuf::from(path_str)))
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Parse `user:password@host:port/database[?query]` into its components.
///
/// - Password may be absent (returns empty string).
/// - Port defaults to `default_port` when not present.
/// - Query string is silently stripped.
/// - Returns `Err(description)` with no credential content.
fn parse_network_authority(
    rest: &str,
    default_port: u16,
) -> std::result::Result<(String, String, String, u16, String), String> {
    // Strip query string
    let rest = rest.split('?').next().unwrap_or(rest);

    // Split on the last "@" to separate userinfo from host
    let (userinfo, hostinfo) = if let Some(at_pos) = rest.rfind('@') {
        (&rest[..at_pos], &rest[at_pos + 1..])
    } else {
        return Err(
            "missing credentials — expected user:password@host:port/database format".to_string()
        );
    };

    // Parse userinfo: "user:password" or "user"
    let (user, password) = if let Some(colon_pos) = userinfo.find(':') {
        (percent_decode(&userinfo[..colon_pos]), percent_decode(&userinfo[colon_pos + 1..]))
    } else {
        (percent_decode(userinfo), String::new())
    };

    if user.is_empty() {
        return Err("missing username".to_string());
    }

    // Split hostinfo on first "/" to get host:port and database
    let (host_and_port, database) = if let Some(slash_pos) = hostinfo.find('/') {
        (&hostinfo[..slash_pos], percent_decode(&hostinfo[slash_pos + 1..]))
    } else {
        return Err("missing database name — add /database to the URL".to_string());
    };

    if database.is_empty() {
        return Err("database name is empty".to_string());
    }

    let (host, port) = parse_host_port(host_and_port, default_port)?;

    Ok((user, password, host, port, database))
}

/// Parse `host:port` or `[ipv6]:port`, returning `(host, port)`.
fn parse_host_port(s: &str, default_port: u16) -> std::result::Result<(String, u16), String> {
    if s.starts_with('[') {
        // IPv6: [::1]:5432
        let close = s.rfind(']').ok_or("malformed IPv6 address (missing closing ']')")?;
        let host = s[1..close].to_string();
        let after_bracket = &s[close + 1..];
        let port = if let Some(port_str) = after_bracket.strip_prefix(':') {
            port_str.parse::<u16>().map_err(|_| format!("invalid port '{port_str}'"))?
        } else {
            default_port
        };
        Ok((host, port))
    } else if let Some(colon_pos) = s.rfind(':') {
        let host = &s[..colon_pos];
        let port_str = &s[colon_pos + 1..];
        let port = port_str.parse::<u16>().map_err(|_| format!("invalid port '{port_str}'"))?;
        Ok((host.to_string(), port))
    } else {
        if s.is_empty() {
            return Err("missing host".to_string());
        }
        Ok((s.to_string(), default_port))
    }
}

/// Minimal percent-decoding for %XX sequences in URL components.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                result.push(char::from(hi * 16 + lo));
                i += 3;
                continue;
            }
        }
        result.push(char::from(bytes[i]));
        i += 1;
    }
    result
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DatabaseType;

    // ─── PostgreSQL ────────────────────────────────────────────────────────────

    #[test]
    fn postgres_full_url() {
        let cfg = parse_dsn("postgres://alice:secret@db.example.com:5432/mydb").unwrap();
        assert_eq!(cfg.engine, DatabaseType::Postgres);
        assert_eq!(cfg.host.as_deref(), Some("db.example.com"));
        assert_eq!(cfg.port, Some(5432));
        assert_eq!(cfg.user.as_deref(), Some("alice"));
        assert_eq!(cfg.password.as_deref(), Some("secret"));
        assert_eq!(cfg.database.as_deref(), Some("mydb"));
        assert!(cfg.file.is_none());
    }

    #[test]
    fn postgresql_scheme_alias() {
        let cfg = parse_dsn("postgresql://alice:secret@localhost:5432/mydb").unwrap();
        assert_eq!(cfg.engine, DatabaseType::Postgres);
        assert_eq!(cfg.host.as_deref(), Some("localhost"));
    }

    #[test]
    fn postgres_default_port() {
        let cfg = parse_dsn("postgres://alice:secret@localhost/mydb").unwrap();
        assert_eq!(cfg.port, Some(5432));
    }

    #[test]
    fn postgres_query_string_stripped() {
        let cfg = parse_dsn("postgres://alice:secret@localhost:5432/mydb?sslmode=require").unwrap();
        assert_eq!(cfg.database.as_deref(), Some("mydb"));
        assert_eq!(cfg.host.as_deref(), Some("localhost"));
    }

    #[test]
    fn postgres_percent_encoded_password() {
        let cfg = parse_dsn("postgres://alice:p%40ssword@localhost/db").unwrap();
        assert_eq!(cfg.password.as_deref(), Some("p@ssword"));
    }

    #[test]
    fn postgres_empty_password_allowed() {
        let cfg = parse_dsn("postgres://alice:@localhost/db").unwrap();
        assert_eq!(cfg.password.as_deref(), Some(""));
    }

    #[test]
    fn postgres_missing_database_fails() {
        let err = parse_dsn("postgres://alice:secret@localhost:5432").unwrap_err();
        assert!(err.message().contains("Invalid PostgreSQL DSN"), "got: {}", err.message());
    }

    #[test]
    fn postgres_missing_credentials_fails() {
        let err = parse_dsn("postgres://localhost:5432/mydb").unwrap_err();
        assert!(err.message().contains("Invalid PostgreSQL DSN"));
    }

    #[test]
    fn postgres_invalid_port_fails() {
        let err = parse_dsn("postgres://alice:secret@localhost:notaport/mydb").unwrap_err();
        assert!(err.message().contains("Invalid PostgreSQL DSN"));
    }

    #[test]
    fn postgres_empty_database_fails() {
        let err = parse_dsn("postgres://alice:secret@localhost:5432/").unwrap_err();
        assert!(err.message().contains("Invalid PostgreSQL DSN"));
    }

    // ─── MySQL ─────────────────────────────────────────────────────────────────

    #[test]
    fn mysql_full_url() {
        let cfg = parse_dsn("mysql://root:hunter2@127.0.0.1:3306/appdb").unwrap();
        assert_eq!(cfg.engine, DatabaseType::MySQL);
        assert_eq!(cfg.host.as_deref(), Some("127.0.0.1"));
        assert_eq!(cfg.port, Some(3306));
        assert_eq!(cfg.user.as_deref(), Some("root"));
        assert_eq!(cfg.password.as_deref(), Some("hunter2"));
        assert_eq!(cfg.database.as_deref(), Some("appdb"));
    }

    #[test]
    fn mysql_default_port() {
        let cfg = parse_dsn("mysql://root:hunter2@localhost/appdb").unwrap();
        assert_eq!(cfg.port, Some(3306));
    }

    #[test]
    fn mysql_missing_database_fails() {
        let err = parse_dsn("mysql://root:hunter2@localhost").unwrap_err();
        assert!(err.message().contains("Invalid MySQL DSN"));
    }

    // ─── SQLite ────────────────────────────────────────────────────────────────

    #[test]
    fn sqlite_triple_slash_absolute() {
        let cfg = parse_dsn("sqlite:///tmp/test.db").unwrap();
        assert_eq!(cfg.engine, DatabaseType::SQLite);
        assert_eq!(cfg.file, Some(PathBuf::from("/tmp/test.db")));
    }

    #[test]
    fn sqlite_single_slash_absolute() {
        let cfg = parse_dsn("sqlite:/tmp/test.db").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("/tmp/test.db")));
    }

    #[test]
    fn sqlite_relative_path() {
        let cfg = parse_dsn("sqlite:relative/path.db").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("relative/path.db")));
    }

    #[test]
    fn sqlite_double_slash_path() {
        let cfg = parse_dsn("sqlite://./local.db").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("./local.db")));
    }

    #[test]
    fn sqlite_memory() {
        let cfg = parse_dsn("sqlite::memory:").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from(":memory:")));
    }

    #[test]
    fn sqlite_empty_path_fails() {
        let err = parse_dsn("sqlite:").unwrap_err();
        assert!(err.message().contains("missing file path"), "got: {}", err.message());
    }

    // ─── DuckDB ────────────────────────────────────────────────────────────────

    #[test]
    fn duckdb_triple_slash_absolute() {
        let cfg = parse_dsn("duckdb:///tmp/test.duckdb").unwrap();
        assert_eq!(cfg.engine, DatabaseType::DuckDB);
        assert_eq!(cfg.file, Some(PathBuf::from("/tmp/test.duckdb")));
    }

    #[test]
    fn duckdb_single_slash_absolute() {
        let cfg = parse_dsn("duckdb:/tmp/test.duckdb").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("/tmp/test.duckdb")));
    }

    #[test]
    fn duckdb_relative_path() {
        let cfg = parse_dsn("duckdb:relative/path.duckdb").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("relative/path.duckdb")));
    }

    #[test]
    fn duckdb_double_slash_path() {
        let cfg = parse_dsn("duckdb://./local.duckdb").unwrap();
        assert_eq!(cfg.file, Some(PathBuf::from("./local.duckdb")));
    }

    #[test]
    fn duckdb_memory() {
        let cfg = parse_dsn("duckdb::memory:").unwrap();
        assert_eq!(cfg.engine, DatabaseType::DuckDB);
        assert_eq!(cfg.file, Some(PathBuf::from(":memory:")));
    }

    #[test]
    fn duckdb_empty_path_fails() {
        let err = parse_dsn("duckdb:").unwrap_err();
        assert!(err.message().contains("missing file path"), "got: {}", err.message());
    }

    #[test]
    fn redact_duckdb_no_op() {
        let dsn = "duckdb:///tmp/test.duckdb";
        assert_eq!(redact_dsn(dsn), dsn);
    }

    // ─── Redaction ─────────────────────────────────────────────────────────────

    #[test]
    fn redact_postgres_password() {
        let redacted = redact_dsn("postgres://alice:mysecret@localhost:5432/db");
        assert_eq!(redacted, "postgres://alice:****@localhost:5432/db");
        assert!(!redacted.contains("mysecret"));
    }

    #[test]
    fn redact_mysql_password() {
        let redacted = redact_dsn("mysql://root:hunter2@127.0.0.1:3306/appdb");
        assert_eq!(redacted, "mysql://root:****@127.0.0.1:3306/appdb");
    }

    #[test]
    fn redact_no_password_unchanged() {
        let dsn = "postgres://alice@localhost/db";
        assert_eq!(redact_dsn(dsn), dsn);
    }

    #[test]
    fn redact_sqlite_no_op() {
        let dsn = "sqlite:///tmp/test.db";
        assert_eq!(redact_dsn(dsn), dsn);
    }

    #[test]
    fn redact_percent_encoded_password() {
        let redacted = redact_dsn("postgres://alice:p%40ssw0rd@localhost/db");
        assert_eq!(redacted, "postgres://alice:****@localhost/db");
    }

    // ─── Scheme errors ─────────────────────────────────────────────────────────

    #[test]
    fn unknown_scheme_fails() {
        let err = parse_dsn("mongodb://user:pass@localhost/db").unwrap_err();
        assert!(err.message().contains("Unrecognized DSN scheme"));
    }

    #[test]
    fn empty_dsn_fails() {
        let err = parse_dsn("").unwrap_err();
        assert!(err.message().contains("Unrecognized DSN scheme"));
    }

    #[test]
    fn no_scheme_fails() {
        let err = parse_dsn("localhost:5432/mydb").unwrap_err();
        assert!(err.message().contains("Unrecognized DSN scheme"));
    }
}
