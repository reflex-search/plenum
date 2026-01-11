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

/// Project configuration (per project path)
///
/// Contains named connections and a default pointer for a specific project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Named connections for this project
    pub connections: HashMap<String, StoredConnection>,

    /// Name of the default connection (must exist in connections map)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Local configuration (stored in `.plenum/config.json`)
///
/// This is a type alias for `ProjectConfig` because local configs are already
/// scoped to a project directory, so they don't need the `projects` wrapper.
pub type LocalConfig = ProjectConfig;

impl Default for ProjectConfig {
    fn default() -> Self {
        Self { connections: HashMap::new(), default: None }
    }
}

/// Connection registry (stored in config files)
///
/// Stores projects organized by project path, each containing connections and a default.
/// Format: `projects[project_path].connections[connection_name] = StoredConnection`
/// Example:
/// ```json
/// {
///   "projects": {
///     "/home/user/project1": {
///       "connections": {
///         "local": { ... },
///         "staging": { ... }
///       },
///       "default": "local"
///     }
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectionRegistry {
    /// Projects organized by project path
    pub projects: HashMap<String, ProjectConfig>,
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

/// Get the current project path (canonicalized current working directory)
///
/// This is used as the key for storing and retrieving connections in the registry.
/// Returns an absolute, canonicalized path.
pub fn get_current_project_path() -> Result<String> {
    let current_dir = std::env::current_dir().map_err(|e| {
        PlenumError::config_error(format!("Could not determine current directory: {e}"))
    })?;

    let canonical = current_dir.canonicalize().map_err(|e| {
        PlenumError::config_error(format!("Could not canonicalize current directory: {e}"))
    })?;

    canonical
        .to_str()
        .ok_or_else(|| PlenumError::config_error("Current directory path contains invalid UTF-8"))
        .map(std::string::ToString::to_string)
}

/// Load connection registry from a config file
///
/// Handles both formats:
/// - Local format: `{ "connections": {...}, "default": "..." }` (ProjectConfig)
/// - Global format: `{ "projects": { "/path": {...} } }` (ConnectionRegistry)
pub fn load_registry(path: &Path) -> Result<ConnectionRegistry> {
    if !path.exists() {
        // File doesn't exist, return empty registry
        return Ok(ConnectionRegistry::default());
    }

    let contents = fs::read_to_string(path)
        .map_err(|e| PlenumError::config_error(format!("Could not read config file: {e}")))?;

    // Determine if this is a local or global config
    let is_local_config =
        path.ends_with(".plenum/config.json") || path.ends_with(".plenum\\config.json"); // Windows support

    if is_local_config {
        // Parse as LocalConfig (ProjectConfig)
        let local_config = serde_json::from_str::<LocalConfig>(&contents)
            .map_err(|e| PlenumError::config_error(format!("Invalid local config file format: {e}")))?;

        // Convert to ConnectionRegistry with the current directory as project path
        let project_path = path
            .parent() // .plenum
            .and_then(|p| p.parent()) // project root
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.to_str().map(String::from))
            .ok_or_else(|| {
                PlenumError::config_error(
                    "Could not determine project path from config file location",
                )
            })?;

        let mut registry = ConnectionRegistry::default();
        registry.projects.insert(project_path, local_config);
        Ok(registry)
    } else {
        // Parse as ConnectionRegistry (global format)
        serde_json::from_str::<ConnectionRegistry>(&contents)
            .map_err(|e| PlenumError::config_error(format!("Invalid global config file format: {e}")))
    }
}

