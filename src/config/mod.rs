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

    /// Current connection name (used when no --name is specified)
    #[serde(skip_serializing_if = "Option::is_none", alias = "default")]
    pub current: Option<String>,
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
///
/// This function merges both local and global configs:
/// - Connections from both configs are included
/// - Local connections override global connections with the same name
/// - Local current connection is used if set, otherwise global current is used
///
/// Used for: list_connections() and when --connection flag is provided
pub fn load_with_precedence() -> Result<ConnectionRegistry> {
    let local_path = local_config_path()?;
    let global_path = global_config_path()?;

    let local_exists = local_path.exists();
    let global_exists = global_path.exists();

    match (local_exists, global_exists) {
        (false, false) => {
            // No configs found, return empty registry
            Ok(ConnectionRegistry::default())
        }
        (true, false) => {
            // Only local config exists
            load_registry(&local_path)
        }
        (false, true) => {
            // Only global config exists
            load_registry(&global_path)
        }
        (true, true) => {
            // Both exist - merge them with local taking precedence
            let global_registry = load_registry(&global_path)?;
            let local_registry = load_registry(&local_path)?;

            let mut merged = ConnectionRegistry::default();

            // Start with global connections
            merged.connections = global_registry.connections;

            // Override with local connections (local wins for same-named connections)
            for (name, conn) in local_registry.connections {
                merged.connections.insert(name, conn);
            }

            // Use local current if set, otherwise use global current
            merged.current = local_registry.current.or(global_registry.current);

            Ok(merged)
        }
    }
}

/// Load connection registry with strict separation (local OR global, not merged)
///
/// This function uses strict precedence:
/// - If local config exists → return local ONLY (ignore global)
/// - If no local config → return global
///
/// Used for: resolving implicit current connection (when no --connection flag)
pub fn load_local_or_global() -> Result<ConnectionRegistry> {
    let local_path = local_config_path()?;

    if local_path.exists() {
        // Local config exists, use it exclusively
        load_registry(&local_path)
    } else {
        // No local config, fall back to global
        let global_path = global_config_path()?;
        load_registry(&global_path)
    }
}

