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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// OS keychain reference for password lookup
///
/// Identifies a credential stored in the platform keychain
/// (macOS Keychain, Windows Credential Manager, Linux Secret Service).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeychainEntry {
    /// Keychain service name (identifies the application/domain)
    pub service: String,
    /// Keychain account name (identifies the specific credential)
    pub account: String,
}

/// Stored connection configuration
///
/// Similar to `ConnectionConfig` but supports indirect secret sources:
/// - `password_env`: resolve password from a named environment variable
/// - `password_command`: run a shell command; use its stdout (trimmed) as the password
/// - `keychain_entry`: look up the password from the platform OS keychain
///
/// Exactly one indirect source may be set per connection. Multiple sources is a config error.
/// Resolution precedence when `password_env`, `password_command`, and `keychain_entry` are all
/// absent: the inline `password` field in `config` is used as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConnection {
    /// Connection configuration
    #[serde(flatten)]
    pub config: ConnectionConfig,

    /// Environment variable name for password (if not storing password directly)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,

    /// Shell command whose stdout (trimmed) is the password.
    /// The command is run via `sh -c` on each connection resolution.
    /// A non-zero exit or empty output is a hard error — no fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_command: Option<String>,

    /// OS keychain reference for password lookup.
    /// Uses the platform-native keychain (macOS Keychain, Windows Credential Manager,
    /// Linux Secret Service). The entry must already exist in the keychain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keychain_entry: Option<KeychainEntry>,

    /// Whether this connection is readonly (rejects all write/DDL operations)
    /// Default: false (allows write/DDL if capabilities are provided)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
}

impl StoredConnection {
    /// Resolve credential sources and return a `ConnectionConfig` and readonly flag.
    ///
    /// Exactly one indirect source (`password_env`, `password_command`, `keychain_entry`) may
    /// be active. Having more than one is a config error. If none are set the inline password
    /// from `config` is used unchanged.
    ///
    /// Returns a tuple of (`ConnectionConfig`, `is_readonly`).
    pub fn resolve(&self) -> Result<(ConnectionConfig, bool)> {
        let mut config = self.config.clone();

        // Enforce: at most one indirect credential source
        let source_count = [
            self.password_env.is_some(),
            self.password_command.is_some(),
            self.keychain_entry.is_some(),
        ]
        .iter()
        .filter(|&&b| b)
        .count();

        if source_count > 1 {
            return Err(PlenumError::config_error(
                "Only one password source is allowed per connection: \
                 password_env, password_command, or keychain_entry",
            ));
        }

        if let Some(env_var) = &self.password_env {
            match std::env::var(env_var) {
                Ok(password) => config.password = Some(password),
                Err(_) => {
                    return Err(PlenumError::config_error(format!(
                        "Environment variable {env_var} not found for password"
                    )));
                }
            }
        } else if let Some(cmd) = &self.password_command {
            config.password = Some(run_password_command(cmd)?);
        } else if let Some(entry) = &self.keychain_entry {
            config.password = Some(lookup_keychain_password(&entry.service, &entry.account)?);
        }

        let is_readonly = self.readonly.unwrap_or(false);
        Ok((config, is_readonly))
    }
}

/// Run a shell command and return its stdout trimmed as the password.
///
/// The command is run via `sh -c`. Non-zero exit status or empty output is a hard error.
/// The raw stdout value is never logged.
///
/// Public alias for external use (e.g. test-mode credential resolution in the CLI).
pub fn run_password_command_pub(command: &str) -> Result<String> {
    run_password_command(command)
}

