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
}

/// Stored connection configuration
///
/// Similar to `ConnectionConfig` but supports environment variable references
/// for sensitive fields like passwords.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    /// Connection configuration
    #[serde(flatten)]
    pub config: ConnectionConfig,

    /// Environment variable name for password (if not storing password directly)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,

    /// Whether this connection is readonly (rejects all write/DDL operations)
    /// Default: false (allows write/DDL if capabilities are provided)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
}

impl StoredConnection {
    /// Resolve environment variables and return a `ConnectionConfig` and readonly flag
    ///
    /// Returns a tuple of (`ConnectionConfig`, `is_readonly`)
    pub fn resolve(&self) -> Result<(ConnectionConfig, bool)> {
        let mut config = self.config.clone();

        // If password_env is set, resolve the environment variable
        if let Some(env_var) = &self.password_env {
            match std::env::var(env_var) {
                Ok(password) => config.password = Some(password),
                Err(_) => {
                    return Err(PlenumError::config_error(format!(
                        "Environment variable {env_var} not found for password"
                    )));
                }
            }
        }

        // Extract readonly flag (defaults to false if not set)
        let is_readonly = self.readonly.unwrap_or(false);

        Ok((config, is_readonly))
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
    let current_dir = std::env::current_dir().map_err(|e| {
        PlenumError::config_error(format!("Could not determine current directory: {e}"))
    })?;

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
        .map_err(|e| PlenumError::config_error(format!("Could not read config file: {e}")))?;

    let registry: ConnectionRegistry = serde_json::from_str(&contents)
        .map_err(|e| PlenumError::config_error(format!("Invalid config file format: {e}")))?;

    Ok(registry)
}

/// Save connection registry to a config file
pub fn save_registry(path: &Path, registry: &ConnectionRegistry) -> Result<()> {
    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            PlenumError::config_error(format!("Could not create config directory: {e}"))
        })?;
    }

    let contents = serde_json::to_string_pretty(registry)
        .map_err(|e| PlenumError::config_error(format!("Could not serialize config: {e}")))?;

    fs::write(path, contents)
        .map_err(|e| PlenumError::config_error(format!("Could not write config file: {e}")))?;

    Ok(())
}

/// Load connection registry with precedence (local first, then global)
///
/// This function merges both local and global configs:
/// - Connections from both configs are included
/// - Local connections override global connections with the same name
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

            // Start with global connections
            let mut merged = global_registry;

            // Override with local connections (local wins for same-named connections)
            for (name, conn) in local_registry.connections {
                merged.connections.insert(name, conn);
            }

            Ok(merged)
        }
    }
}

/// Resolve a connection by name
///
/// Searches in merged view (both local and global configs).
/// Returns an error if the connection is not found.
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
pub fn resolve_connection(name: &str) -> Result<(ConnectionConfig, bool)> {
    // Search in merged view (both local and global)
    let registry = load_with_precedence()?;

    // Look up connection by name
    let stored = registry
        .connections
        .get(name)
        .ok_or_else(|| PlenumError::config_error(format!("Connection '{name}' not found")))?;

    // Resolve environment variables and get readonly flag
    stored.resolve()
}