/// Save connection registry to a config file
///
/// Saves in different formats based on config type:
/// - Local: `{ "connections": {...}, "default": "..." }` (ProjectConfig only)
/// - Global: `{ "projects": { "/path": {...} } }` (Full ConnectionRegistry)
pub fn save_registry(path: &Path, registry: &ConnectionRegistry) -> Result<()> {
    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            PlenumError::config_error(format!("Could not create config directory: {e}"))
        })?;
    }

    // Determine if this is a local or global config
    let is_local_config =
        path.ends_with(".plenum/config.json") || path.ends_with(".plenum\\config.json"); // Windows support

    let contents = if is_local_config {
        // For local configs, extract the single project and save as LocalConfig
        let project_path = path
            .parent() // .plenum
            .and_then(|p| p.parent()) // project root
            .and_then(|p| p.canonicalize().ok())
            .and_then(|p| p.to_str().map(String::from))
            .ok_or_else(|| {
                PlenumError::config_error(
                    "Could not determine project path from config file location",
                )
            })?;

        let project_config = registry.projects.get(&project_path).ok_or_else(|| {
            PlenumError::config_error(format!(
                "No configuration found for project '{}' in registry",
                project_path
            ))
        })?;

        // Serialize as LocalConfig (which is just ProjectConfig)
        serde_json::to_string_pretty(project_config)
            .map_err(|e| PlenumError::config_error(format!("Could not serialize config: {e}")))?
    } else {
        // For global configs, save the full ConnectionRegistry
        serde_json::to_string_pretty(registry)
            .map_err(|e| PlenumError::config_error(format!("Could not serialize config: {e}")))?
    };

    fs::write(path, contents)
        .map_err(|e| PlenumError::config_error(format!("Could not write config file: {e}")))?;

    Ok(())
}

/// Load connection registry with precedence (local first, then global)
///
/// This function merges both local and global configs:
/// - Projects from both configs are included
/// - Local project config (connections + default) override global with same project path
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

            // Start with global projects
            let mut merged = global_registry;

            // Merge local projects (local wins for same project path)
            // Local completely overrides global for a given project path (including default)
            for (project_path, local_project) in local_registry.projects {
                let project_entry = merged.projects.entry(project_path).or_default();

                // Merge connections
                for (conn_name, conn) in local_project.connections {
                    project_entry.connections.insert(conn_name, conn);
                }

                // Override default pointer if local has one
                if local_project.default.is_some() {
                    project_entry.default = local_project.default;
                }
            }

            Ok(merged)
        }
    }
}

/// Resolve a connection by project path and name
///
/// Searches in merged view (both local and global configs).
/// Returns an error if the connection is not found.
/// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
///
/// # Parameters
/// - `project_path`: Optional project path. If None, uses current working directory.
/// - `name`: Optional connection name. If None, uses the project's default connection.
pub fn resolve_connection(
    project_path: Option<&str>,
    name: Option<&str>,
) -> Result<(ConnectionConfig, bool)> {
    // Determine project path (use provided or get current)
    let path = match project_path {
        Some(p) => p.to_string(),
        None => get_current_project_path()?,
    };

    // Search in merged view (both local and global)
    let registry = load_with_precedence()?;

    // Look up project in registry
    let project = registry.projects.get(&path).ok_or_else(|| {
        PlenumError::config_error(format!(
            "No connections found for project path '{path}'. Run 'plenum connect' to create one."
        ))
    })?;

    // Determine connection name (use provided or project's default)
    let conn_name = match name {
        Some(n) => n.to_string(),
        None => {
            // Use project's default
            project
                .default
                .as_ref()
                .ok_or_else(|| {
                    let available: Vec<_> = project.connections.keys().collect();
                    PlenumError::config_error(format!(
                    "No default connection set for project '{path}'. Available connections: {:?}. \
                     Specify one with --name or set a default in the config.",
                    available
                ))
                })?
                .clone()
        }
    };

    // Look up connection by name within project
    let stored = project.connections.get(&conn_name).ok_or_else(|| {
        let available: Vec<_> = project.connections.keys().collect();
        let default_info = match &project.default {
            Some(d) => format!(" (default: '{d}')"),
            None => String::new(),
        };
        PlenumError::config_error(format!(
            "Connection '{conn_name}' not found for project '{path}'. Available connections: {:?}{}",
            available, default_info
        ))
    })?;

    // Resolve environment variables and get readonly flag
    stored.resolve()
}

/// Save a connection to a config file
///
/// # Parameters
/// - `project_path`: Optional project path. If None, uses current working directory.
/// - `name`: Connection name. If this is the first connection for the project, it will be set as default.
/// - `config`: The connection configuration to save.
/// - `location`: Where to save (Local or Global).
pub fn save_connection(
    project_path: Option<String>,
    name: Option<String>,
    config: ConnectionConfig,
    location: ConfigLocation,
) -> Result<()> {
    // Determine project path (use provided or get current)
    let path = match project_path {
        Some(p) => p,
        None => get_current_project_path()?,
    };

    // Determine connection name
    let conn_name = name.unwrap_or_else(|| "default".to_string());

    // Get config file path
    let config_path = match location {
        ConfigLocation::Local => local_config_path()?,
        ConfigLocation::Global => global_config_path()?,
    };

    // Load existing registry (or create empty one)
    let mut registry = if config_path.exists() {
        load_registry(&config_path)?
    } else {
        ConnectionRegistry::default()
    };

    // Get or create project config
    let project = registry.projects.entry(path.clone()).or_default();

    // Check if this is the first connection for the project
    let is_first_connection = project.connections.is_empty();

    // Add or update connection
    project
        .connections
        .insert(conn_name.clone(), StoredConnection { config, password_env: None, readonly: None });

    // Auto-set as default if this is the first connection
    if is_first_connection {
        project.default = Some(conn_name);
    }

    // Save registry
    save_registry(&config_path, &registry)?;

    Ok(())
}