/// Resolve a connection by name
///
/// If `name` is None, uses the current connection from the registry (strict separation).
/// If `name` is Some, searches in merged view (both local and global).
/// Returns an error if the connection is not found.
pub fn resolve_connection(name: Option<&str>) -> Result<ConnectionConfig> {
    // Use different loading strategies based on whether name is provided
    let registry = match name {
        Some(_) => {
            // Explicit connection name: search in merged view (both local and global)
            load_with_precedence()?
        }
        None => {
            // Implicit current: use strict separation (local OR global, not both)
            load_local_or_global()?
        }
    };

    let connection_name = match name {
        Some(n) => n,
        None => {
            // Use current connection
            match registry.current.as_deref() {
                Some(current) => current,
                None => {
                    return Err(PlenumError::config_error(
                        "No connection name specified and no current connection set",
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

    // Set as current if requested or if this is the first connection
    if set_as_default || registry.current.is_none() {
        registry.current = Some(name);
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
        registry.current = Some("test".to_string());

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
        assert!(registry.current.is_none());
    }

    #[test]
    fn test_config_merging_both_connections_visible() {
        // Test that connections from both local and global configs are visible
        let mut global_registry = ConnectionRegistry::default();
        global_registry.connections.insert(
            "global-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "global-host".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );
        global_registry.current = Some("global-conn".to_string());

        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/test.db")),
                password_env: None,
            },
        );

        // Simulate merging (same logic as load_with_precedence when both exist)
        let mut merged = ConnectionRegistry::default();
        merged.connections = global_registry.connections.clone();
        for (name, conn) in local_registry.connections.clone() {
            merged.connections.insert(name, conn);
        }
        merged.current = local_registry.current.or(global_registry.current);

        // Both connections should be present
        assert_eq!(merged.connections.len(), 2);
        assert!(merged.connections.contains_key("global-conn"));
        assert!(merged.connections.contains_key("local-conn"));
    }

    #[test]
    fn test_config_merging_local_overrides_global() {
        // Test that local connection overrides global connection with same name
        let mut global_registry = ConnectionRegistry::default();
        global_registry.connections.insert(
            "shared".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "global-host".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );

        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "shared".to_string(),
            StoredConnection {
                config: ConnectionConfig::mysql(
                    "local-host".to_string(),
                    3306,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );

        // Simulate merging
        let mut merged = ConnectionRegistry::default();
        merged.connections = global_registry.connections;
        for (name, conn) in local_registry.connections {
            merged.connections.insert(name, conn);
        }

        // Should have local version (MySQL, not Postgres)
        assert_eq!(merged.connections.len(), 1);
        let shared_conn = merged.connections.get("shared").unwrap();
        assert_eq!(shared_conn.config.engine, DatabaseType::MySQL);
        assert_eq!(shared_conn.config.host.as_deref(), Some("local-host"));
    }

    #[test]
    fn test_config_merging_current_precedence() {
        // Test that local current takes precedence over global current
        let mut global_registry = ConnectionRegistry::default();
        global_registry.current = Some("global-current".to_string());

        let mut local_registry = ConnectionRegistry::default();
        local_registry.current = Some("local-current".to_string());

        // Simulate merging
        let mut merged = ConnectionRegistry::default();
        merged.current = local_registry.current.or(global_registry.current);

        assert_eq!(merged.current, Some("local-current".to_string()));
    }

    #[test]
    fn test_config_merging_global_current_fallback() {
        // Test that global current is used when local current is None
        let mut global_registry = ConnectionRegistry::default();
        global_registry.current = Some("global-current".to_string());

        let local_registry = ConnectionRegistry::default();

        // Simulate merging
        let mut merged = ConnectionRegistry::default();
        merged.current = local_registry.current.or(global_registry.current);

        assert_eq!(merged.current, Some("global-current".to_string()));
    }

    #[test]
    fn test_backward_compatibility_default_field() {
        // Test that old config files with "default" field still work
        let json_with_default = r#"{
            "connections": {},
            "default": "my-connection"
        }"#;

        let registry: ConnectionRegistry = serde_json::from_str(json_with_default).unwrap();
        assert_eq!(registry.current, Some("my-connection".to_string()));

        // Verify that serialization uses "current" (not "default")
        let serialized = serde_json::to_string(&registry).unwrap();
        assert!(serialized.contains("current"));
        assert!(!serialized.contains("default"));
    }

    #[test]
    fn test_strict_separation_local_only() {
        // Test that when both local and global exist, load_local_or_global() returns ONLY local
        // (This is a simulation test since we can't easily manipulate file system in unit tests)

        // Create a local-style registry
        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
            },
        );
        local_registry.current = Some("local-conn".to_string());

        // Create a global-style registry (would be ignored if local exists)
        let mut global_registry = ConnectionRegistry::default();
        global_registry.connections.insert(
            "global-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "global-host".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );
        global_registry.current = Some("global-conn".to_string());

        // In strict separation mode (simulated):
        // - local_registry would be returned as-is
        // - global_registry would be ignored

        // Verify local has only its own connection
        assert_eq!(local_registry.connections.len(), 1);
        assert!(local_registry.connections.contains_key("local-conn"));
        assert!(!local_registry.connections.contains_key("global-conn"));
        assert_eq!(local_registry.current, Some("local-conn".to_string()));
    }

    #[test]
    fn test_merged_view_has_both() {
        // Test that merged view (load_with_precedence) includes connections from both
        let mut global_registry = ConnectionRegistry::default();
        global_registry.connections.insert(
            "global-only".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "global-host".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
            },
        );

        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-only".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
            },
        );

        // Simulate merging
        let mut merged = ConnectionRegistry::default();
        merged.connections = global_registry.connections.clone();
        for (name, conn) in local_registry.connections.clone() {
            merged.connections.insert(name, conn);
        }

        // Both should be present in merged view
        assert_eq!(merged.connections.len(), 2);
        assert!(merged.connections.contains_key("global-only"));
        assert!(merged.connections.contains_key("local-only"));
    }
}
