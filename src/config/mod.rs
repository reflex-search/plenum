//! Configuration Management
//!
//! This module handles loading and saving database connection configurations.
//!
//! # Configuration Locations
//! - Local: `.plenum/config.json` (team-shareable, per-project)
//! - Global: `~/.config/plenum/connections.json` (per-user)
//!
//! # Resolution Precedence
//! 1. Explicit connection parameters (highest priority)
//! 2. Local config file (`.plenum/config.json`)
//! 3. Global config file (`~/.config/plenum/connections.json`)
//!
//! # Named Connections
//! Connections are stored as named profiles (e.g., "local", "dev", "prod").
//! Agents can reference connections by name.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::ConnectionConfig;
use crate::error::{PlenumError, Result};

/// Connection registry (stored in config files)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionRegistry {
    /// Named connection profiles
    pub connections: HashMap<String, StoredConnection>,

    /// Default connection name (used when no --name is specified)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Stored connection configuration
///
/// Similar to ConnectionConfig but supports environment variable references
/// for sensitive fields like passwords.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    /// Connection configuration
    #[serde(flatten)]
    pub config: ConnectionConfig,

    /// Environment variable name for password (if not storing password directly)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
}

impl StoredConnection {
    /// Resolve environment variables and return a ConnectionConfig
    pub fn resolve(&self) -> Result<ConnectionConfig> {
        let mut config = self.config.clone();

        // If password_env is set, resolve the environment variable
        if let Some(env_var) = &self.password_env {
            match std::env::var(env_var) {
                Ok(password) => config.password = Some(password),
                Err(_) => {
                    return Err(PlenumError::config_error(format!(
                        "Environment variable {} not found for password",
                        env_var
                    )));
                }
            }
        }

        Ok(config)
    }
}

/// Configuration file location
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigLocation {
    /// Local config: `.plenum/config.json` (team-shareable)
    Local,
    /// Global config: `~/.config/plenum/connections.json` (per-user)
    Global,
}

/// Get path to local config file (`.plenum/config.json`)
pub fn local_config_path() -> Result<PathBuf> {
    let current_dir = std::env::current_dir()
        .map_err(|e| PlenumError::config_error(format!("Could not determine current directory: {}", e)))?;

    Ok(current_dir.join(".plenum").join("config.json"))
}

/// Get path to global config file (`~/.config/plenum/connections.json`)
pub fn global_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| PlenumError::config_error("Could not determine user config directory"))?;

    Ok(config_dir.join("plenum").join("connections.json"))
}

/// Load connection registry from a config file
pub fn load_registry(path: &Path) -> Result<ConnectionRegistry> {
    if !path.exists() {
        // File doesn't exist, return empty registry
        return Ok(ConnectionRegistry::default());
    }

    let contents = fs::read_to_string(path)
        .map_err(|e| PlenumError::config_error(format!("Could not read config file: {}", e)))?;

    let registry: ConnectionRegistry = serde_json::from_str(&contents)
        .map_err(|e| PlenumError::config_error(format!("Invalid config file format: {}", e)))?;

    Ok(registry)
}

/// Save connection registry to a config file
pub fn save_registry(path: &Path, registry: &ConnectionRegistry) -> Result<()> {
    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| PlenumError::config_error(format!("Could not create config directory: {}", e)))?;
    }

    let contents = serde_json::to_string_pretty(registry)
        .map_err(|e| PlenumError::config_error(format!("Could not serialize config: {}", e)))?;

    fs::write(path, contents)
        .map_err(|e| PlenumError::config_error(format!("Could not write config file: {}", e)))?;

    Ok(())
}

/// Load connection registry with precedence (local first, then global)
pub fn load_with_precedence() -> Result<ConnectionRegistry> {
    // Try local config first
    let local_path = local_config_path()?;
    if local_path.exists() {
        return load_registry(&local_path);
    }

    // Fall back to global config
    let global_path = global_config_path()?;
    if global_path.exists() {
        return load_registry(&global_path);
    }

    // No config found, return empty registry
    Ok(ConnectionRegistry::default())
}

/// Resolve a connection by name
///
/// If `name` is None, uses the default connection from the registry.
/// Returns an error if the connection is not found.
pub fn resolve_connection(name: Option<&str>) -> Result<ConnectionConfig> {
    let registry = load_with_precedence()?;

    let connection_name = match name {
        Some(n) => n,
        None => {
            // Use default connection
            match registry.default.as_deref() {
                Some(default) => default,
                None => {
                    return Err(PlenumError::config_error(
                        "No connection name specified and no default connection set",
                    ));
                }
            }
        }
    };

    // Look up connection by name
    let stored = registry
        .connections
        .get(connection_name)
        .ok_or_else(|| PlenumError::config_error(format!("Connection '{}' not found", connection_name)))?;

    // Resolve environment variables
    stored.resolve()
}

/// Save a connection to a config file
pub fn save_connection(
    name: String,
    config: ConnectionConfig,
    location: ConfigLocation,
    set_as_default: bool,
) -> Result<()> {
    // Get config path
    let path = match location {
        ConfigLocation::Local => local_config_path()?,
        ConfigLocation::Global => global_config_path()?,
    };

    // Load existing registry (or create empty one)
    let mut registry = if path.exists() {
        load_registry(&path)?
    } else {
        ConnectionRegistry::default()
    };

    // Add or update connection
    registry.connections.insert(
        name.clone(),
        StoredConnection {
            config,
            password_env: None,
        },
    );

    // Set as default if requested or if this is the first connection
    if set_as_default || registry.default.is_none() {
        registry.default = Some(name);
    }

    // Save registry
    save_registry(&path, &registry)?;

    Ok(())
}

/// List all available connections
pub fn list_connections() -> Result<Vec<(String, ConnectionConfig)>> {
    let registry = load_with_precedence()?;

    let mut connections = Vec::new();
    for (name, stored) in registry.connections {
        match stored.resolve() {
            Ok(config) => connections.push((name, config)),
            Err(e) => {
                // Skip connections that fail to resolve (e.g., missing env vars)
                eprintln!("Warning: Could not resolve connection '{}': {}", name, e.message());
            }
        }
    }

    Ok(connections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DatabaseType;

    #[test]
    fn test_connection_registry_serialization() {
        let mut registry = ConnectionRegistry::default();
        registry.connections.insert(
            "test".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "localhost".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );
        registry.default = Some("test".to_string());

        let json = serde_json::to_string_pretty(&registry).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("localhost"));
    }

    #[test]
    fn test_stored_connection_resolve_direct_password() {
        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "pass".to_string(),
                "db".to_string(),
            ),
            password_env: None,
        };

        let resolved = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("pass".to_string()));
    }

    #[test]
    fn test_stored_connection_resolve_env_var() {
        std::env::set_var("TEST_PASSWORD", "secret");

        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
            },
            password_env: Some("TEST_PASSWORD".to_string()),
        };

        let resolved = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("secret".to_string()));

        std::env::remove_var("TEST_PASSWORD");
    }

    #[test]
    fn test_stored_connection_resolve_missing_env_var() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
            },
            password_env: Some("NONEXISTENT_VAR".to_string()),
        };

        let result = stored.resolve();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message()
            .contains("Environment variable NONEXISTENT_VAR not found"));
    }

    #[test]
    fn test_empty_registry() {
        let registry = ConnectionRegistry::default();
        assert!(registry.connections.is_empty());
        assert!(registry.default.is_none());
    }
}