/// Save a connection to a config file
pub fn save_connection(
    name: String,
    config: ConnectionConfig,
    location: ConfigLocation,
) -> Result<()> {
    // Get config path
    let path = match location {
        ConfigLocation::Local => local_config_path()?,
        ConfigLocation::Global => global_config_path()?,
    };

    // Load existing registry (or create empty one)
    let mut registry =
        if path.exists() { load_registry(&path)? } else { ConnectionRegistry::default() };

    // Add or update connection
    registry
        .connections
        .insert(name, StoredConnection { config, password_env: None, readonly: None });

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
            Ok((config, _readonly)) => connections.push((name, config)),
            Err(_e) => {
                // Skip connections that fail to resolve (e.g., missing env vars)
                // Note: Error details not logged to prevent credential leakage
                eprintln!("Warning: Could not resolve connection '{name}'");
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
                readonly: None,
            },
        );

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
            readonly: None,
        };

        let (resolved, is_readonly) = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("pass".to_string()));
        assert!(!is_readonly);
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
            readonly: None,
        };

        let (resolved, is_readonly) = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("secret".to_string()));
        assert!(!is_readonly);

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
            readonly: None,
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
                readonly: None,
            },
        );

        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/test.db")),
                password_env: None,
                readonly: None,
            },
        );

        // Simulate merging (same logic as load_with_precedence when both exist)
        let mut merged = ConnectionRegistry { connections: global_registry.connections.clone() };
        for (name, conn) in local_registry.connections.clone() {
            merged.connections.insert(name, conn);
        }

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
                readonly: None,
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
                readonly: None,
            },
        );

        // Simulate merging
        let mut merged = global_registry;
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
    fn test_local_connections_separate() {
        // Test that local registries only contain their own connections
        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
                readonly: None,
            },
        );

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
                readonly: None,
            },
        );

        // Verify each registry has only its own connection
        assert_eq!(local_registry.connections.len(), 1);
        assert!(local_registry.connections.contains_key("local-conn"));
        assert!(!local_registry.connections.contains_key("global-conn"));

        assert_eq!(global_registry.connections.len(), 1);
        assert!(global_registry.connections.contains_key("global-conn"));
        assert!(!global_registry.connections.contains_key("local-conn"));
    }

    #[test]
    fn test_stored_connection_readonly_true() {
        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "pass".to_string(),
                "db".to_string(),
            ),
            password_env: None,
            readonly: Some(true),
        };

        let (resolved, is_readonly) = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("pass".to_string()));
        assert!(is_readonly);
    }

    #[test]
    fn test_stored_connection_readonly_false() {
        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "pass".to_string(),
                "db".to_string(),
            ),
            password_env: None,
            readonly: Some(false),
        };

        let (resolved, is_readonly) = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("pass".to_string()));
        assert!(!is_readonly);
    }

    #[test]
    fn test_stored_connection_readonly_defaults_to_false() {
        // When readonly is None, it should default to false
        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "pass".to_string(),
                "db".to_string(),
            ),
            password_env: None,
            readonly: None,
        };

        let (resolved, is_readonly) = stored.resolve().unwrap();
        assert_eq!(resolved.password, Some("pass".to_string()));
        assert!(!is_readonly); // Should default to false
    }

    #[test]
    fn test_readonly_serialization() {
        // Test that readonly field serializes correctly
        let mut registry = ConnectionRegistry::default();
        registry.connections.insert(
            "readonly-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "localhost".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
                readonly: Some(true),
            },
        );

        let json = serde_json::to_string_pretty(&registry).unwrap();
        assert!(json.contains("readonly"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_readonly_not_serialized_when_none() {
        // Test that readonly field is omitted when None (backwards compatibility)
        let mut registry = ConnectionRegistry::default();
        registry.connections.insert(
            "normal-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "localhost".to_string(),
                    5432,
                    "user".to_string(),
                    "pass".to_string(),
                    "db".to_string(),
                ),
                password_env: None,
                readonly: None,
            },
        );

        let json = serde_json::to_string_pretty(&registry).unwrap();
        // readonly field should not be present when it's None
        assert!(!json.contains("readonly"));
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
                readonly: None,
            },
        );

        let mut local_registry = ConnectionRegistry::default();
        local_registry.connections.insert(
            "local-only".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
                readonly: None,
            },
        );

        // Simulate merging
        let mut merged = ConnectionRegistry { connections: global_registry.connections.clone() };
        for (name, conn) in local_registry.connections.clone() {
            merged.connections.insert(name, conn);
        }

        // Both should be present in merged view
        assert_eq!(merged.connections.len(), 2);
        assert!(merged.connections.contains_key("global-only"));
        assert!(merged.connections.contains_key("local-only"));
    }
}
