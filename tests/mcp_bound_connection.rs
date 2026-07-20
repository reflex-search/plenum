//! End-to-end coverage for the `plenum mcp` connection-binding flags (REF-297):
//! `--project-path`, `--name`, and `--dsn-env`.
//!
//! The MCP server is decoupled from the launcher's cwd: the binding captured at
//! `plenum mcp` startup supplies the default connection source for tool calls
//! that omit their own connection selectors. Every test drives the compiled
//! `plenum mcp` binary over stdio.
//!
//! Contract notes exercised here:
//! - Errors from a tool call surface as JSON-RPC error responses (the server's
//!   existing error surface); the `error.message` is what callers read.
//! - `--dsn-env` reads the DSN at connection time and consults ONLY the named
//!   variable — never an ambient source such as `DATABASE_URL`.
//! - `--project-path` re-keys config resolution; global config is keyed by
//!   project path, so a global connection resolves regardless of cwd.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);

// ─── helpers ────────────────────────────────────────────────────────────────

/// Create a unique, empty scratch directory with no `.plenum` config.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("plenum-bind-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Spawn `plenum mcp <extra_args>` with the given env overrides and cwd.
///
/// `xdg_config_home` isolates global-config lookup to a per-test directory so
/// the developer's real `~/.config/plenum` never influences the result.
fn spawn_mcp(
    extra_args: &[&str],
    envs: &[(&str, &str)],
    cwd: &Path,
    xdg_config_home: &Path,
) -> Child {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let mut cmd = Command::new(bin);
    cmd.arg("mcp").args(extra_args);
    cmd.env("XDG_CONFIG_HOME", xdg_config_home);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn plenum mcp")
}

fn read_line_json(reader: &mut BufReader<impl std::io::Read>) -> Option<Value> {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let mut buf = String::new();
        match reader.read_line(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                let t = buf.trim();
                if t.is_empty() {
                    continue;
                }
                return serde_json::from_str(t).ok();
            }
        }
    }
    None
}

/// Drive the handshake and a single `connect` tool call (no connection args),
/// returning the JSON-RPC response for the call.
fn handshake_then_connect(child: &mut Child) -> Value {
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);

    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" } }
    });
    let notif = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "connect", "arguments": {} }
    });

    writeln!(stdin, "{init}").unwrap();
    writeln!(stdin, "{notif}").unwrap();
    writeln!(stdin, "{call}").unwrap();
    stdin.flush().unwrap();
    drop(stdin);

    let mut response = None;
    while let Some(v) = read_line_json(&mut reader) {
        if v.get("id") == Some(&json!(2)) {
            response = Some(v);
            break;
        }
    }
    response.expect("no response for connect tool call")
}

/// Create an on-disk `SQLite` database at `path`. Plenum opens connections
/// read-only, so the file must already exist for `connect` to succeed.
fn create_sqlite_db(path: &Path) {
    let conn = rusqlite::Connection::open(path).expect("create sqlite db");
    conn.execute_batch("CREATE TABLE IF NOT EXISTS probe (id INTEGER PRIMARY KEY)")
        .expect("seed sqlite db");
}

/// Write a global config registry (`{ "projects": { <path>: {...} } }`) into
/// `<xdg>/plenum/connections.json`.
fn write_global_config(xdg: &Path, registry: &Value) {
    let dir = xdg.join("plenum");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("connections.json"), registry.to_string()).unwrap();
}

fn error_message(resp: &Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error response, got: {resp}"))
        .to_string()
}

fn assert_no_error(resp: &Value) {
    assert!(
        resp.get("error").is_none(),
        "expected a successful tool result, got JSON-RPC error: {:?}",
        resp.get("error")
    );
}

// ─── Criterion 4: no binding + no config → structured error naming the flags ──

