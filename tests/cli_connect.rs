//! CLI integration coverage for `plenum connect --password-env`.
//!
//! Drives the compiled `plenum` binary against a scratch project directory and
//! asserts that:
//!   - a `--password-env VAR` invocation persists the variable name (not the value)
//!     into the local config file,
//!   - a missing/empty env var produces a clear error,
//!   - error output never leaks the resolved credential.

use rusqlite;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_tmp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("plenum_cli_connect_{tag}_{pid}_{id}_{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Spawn `plenum connect ...` in `cwd` with `env` overrides.
/// Returns (exit code, stdout, stderr).
fn run_connect(cwd: &Path, env: &[(&str, &str)], args: &[&str]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let mut cmd = Command::new(bin);
    cmd.arg("connect");
    cmd.args(args);
    cmd.current_dir(cwd);
    // Point HOME/XDG_CONFIG_HOME at the scratch dir so any stray global
    // config writes stay contained.
    cmd.env("HOME", cwd);
    cmd.env("XDG_CONFIG_HOME", cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("spawn plenum");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn connect_password_env_persists_variable_name_not_value() {
    let dir = unique_tmp_dir("ok");
    let secret = "s3cret-do-not-persist";

    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[("PLENUM_TEST_PWD_OK", secret)],
        &[
            "--engine",
            "postgres",
            "--host",
            "db.example.com",
            "--port",
            "5432",
            "--user",
            "plenum",
            "--password-env",
            "PLENUM_TEST_PWD_OK",
            "--database",
            "appdb",
            "--save",
            "local",
        ],
    );

    assert_eq!(code, 0, "expected success, stdout={stdout}");

    // Stdout should be a single-line success envelope; the secret must not be in it.
    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("non-JSON stdout {stdout:?}: {e}"));
    assert_eq!(envelope.get("ok").and_then(Value::as_bool), Some(true));
    assert!(!stdout.contains(secret), "secret leaked into stdout");

    // Inspect persisted config: must reference the env var by name only.
    let cfg_path = dir.join(".plenum").join("config.json");
    let contents = std::fs::read_to_string(&cfg_path).expect("local config written");
    let parsed: Value =
        serde_json::from_str(&contents).expect("local config is valid JSON");

    let conn = parsed
        .pointer("/connections/default")
        .expect("default connection saved under /connections/default");
    assert_eq!(
        conn.get("password_env").and_then(Value::as_str),
        Some("PLENUM_TEST_PWD_OK"),
        "password_env should be persisted by name"
    );
    assert!(
        conn.get("password").is_none(),
        "literal password must not be persisted when --password-env is used"
    );
    assert!(!contents.contains(secret), "secret leaked into config file");

    cleanup(&dir);
}

#[test]
fn connect_password_env_missing_var_errors_without_leaking() {
    let dir = unique_tmp_dir("missing");
    // Deliberately do NOT set the env var.
    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[],
        &[
            "--engine",
            "postgres",
            "--host",
            "db.example.com",
            "--port",
            "5432",
            "--user",
            "plenum",
            "--password-env",
            "PLENUM_TEST_PWD_MISSING_VAR_XYZ",
            "--database",
            "appdb",
            "--save",
            "local",
        ],
    );

    assert_ne!(code, 0, "missing env var must fail");

    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("error envelope must be JSON: {e}: {stdout:?}"));
    assert_eq!(envelope.get("ok").and_then(Value::as_bool), Some(false));
    let message = envelope
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        message.contains("PLENUM_TEST_PWD_MISSING_VAR_XYZ"),
        "error message should name the missing variable, got {message:?}"
    );
    assert!(
        message.contains("not set") || message.contains("not found"),
        "error message should explain the variable is unset, got {message:?}"
    );

    // No config file should have been written.
    let cfg_path = dir.join(".plenum").join("config.json");
    assert!(!cfg_path.exists(), "no config should be written on failure");

    cleanup(&dir);
}

#[test]
fn connect_rejects_password_and_password_env_together() {
    let dir = unique_tmp_dir("conflict");
    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[("PLENUM_TEST_PWD_CONFLICT", "ignored")],
        &[
            "--engine",
            "postgres",
            "--host",
            "db.example.com",
            "--port",
            "5432",
            "--user",
            "plenum",
            "--password",
            "literal",
            "--password-env",
            "PLENUM_TEST_PWD_CONFLICT",
            "--database",
            "appdb",
            "--save",
            "local",
        ],
    );

    assert_ne!(code, 0, "mutually exclusive flags must fail");

    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("error envelope must be JSON: {e}: {stdout:?}"));
    let message = envelope
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        message.contains("mutually exclusive"),
        "expected mutual-exclusion error, got {message:?}"
    );

    cleanup(&dir);
}

