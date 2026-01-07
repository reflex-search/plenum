//! JSON Output Envelope Types
//!
//! This module defines the structured JSON output format for all Plenum operations.
//! All operations return either a SuccessEnvelope or an ErrorEnvelope.
//!
//! # Output Contract
//! - Success: `{"ok": true, "engine": "...", "command": "...", "data": {...}, "meta": {...}}`
//! - Error: `{"ok": false, "engine": "...", "command": "...", "error": {"code": "...", "message": "..."}}`
//!
//! Output is stable, versioned, and suitable for programmatic parsing by agents.

use serde::{Deserialize, Serialize};

use crate::error::PlenumError;

/// Success envelope for operation results
///
/// Generic over the data type to support different operation return values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessEnvelope<T> {
    /// Always true for success envelopes
    pub ok: bool,

    /// Database engine used for this operation (postgres, mysql, sqlite)
    pub engine: String,

    /// Command that was executed (connect, introspect, query)
    pub command: String,

    /// Operation-specific data
    pub data: T,

    /// Execution metadata
    pub meta: Metadata,
}

impl<T> SuccessEnvelope<T> {
    /// Create a new success envelope
    pub fn new(engine: impl Into<String>, command: impl Into<String>, data: T, meta: Metadata) -> Self {
        Self {
            ok: true,
            engine: engine.into(),
            command: command.into(),
            data,
            meta,
        }
    }
}

/// Error envelope for operation failures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    /// Always false for error envelopes
    pub ok: bool,

    /// Database engine (if applicable, empty string if not engine-specific)
    pub engine: String,

    /// Command that was attempted (connect, introspect, query)
    pub command: String,

    /// Error information
    pub error: ErrorInfo,
}

impl ErrorEnvelope {
    /// Create a new error envelope
    pub fn new(
        engine: impl Into<String>,
        command: impl Into<String>,
        error: ErrorInfo,
    ) -> Self {
        Self {
            ok: false,
            engine: engine.into(),
            command: command.into(),
            error,
        }
    }

    /// Create error envelope from PlenumError
    pub fn from_error(
        engine: impl Into<String>,
        command: impl Into<String>,
        err: &PlenumError,
    ) -> Self {
        Self::new(
            engine,
            command,
            ErrorInfo {
                code: err.error_code().to_string(),
                message: err.message(),
            },
        )
    }
}

/// Error information structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// Stable error code (e.g., "CAPABILITY_VIOLATION", "CONNECTION_FAILED")
    pub code: String,

    /// Human-readable error message (agent-appropriate, no sensitive data)
    pub message: String,
}

impl ErrorInfo {
    /// Create a new error info
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Execution metadata included in all success responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Execution time in milliseconds
    pub execution_ms: u64,

    /// Number of rows returned (for query results, None for other operations)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_returned: Option<usize>,
}

impl Metadata {
    /// Create new metadata with just execution time
    pub fn new(execution_ms: u64) -> Self {
        Self {
            execution_ms,
            rows_returned: None,
        }
    }

    /// Create new metadata with execution time and row count
    pub fn with_rows(execution_ms: u64, rows_returned: usize) -> Self {
        Self {
            execution_ms,
            rows_returned: Some(rows_returned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_success_envelope_serialization() {
        let envelope = SuccessEnvelope::new(
            "postgres",
            "query",
            serde_json::json!({"result": "test"}),
            Metadata::with_rows(42, 10),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains(r#""ok":true"#));
        assert!(json.contains(r#""engine":"postgres"#));
        assert!(json.contains(r#""command":"query"#));
        assert!(json.contains(r#""execution_ms":42"#));
        assert!(json.contains(r#""rows_returned":10"#));
    }

    #[test]
    fn test_error_envelope_serialization() {
        let envelope = ErrorEnvelope::new(
            "mysql",
            "connect",
            ErrorInfo::new("CONNECTION_FAILED", "Could not connect to database"),
        );

        let json = serde_json::to_string(&envelope).unwrap();
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains(r#""engine":"mysql"#));
        assert!(json.contains(r#""command":"connect"#));
        assert!(json.contains(r#""code":"CONNECTION_FAILED"#));
        assert!(json.contains(r#""message":"Could not connect to database"#));
    }

    #[test]
    fn test_error_envelope_from_plenum_error() {
        let err = PlenumError::capability_violation("DDL operations not allowed");
        let envelope = ErrorEnvelope::from_error("sqlite", "query", &err);

        assert!(!envelope.ok);
        assert_eq!(envelope.engine, "sqlite");
        assert_eq!(envelope.command, "query");
        assert_eq!(envelope.error.code, "CAPABILITY_VIOLATION");
        assert!(envelope.error.message.contains("DDL operations not allowed"));
    }

    #[test]
    fn test_metadata_without_rows() {
        let meta = Metadata::new(100);
        let json = serde_json::to_string(&meta).unwrap();

        assert!(json.contains(r#""execution_ms":100"#));
        // rows_returned should be omitted when None
        assert!(!json.contains("rows_returned"));
    }

    #[test]
    fn test_metadata_with_rows() {
        let meta = Metadata::with_rows(100, 50);
        let json = serde_json::to_string(&meta).unwrap();

        assert!(json.contains(r#""execution_ms":100"#));
        assert!(json.contains(r#""rows_returned":50"#));
    }

    #[test]
    fn test_success_envelope_ok_always_true() {
        let envelope = SuccessEnvelope::new(
            "postgres",
            "introspect",
            serde_json::json!({}),
            Metadata::new(10),
        );
        assert!(envelope.ok);
    }

    #[test]
    fn test_error_envelope_ok_always_false() {
        let envelope = ErrorEnvelope::new(
            "mysql",
            "query",
            ErrorInfo::new("QUERY_FAILED", "Syntax error"),
        );
        assert!(!envelope.ok);
    }
}