#[test]
fn no_binding_and_no_config_names_the_required_flags() {
    let cwd = scratch("noflags");
    let xdg = scratch("noflags-xdg"); // empty → no global config
    let mut child = spawn_mcp(&[], &[], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    let msg = error_message(&resp);
    assert!(msg.contains("--project-path"), "must name --project-path: {msg}");
    assert!(msg.contains("--name"), "must name --name: {msg}");
    assert!(msg.contains("--dsn-env"), "must name --dsn-env: {msg}");

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

// ─── Credential safety: no ambient pickup of DATABASE_URL ─────────────────────

#[test]
fn database_url_is_never_used_as_an_ambient_source() {
    let cwd = scratch("ambient");
    let xdg = scratch("ambient-xdg");
    // A valid ambient DSN is present but no --dsn-env flag names it.
    let mut child = spawn_mcp(&[], &[("DATABASE_URL", "sqlite::memory:")], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    // Must fail (not silently resolve from DATABASE_URL), and the error must not
    // reference the ambient variable it was never told to read.
    let msg = error_message(&resp);
    assert!(!msg.contains("DATABASE_URL"), "must not reference the ambient var: {msg}");
    assert!(msg.contains("--dsn-env"), "should point the operator at the flags: {msg}");

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

// ─── Criterion 3: --dsn-env resolves the DSN from ONLY the named variable ─────

#[test]
fn dsn_env_resolves_connection_from_named_variable() {
    let cwd = scratch("dsnenv-ok");
    let xdg = scratch("dsnenv-ok-xdg");
    let db = cwd.join("app.db");
    create_sqlite_db(&db);
    let dsn = format!("sqlite:{}", db.display());

    let mut child =
        spawn_mcp(&["--dsn-env", "PLENUM_ITEST_DSN"], &[("PLENUM_ITEST_DSN", &dsn)], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert_no_error(&resp);
    let content = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
    assert!(!content.is_empty(), "expected ConnectionInfo content, got empty: {resp}");

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn dsn_env_unset_variable_errors_and_names_the_variable() {
    let cwd = scratch("dsnenv-unset");
    let xdg = scratch("dsnenv-unset-xdg");
    // The var is deliberately not provided to the child.
    let mut child = spawn_mcp(&["--dsn-env", "PLENUM_ITEST_UNSET"], &[], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    let msg = error_message(&resp);
    assert!(msg.contains("PLENUM_ITEST_UNSET"), "error must name the missing var: {msg}");

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

// ─── Criterion 1: --project-path pins resolution regardless of launcher cwd ───

#[test]
fn project_path_pins_resolution_regardless_of_cwd() {
    let cwd = scratch("projpath-cwd"); // empty: no local config here
    let xdg = scratch("projpath-xdg");
    let db = xdg.join("proj.db");
    create_sqlite_db(&db);
    let proj = format!("/plenum-itest-proj-{}", std::process::id());

    // Global config keyed by the project path — resolvable from any cwd.
    write_global_config(
        &xdg,
        &json!({
            "projects": {
                proj.clone(): {
                    "connections": {
                        "default": { "engine": "sqlite", "file": db.to_str().unwrap() }
                    },
                    "default": "default"
                }
            }
        }),
    );

    let mut child = spawn_mcp(&["--project-path", &proj], &[], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert_no_error(&resp);

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn project_path_loads_local_config_from_the_project_directory() {
    // The core of `resolve_connection_in_project`: a LOCAL `.plenum/config.json`
    // living inside the pinned project is loaded even when the server is launched
    // from an unrelated cwd with no config of its own.
    let proj = scratch("localcfg-proj");
    let launch_cwd = scratch("localcfg-launch"); // different, config-less cwd
    let xdg = scratch("localcfg-xdg"); // empty global config
    let db = proj.join("app.db");
    create_sqlite_db(&db);

    let plenum_dir = proj.join(".plenum");
    std::fs::create_dir_all(&plenum_dir).unwrap();
    std::fs::write(
        plenum_dir.join("config.json"),
        json!({
            "connections": { "default": { "engine": "sqlite", "file": db.to_str().unwrap() } },
            "default": "default"
        })
        .to_string(),
    )
    .unwrap();

    let proj_str = proj.to_str().unwrap();
    let mut child = spawn_mcp(&["--project-path", proj_str], &[], &launch_cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert_no_error(&resp);

    let _ = std::fs::remove_dir_all(&proj);
    let _ = std::fs::remove_dir_all(&launch_cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

// ─── Criteria 2 + 5: --name selects a connection within the bound project ─────

#[test]
fn name_selects_connection_and_combines_with_project_path() {
    let cwd = scratch("name-cwd");
    let xdg = scratch("name-xdg");
    let good_db = xdg.join("prod.db");
    create_sqlite_db(&good_db);
    let proj = format!("/plenum-itest-named-{}", std::process::id());

    // Default ("staging") points at an unopenable path; "prod" is valid.
    // Selecting --name prod must succeed, proving the name overrides the default.
    write_global_config(
        &xdg,
        &json!({
            "projects": {
                proj.clone(): {
                    "connections": {
                        "staging": {
                            "engine": "sqlite",
                            "file": "/plenum-nonexistent-dir/staging.db"
                        },
                        "prod": { "engine": "sqlite", "file": good_db.to_str().unwrap() }
                    },
                    "default": "staging"
                }
            }
        }),
    );

    let mut child = spawn_mcp(&["--project-path", &proj, "--name", "prod"], &[], &cwd, &xdg);
    let resp = handshake_then_connect(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert_no_error(&resp);

    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&xdg);
}

// ─── Criterion 5: --dsn-env conflicts with saved-config selectors ─────────────

#[test]
fn dsn_env_conflicts_with_project_path_at_cli() {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let output = Command::new(bin)
        .args(["mcp", "--dsn-env", "PLENUM_DSN", "--project-path", "/tmp/x"])
        .stdin(Stdio::null())
        .output()
        .expect("run plenum mcp");

    assert!(!output.status.success(), "conflicting flags must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dsn-env") || stderr.contains("dsn_env"),
        "conflict error should mention --dsn-env: {stderr}"
    );
}

#[test]
fn dsn_env_conflicts_with_name_at_cli() {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let output = Command::new(bin)
        .args(["mcp", "--dsn-env", "PLENUM_DSN", "--name", "prod"])
        .stdin(Stdio::null())
        .output()
        .expect("run plenum mcp");

    assert!(!output.status.success(), "conflicting flags must be rejected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("dsn-env") || stderr.contains("dsn_env"),
        "conflict error should mention --dsn-env: {stderr}"
    );
}