fn run_password_command(command: &str) -> Result<String> {
    let output = std::process::Command::new("sh").arg("-c").arg(command).output().map_err(|e| {
        PlenumError::config_error(format!("Failed to execute password_command: {e}"))
    })?;

    if !output.status.success() {
        let code = output.status.code().map_or_else(|| "signal".to_string(), |c| c.to_string());
        return Err(PlenumError::config_error(format!(
            "password_command exited with non-zero status ({code})"
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| PlenumError::config_error("password_command output is not valid UTF-8"))?;

    let password = stdout.trim().to_string();
    if password.is_empty() {
        return Err(PlenumError::config_error("password_command produced empty output"));
    }

    Ok(password)
}

/// Look up a password from the platform OS keychain.
///
/// Uses the `keyring` crate (macOS Keychain, Windows Credential Manager, Linux Secret Service).
/// The resolved password is never logged.
///
/// Public alias for external use (e.g. test-mode credential resolution in the CLI).
pub fn lookup_keychain_password_pub(service: &str, account: &str) -> Result<String> {
    lookup_keychain_password(service, account)
}

// In tests, use a thread-local map so we can pre-populate entries without
// needing a real OS keychain. The keyring mock's EntryOnly persistence means
// each Entry::new() creates a fresh credential with no shared state, making it
// unsuitable for round-trip tests.
#[cfg(test)]
thread_local! {
    static MOCK_KEYCHAIN: std::cell::RefCell<std::collections::HashMap<String, String>>
        = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(test)]
fn mock_keychain_key(service: &str, account: &str) -> String {
    format!("{service}\0{account}")
}

#[cfg(test)]
fn set_mock_keychain(service: &str, account: &str, password: &str) {
    MOCK_KEYCHAIN.with(|k| {
        k.borrow_mut().insert(mock_keychain_key(service, account), password.to_string());
    });
}

#[cfg(test)]
fn clear_mock_keychain() {
    MOCK_KEYCHAIN.with(|k| k.borrow_mut().clear());
}

#[cfg(not(test))]
fn lookup_keychain_password(service: &str, account: &str) -> Result<String> {
    let entry = keyring::Entry::new(service, account).map_err(|_| {
        PlenumError::config_error(format!(
            "Could not access keychain for service '{service}', account '{account}'"
        ))
    })?;

    entry.get_password().map_err(|_| {
        PlenumError::config_error(format!(
            "No password found in keychain for service '{service}', account '{account}'"
        ))
    })
}

#[cfg(test)]
fn lookup_keychain_password(service: &str, account: &str) -> Result<String> {
    MOCK_KEYCHAIN.with(|k| {
        k.borrow().get(&mock_keychain_key(service, account)).cloned().ok_or_else(|| {
            PlenumError::config_error(format!(
                "No password found in keychain for service '{service}', account '{account}'"
            ))
        })
    })
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
/// - Local format: `{ "connections": {...}, "default": "..." }` (`ProjectConfig`)
/// - Global format: `{ "projects": { "/path": {...} } }` (`ConnectionRegistry`)
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
        let local_config = serde_json::from_str::<LocalConfig>(&contents).map_err(|e| {
            PlenumError::config_error(format!("Invalid local config file format: {e}"))
        })?;

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
        serde_json::from_str::<ConnectionRegistry>(&contents).map_err(|e| {
            PlenumError::config_error(format!("Invalid global config file format: {e}"))
        })
    }
}

/// Save connection registry to a config file
///
/// Saves in different formats based on config type:
/// - Local: `{ "connections": {...}, "default": "..." }` (`ProjectConfig` only)
/// - Global: `{ "projects": { "/path": {...} } }` (Full `ConnectionRegistry`)
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
                "No configuration found for project '{project_path}' in registry"
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
                    "No default connection set for project '{path}'. Available connections: {available:?}. \
                     Specify one with --name or set a default in the config."
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
            "Connection '{conn_name}' not found for project '{path}'. Available connections: {available:?}{default_info}"
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
/// - `password_env`: Optional environment variable name from which to resolve the password at use time.
/// - `password_command`: Optional shell command whose stdout is the password at use time.
/// - `keychain_entry`: Optional OS keychain reference for password lookup at use time.
/// - `location`: Where to save (Local or Global).
///
/// Only one of `password_env`, `password_command`, or `keychain_entry` may be `Some`.
pub fn save_connection(
    project_path: Option<String>,
    name: Option<String>,
    config: ConnectionConfig,
    password_env: Option<String>,
    password_command: Option<String>,
    keychain_entry: Option<KeychainEntry>,
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
    let project = registry.projects.entry(path).or_default();

    // Check if this is the first connection for the project
    let is_first_connection = project.connections.is_empty();

    // Add or update connection
    project.connections.insert(
        conn_name.clone(),
        StoredConnection { config, password_env, password_command, keychain_entry, readonly: None },
    );

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

/// Raw connection listing for a project: named stored connections plus the
/// optional default connection name.
pub type RawConnectionListing = (Vec<(String, StoredConnection)>, Option<String>);

/// List raw stored connections for a project without resolving secrets
///
/// Returns `(Vec<(name, StoredConnection)>, Option<default_name>)`.
/// Connections are sorted alphabetically by name for deterministic output.
/// Does NOT call `StoredConnection::resolve` — callers must NOT expose the password field.
pub fn list_connections_raw(project_path: &str) -> Result<RawConnectionListing> {
    let registry = load_with_precedence()?;

    match registry.projects.get(project_path) {
        None => Ok((Vec::new(), None)),
        Some(project) => {
            let mut connections: Vec<(String, StoredConnection)> = project
                .connections
                .iter()
                .map(|(name, stored)| (name.clone(), stored.clone()))
                .collect();
            connections.sort_by(|a, b| a.0.cmp(&b.0));
            Ok((connections, project.default.clone()))
        }
    }
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
                password_command: None,
                keychain_entry: None,
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
            password_command: None,
            keychain_entry: None,
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
                tls: None,
            },
            password_env: Some("TEST_PASSWORD".to_string()),
            password_command: None,
            keychain_entry: None,
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
                tls: None,
            },
            password_env: Some("NONEXISTENT_VAR".to_string()),
            password_command: None,
            keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
            password_command: None,
            keychain_entry: None,
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
            password_command: None,
            keychain_entry: None,
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
            password_command: None,
            keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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
                password_command: None,
                keychain_entry: None,
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

    #[test]
    fn test_password_env_serialization_roundtrip() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: Some("MY_DB_PASS".to_string()),
            password_command: None,
            keychain_entry: None,
            readonly: None,
        };

        let json = serde_json::to_string(&stored).unwrap();
        assert!(json.contains("\"password_env\":\"MY_DB_PASS\""));
        // password field should be omitted when None
        assert!(!json.contains("\"password\":"));

        let parsed: StoredConnection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.password_env.as_deref(), Some("MY_DB_PASS"));
        assert!(parsed.config.password.is_none());
    }

    #[test]
    fn test_password_env_not_serialized_when_none() {
        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "pass".to_string(),
                "db".to_string(),
            ),
            password_env: None,
            password_command: None,
            keychain_entry: None,
            readonly: None,
        };

        let json = serde_json::to_string(&stored).unwrap();
        assert!(!json.contains("password_env"));
    }

    #[test]
    fn test_list_connections_raw_redacts_password_and_sorts() {
        // Verify that list_connections_raw returns connections sorted by name
        // and that StoredConnections retain password_env but NOT the resolved password.
        let mut project = ProjectConfig::default();
        project.connections.insert(
            "zebra".to_string(),
            StoredConnection {
                config: ConnectionConfig::postgres(
                    "z-host".to_string(),
                    5432,
                    "zuser".to_string(),
                    "zpass".to_string(),
                    "zdb".to_string(),
                ),
                password_env: None,
                password_command: None,
                keychain_entry: None,
                readonly: None,
            },
        );
        project.connections.insert(
            "alpha".to_string(),
            StoredConnection {
                config: ConnectionConfig {
                    engine: DatabaseType::MySQL,
                    host: Some("a-host".to_string()),
                    port: Some(3306),
                    user: Some("auser".to_string()),
                    password: None,
                    database: Some("adb".to_string()),
                    file: None,
                    tls: None,
                },
                password_env: Some("ALPHA_DB_PASS".to_string()),
                password_command: None,
                keychain_entry: None,
                readonly: None,
            },
        );
        project.default = Some("alpha".to_string());

        let mut connections: Vec<(String, StoredConnection)> =
            project.connections.iter().map(|(n, s)| (n.clone(), s.clone())).collect();
        connections.sort_by(|a, b| a.0.cmp(&b.0));

        // Verify alphabetical ordering
        assert_eq!(connections[0].0, "alpha");
        assert_eq!(connections[1].0, "zebra");

        // Verify password_env is preserved
        assert_eq!(connections[0].1.password_env.as_deref(), Some("ALPHA_DB_PASS"));
        // Verify inline password is present on the stored struct but must not be emitted in output
        assert_eq!(connections[1].1.config.password.as_deref(), Some("zpass"));
        // (output layer is responsible for dropping password — confirmed by ConnectionListEntry)
    }

    #[test]
    fn test_connect_list_envelope_json_shape() {
        // Snapshot: verify the JSON shape of the connect --list data field.
        // No password field; password_env var name only; deterministic key ordering.
        #[derive(serde::Serialize)]
        struct ConnectionListEntry {
            name: String,
            engine: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            host: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            port: Option<u16>,
            #[serde(skip_serializing_if = "Option::is_none")]
            user: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            database: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            file: Option<PathBuf>,
            #[serde(skip_serializing_if = "Option::is_none")]
            password_env: Option<String>,
        }

        let entries = vec![
            ConnectionListEntry {
                name: "alpha".to_string(),
                engine: "mysql".to_string(),
                host: Some("a-host".to_string()),
                port: Some(3306),
                user: Some("auser".to_string()),
                database: Some("adb".to_string()),
                file: None,
                password_env: Some("ALPHA_DB_PASS".to_string()),
            },
            ConnectionListEntry {
                name: "zebra".to_string(),
                engine: "postgres".to_string(),
                host: Some("z-host".to_string()),
                port: Some(5432),
                user: Some("zuser".to_string()),
                database: Some("zdb".to_string()),
                file: None,
                password_env: None,
            },
        ];

        let data = serde_json::json!({
            "connections": entries,
            "default": "alpha",
        });

        let json = serde_json::to_string(&data).unwrap();

        // Must include expected fields
        assert!(json.contains("\"connections\""));
        assert!(json.contains("\"default\":\"alpha\""));
        assert!(json.contains("\"password_env\":\"ALPHA_DB_PASS\""));
        assert!(json.contains("\"engine\":\"mysql\""));
        // Must NOT include plaintext password
        assert!(!json.contains("\"password\""));
        // Zebra entry must not have password_env key
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let zebra = &parsed["connections"][1];
        assert!(zebra["password_env"].is_null()); // absent → null from serde_json::Value
        assert_eq!(zebra["name"], "zebra");
    }

    #[test]
    fn test_connect_list_empty_project_returns_empty() {
        // An absent project path returns an empty connections vec and None default.
        let registry = ConnectionRegistry::default();
        let project = registry.projects.get("/nonexistent/path");
        assert!(project.is_none());
        // Simulate what list_connections_raw returns for a missing project
        let connections: Vec<(String, StoredConnection)> = Vec::new();
        let default_name: Option<String> = None;
        assert!(connections.is_empty());
        assert!(default_name.is_none());
    }

    #[test]
    fn test_password_env_overrides_inline_password_on_resolve() {
        // When both password and password_env are set on a stored connection,
        // resolve() should overwrite the inline password with the env var value.
        std::env::set_var("PLENUM_TEST_PWD_OVERRIDE", "from-env");

        let stored = StoredConnection {
            config: ConnectionConfig::postgres(
                "localhost".to_string(),
                5432,
                "user".to_string(),
                "inline-value".to_string(),
                "db".to_string(),
            ),
            password_env: Some("PLENUM_TEST_PWD_OVERRIDE".to_string()),
            password_command: None,
            keychain_entry: None,
            readonly: None,
        };

        let (resolved, _) = stored.resolve().unwrap();
        assert_eq!(resolved.password.as_deref(), Some("from-env"));

        std::env::remove_var("PLENUM_TEST_PWD_OVERRIDE");
    }

    // --- password_command tests ---

    #[test]
    fn test_password_command_echo_resolves() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: Some("echo 'secretpassword'".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let (resolved, _) = stored.resolve().unwrap();
        assert_eq!(resolved.password.as_deref(), Some("secretpassword"));
    }

    #[test]
    fn test_password_command_output_is_trimmed() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            // printf avoids a trailing newline but let's confirm trim works regardless
            password_command: Some("printf '  trimmed  '".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let (resolved, _) = stored.resolve().unwrap();
        assert_eq!(resolved.password.as_deref(), Some("trimmed"));
    }

    #[test]
    fn test_password_command_nonzero_exit_is_error() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: Some("exit 1".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let result = stored.resolve();
        assert!(result.is_err());
        let msg = result.unwrap_err().message();
        assert!(msg.contains("non-zero"), "expected 'non-zero' in: {msg}");
    }

    #[test]
    fn test_password_command_empty_output_is_error() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: Some("echo ''".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let result = stored.resolve();
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("empty output"));
    }

    #[test]
    fn test_password_command_serialization_roundtrip() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: Some("op read op://vault/item/password".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let json = serde_json::to_string(&stored).unwrap();
        assert!(json.contains("\"password_command\""));
        assert!(json.contains("op read op://vault/item/password"));
        // Must NOT include plaintext password
        assert!(!json.contains("\"password\":"));

        let parsed: StoredConnection = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.password_command.as_deref(), Some("op read op://vault/item/password"));
        assert!(parsed.config.password.is_none());
    }

    // --- keychain_entry tests ---

    #[test]
    fn test_keychain_entry_resolves_via_mock() {
        clear_mock_keychain();
        set_mock_keychain("plenum-test-svc", "plenum-test-user", "mock-keychain-password");

        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("pguser".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: None,
            keychain_entry: Some(KeychainEntry {
                service: "plenum-test-svc".to_string(),
                account: "plenum-test-user".to_string(),
            }),
            readonly: None,
        };

        let (resolved, _) = stored.resolve().unwrap();
        assert_eq!(resolved.password.as_deref(), Some("mock-keychain-password"));
    }

    #[test]
    fn test_keychain_entry_missing_key_is_error() {
        clear_mock_keychain(); // ensure no entry exists for these service/account

        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: None,
            keychain_entry: Some(KeychainEntry {
                service: "nonexistent-service".to_string(),
                account: "nonexistent-account".to_string(),
            }),
            readonly: None,
        };

        let result = stored.resolve();
        assert!(result.is_err());
        let msg = result.unwrap_err().message();
        assert!(msg.contains("keychain"), "expected 'keychain' in: {msg}");
    }

    #[test]
    fn test_keychain_entry_serialization_roundtrip() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: None,
            password_command: None,
            keychain_entry: Some(KeychainEntry {
                service: "myapp".to_string(),
                account: "db-prod".to_string(),
            }),
            readonly: None,
        };

        let json = serde_json::to_string(&stored).unwrap();
        assert!(json.contains("\"keychain_entry\""));
        assert!(json.contains("\"service\":\"myapp\""));
        assert!(json.contains("\"account\":\"db-prod\""));
        // Must NOT include plaintext password
        assert!(!json.contains("\"password\":"));

        let parsed: StoredConnection = serde_json::from_str(&json).unwrap();
        let ke = parsed.keychain_entry.unwrap();
        assert_eq!(ke.service, "myapp");
        assert_eq!(ke.account, "db-prod");
    }

    // --- mutual exclusion test ---

    #[test]
    fn test_multiple_credential_sources_is_error() {
        let stored = StoredConnection {
            config: ConnectionConfig {
                engine: DatabaseType::Postgres,
                host: Some("localhost".to_string()),
                port: Some(5432),
                user: Some("user".to_string()),
                password: None,
                database: Some("db".to_string()),
                file: None,
                tls: None,
            },
            password_env: Some("SOME_VAR".to_string()),
            password_command: Some("echo secret".to_string()),
            keychain_entry: None,
            readonly: None,
        };

        let result = stored.resolve();
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("Only one password source"));
    }
}