/// List all available connections
///
/// Returns a Vec of tuples: (`project_path`, `connection_name`, config)
pub fn list_connections() -> Result<Vec<(String, String, ConnectionConfig)>> {
    let registry = load_with_precedence()?;

    let mut connections = Vec::new();
    for (project_path, project) in registry.projects {
        for (conn_name, stored) in project.connections {
            match stored.resolve() {
                Ok((config, _readonly)) => {
                    connections.push((project_path.clone(), conn_name, config));
                }
                Err(_e) => {
                    // Skip connections that fail to resolve (e.g., missing env vars)
                    // Note: Error details not logged to prevent credential leakage
                    eprintln!(
                        "Warning: Could not resolve connection '{conn_name}' for project '{project_path}'"
                    );
                }
            }
        }
    }

    Ok(connections)
}

/// List connections for a specific project
///
/// Returns a Vec of tuples: (`connection_name`, config)
/// Only returns connections for the specified project path.
pub fn list_connections_for_project(project_path: &str) -> Result<Vec<(String, ConnectionConfig)>> {
    let registry = load_with_precedence()?;

    let mut connections = Vec::new();

    // Look up project in registry
    if let Some(project) = registry.projects.get(project_path) {
        for (conn_name, stored) in &project.connections {
            match stored.resolve() {
                Ok((config, _readonly)) => {
                    connections.push((conn_name.clone(), config));
                }
                Err(_e) => {
                    // Skip connections that fail to resolve (e.g., missing env vars)
                    // Note: Error details not logged to prevent credential leakage
                    eprintln!(
                        "Warning: Could not resolve connection '{conn_name}' for project '{project_path}'"
                    );
                }
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
        let mut project = ProjectConfig::default();
        project.connections.insert(
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
        project.default = Some("test".to_string());
        registry.projects.insert("/test/project".to_string(), project);

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
        assert!(registry.projects.is_empty());
    }

    #[test]
    fn test_config_merging_both_connections_visible() {
        // Test that connections from both local and global configs are visible
        let mut global_registry = ConnectionRegistry::default();
        let mut global_project = ProjectConfig::default();
        global_project.connections.insert(
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
        global_project.default = Some("global-conn".to_string());
        global_registry.projects.insert("/project1".to_string(), global_project);

        let mut local_registry = ConnectionRegistry::default();
        let mut local_project = ProjectConfig::default();
        local_project.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/test.db")),
                password_env: None,
                readonly: None,
            },
        );
        local_project.default = Some("local-conn".to_string());
        local_registry.projects.insert("/project2".to_string(), local_project);

        // Simulate merging (same logic as load_with_precedence when both exist)
        let mut merged = global_registry;
        for (project_path, local_proj) in local_registry.projects {
            let project_entry = merged.projects.entry(project_path).or_default();
            for (conn_name, conn) in local_proj.connections {
                project_entry.connections.insert(conn_name, conn);
            }
            if local_proj.default.is_some() {
                project_entry.default = local_proj.default;
            }
        }

        // Both projects should be present
        assert_eq!(merged.projects.len(), 2);
        assert!(merged.projects.contains_key("/project1"));
        assert!(merged.projects.contains_key("/project2"));
    }

    #[test]
    fn test_config_merging_local_overrides_global() {
        // Test that local connection overrides global connection with same project path + name
        let mut global_registry = ConnectionRegistry::default();
        let mut global_project = ProjectConfig::default();
        global_project.connections.insert(
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
        global_project.default = Some("shared".to_string());
        global_registry.projects.insert("/same/project".to_string(), global_project);

        let mut local_registry = ConnectionRegistry::default();
        let mut local_project = ProjectConfig::default();
        local_project.connections.insert(
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
        local_project.default = Some("shared".to_string());
        local_registry.projects.insert("/same/project".to_string(), local_project);

        // Simulate merging
        let mut merged = global_registry;
        for (project_path, local_proj) in local_registry.projects {
            let project_entry = merged.projects.entry(project_path).or_default();
            for (conn_name, conn) in local_proj.connections {
                project_entry.connections.insert(conn_name, conn);
            }
            if local_proj.default.is_some() {
                project_entry.default = local_proj.default;
            }
        }

        // Should have local version (MySQL, not Postgres)
        assert_eq!(merged.projects.len(), 1);
        let project = merged.projects.get("/same/project").unwrap();
        assert_eq!(project.connections.len(), 1);
        let shared_conn = project.connections.get("shared").unwrap();
        assert_eq!(shared_conn.config.engine, DatabaseType::MySQL);
        assert_eq!(shared_conn.config.host.as_deref(), Some("local-host"));
    }

    #[test]
    fn test_local_connections_separate() {
        // Test that local registries only contain their own project paths
        let mut local_registry = ConnectionRegistry::default();
        let mut local_project = ProjectConfig::default();
        local_project.connections.insert(
            "local-conn".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
                readonly: None,
            },
        );
        local_project.default = Some("local-conn".to_string());
        local_registry.projects.insert("/local/project".to_string(), local_project);

        let mut global_registry = ConnectionRegistry::default();
        let mut global_project = ProjectConfig::default();
        global_project.connections.insert(
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
        global_project.default = Some("global-conn".to_string());
        global_registry.projects.insert("/global/project".to_string(), global_project);

        // Verify each registry has only its own project path
        assert_eq!(local_registry.projects.len(), 1);
        assert!(local_registry.projects.contains_key("/local/project"));
        assert!(!local_registry.projects.contains_key("/global/project"));

        assert_eq!(global_registry.projects.len(), 1);
        assert!(global_registry.projects.contains_key("/global/project"));
        assert!(!global_registry.projects.contains_key("/local/project"));
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
        let mut project = ProjectConfig::default();
        project.connections.insert(
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
        project.default = Some("readonly-conn".to_string());
        registry.projects.insert("/test/project".to_string(), project);

        let json = serde_json::to_string_pretty(&registry).unwrap();
        assert!(json.contains("readonly"));
        assert!(json.contains("true"));
    }

    #[test]
    fn test_readonly_not_serialized_when_none() {
        // Test that readonly field is omitted when None (backwards compatibility)
        let mut registry = ConnectionRegistry::default();
        let mut project = ProjectConfig::default();
        project.connections.insert(
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
        project.default = Some("normal-conn".to_string());
        registry.projects.insert("/test/project".to_string(), project);

        let json = serde_json::to_string_pretty(&registry).unwrap();
        // readonly field should not be present when it's None
        assert!(!json.contains("readonly"));
    }

    #[test]
    fn test_merged_view_has_both() {
        // Test that merged view (load_with_precedence) includes connections from both
        let mut global_registry = ConnectionRegistry::default();
        let mut global_project = ProjectConfig::default();
        global_project.connections.insert(
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
        global_project.default = Some("global-only".to_string());
        global_registry.projects.insert("/project1".to_string(), global_project);

        let mut local_registry = ConnectionRegistry::default();
        let mut local_project = ProjectConfig::default();
        local_project.connections.insert(
            "local-only".to_string(),
            StoredConnection {
                config: ConnectionConfig::sqlite(PathBuf::from("/tmp/local.db")),
                password_env: None,
                readonly: None,
            },
        );
        local_project.default = Some("local-only".to_string());
        local_registry.projects.insert("/project2".to_string(), local_project);

        // Simulate merging
        let mut merged = global_registry;
        for (project_path, local_proj) in local_registry.projects {
            let project_entry = merged.projects.entry(project_path).or_default();
            for (conn_name, conn) in local_proj.connections {
                project_entry.connections.insert(conn_name, conn);
            }
            if local_proj.default.is_some() {
                project_entry.default = local_proj.default;
            }
        }

        // Both projects should be present in merged view
        assert_eq!(merged.projects.len(), 2);
        assert!(merged.projects.contains_key("/project1"));
        assert!(merged.projects.contains_key("/project2"));
    }
}
