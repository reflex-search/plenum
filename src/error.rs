//! Error Handling Infrastructure
//!
//! This module defines all error types used throughout Plenum.
//! All errors are structured and map to specific error codes for JSON output.
//!
//! # Error Categories
//! - `CapabilityViolation`: Operations blocked by capability constraints
//! - `ConnectionFailed`: Database connection errors
//! - `QueryFailed`: Query execution errors
//! - `InvalidInput`: Malformed input or missing required parameters
//! - `EngineError`: Engine-specific database errors
//! - `ConfigError`: Configuration file or connection registry errors

use thiserror::Error;

/// Main error type for Plenum operations
#[derive(Error, Debug)]
pub enum PlenumError {
    /// Operation blocked by capability constraints
    #[error("Capability violation: {0}")]
    CapabilityViolation(String),

    /// Database connection failed
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// Query execution failed
    #[error("Query execution failed: {0}")]
    QueryFailed(String),

    /// Invalid input or missing required parameters
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Engine-specific database error
    #[error("Engine error ({engine}): {detail}")]
    EngineError { engine: String, detail: String },

    /// Configuration error (file not found, invalid JSON, etc.)
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

impl PlenumError {
    /// Convert error to error code string for JSON output
    ///
    /// Error codes are stable and suitable for programmatic handling by agents.
    #[must_use]
    pub const fn error_code(&self) -> &'static str {
        match self {
            Self::CapabilityViolation(_) => "CAPABILITY_VIOLATION",
            Self::ConnectionFailed(_) => "CONNECTION_FAILED",
            Self::QueryFailed(_) => "QUERY_FAILED",
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::EngineError { .. } => "ENGINE_ERROR",
            Self::ConfigError(_) => "CONFIG_ERROR",
        }
    }

    /// Get human-readable error message (agent-appropriate, no sensitive data)
    ///
    /// This message is safe to include in JSON output.
    /// It does not contain credentials, file paths, or other sensitive information.
    #[must_use]
    pub fn message(&self) -> String {
        // Use Display implementation from thiserror
        self.to_string()
    }

    /// Create a capability violation error
    pub fn capability_violation(message: impl Into<String>) -> Self {
        Self::CapabilityViolation(message.into())
    }

    /// Create a connection failed error
    pub fn connection_failed(message: impl Into<String>) -> Self {
        Self::ConnectionFailed(message.into())
    }

    /// Create a query failed error
    pub fn query_failed(message: impl Into<String>) -> Self {
        Self::QueryFailed(message.into())
    }

    /// Create an invalid input error
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::InvalidInput(message.into())
    }

    /// Create an engine-specific error
    pub fn engine_error(engine: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::EngineError { engine: engine.into(), detail: detail.into() }
    }

    /// Create a configuration error
    pub fn config_error(message: impl Into<String>) -> Self {
        Self::ConfigError(message.into())
    }
}

/// Result type alias for Plenum operations
pub type Result<T> = std::result::Result<T, PlenumError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(PlenumError::capability_violation("test").error_code(), "CAPABILITY_VIOLATION");
        assert_eq!(PlenumError::connection_failed("test").error_code(), "CONNECTION_FAILED");
        assert_eq!(PlenumError::query_failed("test").error_code(), "QUERY_FAILED");
        assert_eq!(PlenumError::invalid_input("test").error_code(), "INVALID_INPUT");
        assert_eq!(PlenumError::engine_error("mysql", "test").error_code(), "ENGINE_ERROR");
        assert_eq!(PlenumError::config_error("test").error_code(), "CONFIG_ERROR");
    }

    #[test]
    fn test_error_messages() {
        let err = PlenumError::capability_violation("DDL not allowed");
        assert!(err.message().contains("DDL not allowed"));

        let err = PlenumError::engine_error("postgres", "connection timeout");
        assert!(err.message().contains("postgres"));
        assert!(err.message().contains("connection timeout"));
    }

    #[test]
    fn test_error_constructors() {
        let err = PlenumError::capability_violation("test");
        assert!(matches!(err, PlenumError::CapabilityViolation(_)));

        let err = PlenumError::connection_failed("test");
        assert!(matches!(err, PlenumError::ConnectionFailed(_)));

        let err = PlenumError::query_failed("test");
        assert!(matches!(err, PlenumError::QueryFailed(_)));

        let err = PlenumError::invalid_input("test");
        assert!(matches!(err, PlenumError::InvalidInput(_)));

        let err = PlenumError::engine_error("mysql", "test");
        assert!(matches!(err, PlenumError::EngineError { .. }));

        let err = PlenumError::config_error("test");
        assert!(matches!(err, PlenumError::ConfigError(_)));
    }
}