// ============================================================================
// --test (connection ping) tests
// ============================================================================

/// Create a minimal valid SQLite database file and return its path.
fn create_sqlite_db(dir: &Path) -> PathBuf {
    let db_path = dir.join("test.db");
    let conn = rusqlite::Connection::open(&db_path).expect("create sqlite db");
    conn.execute("CREATE TABLE _ping (id INTEGER PRIMARY KEY)", [])
        .expect("create ping table");
    db_path
}

#[test]
fn connect_test_reachable_sqlite_returns_connection_info() {
    let dir = unique_tmp_dir("test_ok");
    let db_path = create_sqlite_db(&dir);

    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[],
        &["--test", "--engine", "sqlite", "--file", db_path.to_str().unwrap()],
    );

    assert_eq!(code, 0, "expected success, stdout={stdout}");

    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("non-JSON stdout {stdout:?}: {e}"));
    assert_eq!(envelope.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(envelope.get("command").and_then(Value::as_str), Some("connect"));
    assert_eq!(envelope.get("engine").and_then(Value::as_str), Some("sqlite"));

    let data = envelope.get("data").expect("data field present");
    assert!(data.get("database_version").and_then(Value::as_str).is_some(), "database_version present");
    assert!(data.get("server_info").and_then(Value::as_str).is_some(), "server_info present");
    assert!(data.get("connected_database").and_then(Value::as_str).is_some(), "connected_database present");
    assert!(data.get("user").and_then(Value::as_str).is_some(), "user present");

    // --test must not write a config file
    assert!(!dir.join(".plenum").join("config.json").exists(), "no config saved in test mode");

    cleanup(&dir);
}

#[test]
fn connect_test_unreachable_sqlite_returns_connection_failed() {
    let dir = unique_tmp_dir("test_fail");

    let missing = dir.join("does_not_exist.db");
    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[],
        &["--test", "--engine", "sqlite", "--file", missing.to_str().unwrap()],
    );

    assert_ne!(code, 0, "expected failure for missing file");

    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("error envelope must be JSON: {e}: {stdout:?}"));
    assert_eq!(envelope.get("ok").and_then(Value::as_bool), Some(false));
    assert_eq!(
        envelope.pointer("/error/code").and_then(Value::as_str),
        Some("CONNECTION_FAILED"),
        "error code must be CONNECTION_FAILED"
    );

    // No config should have been written
    assert!(!dir.join(".plenum").join("config.json").exists(), "no config saved on failure");

    cleanup(&dir);
}

#[test]
fn connect_test_uses_saved_connection_by_name() {
    let dir = unique_tmp_dir("test_named");
    let db_path = create_sqlite_db(&dir);

    // First, save a named connection
    let (save_code, _, _) = run_connect(
        &dir,
        &[],
        &[
            "--name", "myconn",
            "--engine", "sqlite",
            "--file", db_path.to_str().unwrap(),
            "--save", "local",
        ],
    );
    assert_eq!(save_code, 0, "save should succeed");

    // Now test it by name
    let (code, stdout, _stderr) = run_connect(&dir, &[], &["--test", "--name", "myconn"]);

    assert_eq!(code, 0, "expected success, stdout={stdout}");

    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("non-JSON stdout {stdout:?}: {e}"));
    assert_eq!(envelope.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(envelope.get("engine").and_then(Value::as_str), Some("sqlite"));

    cleanup(&dir);
}

#[test]
fn connect_test_does_not_save_config() {
    let dir = unique_tmp_dir("test_nosave");
    let db_path = create_sqlite_db(&dir);

    let (code, stdout, _stderr) = run_connect(
        &dir,
        &[],
        &["--test", "--engine", "sqlite", "--file", db_path.to_str().unwrap()],
    );

    assert_eq!(code, 0, "expected success, stdout={stdout}");

    // Verify no config directory or file was created
    assert!(
        !dir.join(".plenum").exists(),
        ".plenum dir must not exist after --test"
    );

    cleanup(&dir);
}

#[test]
fn connect_test_and_save_are_mutually_exclusive() {
    let dir = unique_tmp_dir("test_conflict");
    let db_path = create_sqlite_db(&dir);

    // clap should reject --test --save together
    let (code, stdout, stderr) = run_connect(
        &dir,
        &[],
        &[
            "--test",
            "--save", "local",
            "--engine", "sqlite",
            "--file", db_path.to_str().unwrap(),
        ],
    );

    assert_ne!(code, 0, "expected failure: stdout={stdout} stderr={stderr}");

    cleanup(&dir);
}
