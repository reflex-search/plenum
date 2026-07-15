//! Live `PostgreSQL` integration tests (REF-274 / REF-275).
//!
//! Every test in this file drives the compiled `plenum` binary end-to-end
//! against a real `PostgreSQL` server, so the CLI + JSON contract is what's
//! under test, not internal APIs.
//!
//! All tests are `#[ignore]`d: plain `cargo test` needs no Docker and skips
//! them. Run them through the harness, which provisions seeded databases
//! and exports the DSN env vars:
//!
//! ```text
//! scripts/test-live.sh            # up --wait, run, tear down
//! scripts/test-live.sh --keep     # leave containers running for iteration
//! ```
//!
//! Env contract (explicit over implicit — no auto-discovery of containers):
//! - `PLENUM_TEST_POSTGRES_DSN` → `PostgreSQL` 16 (e.g. `postgres://plenum:plenum_pw@127.0.0.1:45432/plenum_test`)
//!
//! When run with `--include-ignored` and the DSN var is missing, tests fail
//! fast with a clear message. They never silently skip or pass.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const POSTGRES_DSN_VAR: &str = "PLENUM_TEST_POSTGRES_DSN";

/// Read a required live-DB DSN from the environment, failing fast (never
/// skipping) when it is absent so `--include-ignored` runs cannot
/// false-pass without a database.
fn require_dsn(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. Live PostgreSQL tests require a running, seeded server.\n\
             Start one with scripts/test-live.sh (add --keep to iterate), or export\n\
             {var}=postgres://user:pass@host:port/db to target an existing server."
        ),
    }
}

/// Connection pieces recovered from a `postgres://user:pass@host:port/db`
/// DSN, used to exercise `plenum connect` (which takes explicit flags, not
/// a DSN).
struct DsnParts {
    user: String,
    password: String,
    host: String,
    port: String,
    database: String,
}

fn parse_dsn(dsn: &str) -> DsnParts {
    let rest = dsn
        .strip_prefix("postgres://")
        .or_else(|| dsn.strip_prefix("postgresql://"))
        .unwrap_or_else(|| panic!("expected postgres:// DSN, got {dsn:?}"));
    let (userinfo, remainder) =
        rest.split_once('@').expect("DSN must look like postgres://user:pass@host:port/db");
    let (user, password) = userinfo.split_once(':').expect("DSN missing password");
    let (hostport, database) = remainder.split_once('/').expect("DSN missing database");
    let (host, port) = hostport.split_once(':').expect("DSN missing port");
    DsnParts {
        user: user.to_string(),
        password: password.to_string(),
        host: host.to_string(),
        port: port.to_string(),
        database: database.to_string(),
    }
}

fn scratch_home(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("plenum_live_postgres_{tag}_{pid}_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Spawn the compiled `plenum` binary with `args`, HOME/XDG isolated to a
/// scratch dir so no real user config leaks in. Returns (exit code, stdout).
fn run_plenum(home: &Path, args: &[&str]) -> (i32, String) {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let output = Command::new(bin)
        .args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .output()
        .expect("spawn plenum");
    (output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse stdout as a single JSON envelope and assert the shared contract:
/// valid JSON, expected `ok`, `engine`, and `command` fields.
fn assert_envelope(stdout: &str, expect_ok: bool, command: &str) -> Value {
    let envelope: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not valid JSON ({e}): {stdout:?}"));
    assert_eq!(
        envelope.get("ok").and_then(Value::as_bool),
        Some(expect_ok),
        "unexpected ok flag in envelope: {envelope}"
    );
    assert_eq!(
        envelope.get("engine").and_then(Value::as_str),
        Some("postgres"),
        "unexpected engine in envelope: {envelope}"
    );
    assert_eq!(
        envelope.get("command").and_then(Value::as_str),
        Some(command),
        "unexpected command in envelope: {envelope}"
    );
    envelope
}

/// Smoke: connect → introspect → SELECT round-trips through the CLI with
/// valid JSON envelopes on every step.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_smoke_connect_introspect_select() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let parts = parse_dsn(&dsn);
    let home = scratch_home("smoke16");

    // 1. connect --test: liveness + server metadata, nothing saved.
    let (code, stdout) = run_plenum(
        &home,
        &[
            "connect",
            "--engine",
            "postgres",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password",
            &parts.password,
            "--database",
            &parts.database,
            "--test",
        ],
    );
    assert_eq!(code, 0, "connect --test failed, stdout={stdout}");
    assert_envelope(&stdout, true, "connect");

    // 2. introspect --list-tables: seeded tables must be visible.
    let (code, stdout) = run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-tables"]);
    assert_eq!(code, 0, "introspect failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    let data = envelope.get("data").expect("introspect envelope has data");
    let serialized = data.to_string();
    for table in ["type_matrix", "customers", "orders", "order_items", "bulk_rows"] {
        assert!(
            serialized.contains(table),
            "seeded table {table:?} missing from introspect data: {serialized}"
        );
    }

    // 3. SELECT against seeded data.
    let (code, stdout) = run_plenum(
        &home,
        &["query", "--dsn", &dsn, "--sql", "SELECT id, name FROM customers ORDER BY id"],
    );
    assert_eq!(code, 0, "query failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64),
        Some(3),
        "expected the 3 seeded customers: {envelope}"
    );
    let rows = envelope.pointer("/data/rows").map(Value::to_string).unwrap_or_default();
    assert!(rows.contains("Ada Lovelace"), "seeded row missing from query result: {envelope}");

    let _ = std::fs::remove_dir_all(&home);
}
