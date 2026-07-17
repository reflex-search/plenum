//! Live `PostgreSQL` integration tests (REF-274 / REF-275 / REF-277).
//!
//! Every test in this file drives the compiled `plenum` binary end-to-end
//! against a real `PostgreSQL` server, so the CLI + JSON contract is what's
//! under test, not internal APIs.
//!
//! Full matrix (REF-277):
//! - connect: valid creds, wrong password → normalized error, `password_env`
//!   and `password_command` credential sources
//! - introspect: tables, columns + type names (arrays / JSONB / enums),
//!   composite PK/FK, indexes, views, multiple schemas
//! - query allowed: SELECT, EXPLAIN, EXPLAIN ANALYZE, transaction control
//! - query denied: writes/DDL → `CAPABILITY_VIOLATION` with DB state proven
//!   unchanged afterwards
//! - safety: `max_rows` truncation on the >1,000-row table, `timeout_ms`
//!   via `pg_sleep()`
//! - envelope: required fields from `schemas/*.json`, deterministic output
//!   with `execution_ms` redacted, JSON-only stdout
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
    run_plenum_env(home, args, &[])
}

/// Like [`run_plenum`], with extra environment variables set on the child.
/// Used by the `password_env` credential-source tests.
fn run_plenum_env(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> (i32, String) {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(home).env("HOME", home).env("XDG_CONFIG_HOME", home);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn plenum");
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

// ===== Shared helpers for the full matrix (REF-277) =====

/// Assert an error envelope carries the expected stable error code.
fn assert_error_code(envelope: &Value, code: &str) {
    assert_eq!(
        envelope.pointer("/error/code").and_then(Value::as_str),
        Some(code),
        "unexpected error code in envelope: {envelope}"
    );
    let message = envelope
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("error envelope missing message: {envelope}"));
    assert!(!message.is_empty(), "error message must be non-empty: {envelope}");
}

/// Assert `envelope` satisfies the required-field contract of a schema in
/// `schemas/`. This checks every top-level `required` key, every required
/// `Metadata` key under `meta`, and (for the error schema) every required
/// `ErrorInfo` key under `error` — a structural match against the same
/// files agents consume, without pulling in a full JSON-Schema validator.
fn assert_matches_schema(envelope: &Value, schema_file: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas").join(schema_file);
    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display())),
    )
    .unwrap_or_else(|e| panic!("{schema_file} is not valid JSON: {e}"));

    let required = schema["required"].as_array().unwrap_or_else(|| {
        panic!("{schema_file} has no top-level required array");
    });
    for key in required {
        let key = key.as_str().expect("required entries are strings");
        assert!(
            envelope.get(key).is_some(),
            "envelope missing required key {key:?} per {schema_file}: {envelope}"
        );
    }

    let meta_required = schema["definitions"]["Metadata"]["required"]
        .as_array()
        .unwrap_or_else(|| panic!("{schema_file} has no Metadata.required"));
    for key in meta_required {
        let key = key.as_str().expect("required entries are strings");
        assert!(
            envelope["meta"].get(key).is_some(),
            "meta missing required key {key:?} per {schema_file}: {envelope}"
        );
    }

    if let Some(error_required) = schema["definitions"]["ErrorInfo"]["required"].as_array() {
        for key in error_required {
            let key = key.as_str().expect("required entries are strings");
            assert!(
                envelope["error"].get(key).is_some(),
                "error missing required key {key:?} per {schema_file}: {envelope}"
            );
        }
    }
}

/// Remove the timing fields (`meta.execution_ms`, `data.execution_ms`) so two
/// otherwise-identical envelopes can be compared byte-for-byte.
fn redact_execution_ms(mut envelope: Value) -> Value {
    if let Some(meta) = envelope.get_mut("meta").and_then(Value::as_object_mut) {
        meta.remove("execution_ms");
    }
    if let Some(data) = envelope.get_mut("data").and_then(Value::as_object_mut) {
        data.remove("execution_ms");
    }
    envelope
}

/// Run a single-value scalar query (e.g. `SELECT count(*) …`) and return the
/// lone cell as a JSON value.
fn scalar_query(home: &Path, dsn: &str, sql: &str) -> Value {
    let (code, stdout) = run_plenum(home, &["query", "--dsn", dsn, "--sql", sql]);
    assert_eq!(code, 0, "scalar query {sql:?} failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    envelope
        .pointer("/data/rows/0/0")
        .cloned()
        .unwrap_or_else(|| panic!("scalar query {sql:?} returned no cell: {envelope}"))
}

/// Fetch full details for a table via `introspect --table`, asserting the
/// success envelope, and return the `data.table` object.
fn introspect_table(home: &Path, dsn: &str, extra: &[&str], table: &str) -> Value {
    let mut args = vec!["introspect", "--dsn", dsn];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--table", table]);
    let (code, stdout) = run_plenum(home, &args);
    assert_eq!(code, 0, "introspect --table {table} failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_matches_schema(&envelope, "introspect_success.json");
    envelope
        .pointer("/data/table")
        .cloned()
        .unwrap_or_else(|| panic!("introspect --table {table} returned no data.table: {envelope}"))
}

/// Find a column by name in a `data.table.columns` array.
fn column<'a>(table: &'a Value, name: &str) -> &'a Value {
    table["columns"]
        .as_array()
        .expect("table has columns array")
        .iter()
        .find(|c| c["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("column {name:?} not found in table: {table}"))
}

/// Collect string arrays like `primary_key`, `columns`, `referenced_columns`.
fn string_vec(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected string array, got: {value}"))
        .iter()
        .map(|v| v.as_str().expect("array of strings").to_string())
        .collect()
}

// ===== connect =====

/// `connect --test` with valid credentials returns server metadata matching
/// the `connect_success.json` contract.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_connect_test_reports_server_metadata() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let parts = parse_dsn(&dsn);
    let home = scratch_home("connect_meta");

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
    let envelope = assert_envelope(&stdout, true, "connect");
    assert_matches_schema(&envelope, "connect_success.json");

    assert_eq!(
        envelope.pointer("/data/connected_database").and_then(Value::as_str),
        Some(parts.database.as_str()),
        "connected_database mismatch: {envelope}"
    );
    assert_eq!(
        envelope.pointer("/data/user").and_then(Value::as_str),
        Some(parts.user.as_str()),
        "user mismatch: {envelope}"
    );
    let version = envelope
        .pointer("/data/database_version")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing database_version: {envelope}"));
    assert!(version.contains("16"), "expected a PostgreSQL 16 version string, got {version:?}");

    let _ = std::fs::remove_dir_all(&home);
}

/// A wrong password yields a normalized `CONNECTION_FAILED` error envelope
/// that never echoes the credential back.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_connect_wrong_password_normalized_error() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let parts = parse_dsn(&dsn);
    let home = scratch_home("badpw");
    let wrong_password = "definitely-not-the-password-5x9";

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
            wrong_password,
            "--database",
            &parts.database,
            "--test",
        ],
    );
    assert_ne!(code, 0, "connect --test with a wrong password must fail, stdout={stdout}");
    let envelope = assert_envelope(&stdout, false, "connect");
    assert_matches_schema(&envelope, "error_envelope.json");
    assert_error_code(&envelope, "CONNECTION_FAILED");
    assert!(
        !stdout.contains(wrong_password),
        "error output must not echo the password back: {stdout}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// `password_env`: the saved connection stores only the env var name; the
/// password resolves from the environment at query time and querying without
/// the variable set fails with a structured error.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_connect_password_env_source() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let parts = parse_dsn(&dsn);
    let home = scratch_home("pwenv");
    const PW_VAR: &str = "PLENUM_LIVE_TEST_PG_PASSWORD";

    let (code, stdout) = run_plenum_env(
        &home,
        &[
            "connect",
            "--name",
            "envconn",
            "--engine",
            "postgres",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password-env",
            PW_VAR,
            "--database",
            &parts.database,
            "--save",
            "local",
        ],
        &[(PW_VAR, &parts.password)],
    );
    assert_eq!(code, 0, "connect --save with --password-env failed, stdout={stdout}");
    assert_envelope(&stdout, true, "connect");

    // The stored config must reference the env var, never the plaintext password.
    let config = std::fs::read_to_string(home.join(".plenum").join("config.json"))
        .expect("local config written by connect --save local");
    assert!(config.contains(PW_VAR), "saved config must record the env var name: {config}");
    assert!(
        !config.contains(&parts.password),
        "saved config must not contain the plaintext password: {config}"
    );

    // Query resolves the password from the environment at use time.
    let (code, stdout) = run_plenum_env(
        &home,
        &["query", "--name", "envconn", "--sql", "SELECT count(*) FROM customers"],
        &[(PW_VAR, &parts.password)],
    );
    assert_eq!(code, 0, "query via password_env connection failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/data/rows/0/0").and_then(Value::as_i64),
        Some(3),
        "expected the 3 seeded customers: {envelope}"
    );

    // Without the env var the query must fail fast with a structured error.
    let (code, stdout) = run_plenum(&home, &["query", "--name", "envconn", "--sql", "SELECT 1"]);
    assert_ne!(code, 0, "query without the password env var must fail, stdout={stdout}");
    let envelope: Value =
        serde_json::from_str(stdout.trim()).expect("error output is a JSON envelope");
    assert_eq!(envelope["ok"].as_bool(), Some(false), "expected error envelope: {envelope}");
    assert_error_code(&envelope, "CONFIG_ERROR");

    let _ = std::fs::remove_dir_all(&home);
}

/// `password_command`: the saved connection runs a shell command at use time
/// and uses its stdout as the password; the plaintext never lands on disk.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_connect_password_command_source() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let parts = parse_dsn(&dsn);
    let home = scratch_home("pwcmd");

    let pw_file = home.join("pg_password.txt");
    std::fs::write(&pw_file, &parts.password).expect("write password file");
    let command = format!("cat {}", pw_file.display());

    let (code, stdout) = run_plenum(
        &home,
        &[
            "connect",
            "--name",
            "cmdconn",
            "--engine",
            "postgres",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password-command",
            &command,
            "--database",
            &parts.database,
            "--save",
            "local",
        ],
    );
    assert_eq!(code, 0, "connect --save with --password-command failed, stdout={stdout}");
    assert_envelope(&stdout, true, "connect");

    let config = std::fs::read_to_string(home.join(".plenum").join("config.json"))
        .expect("local config written by connect --save local");
    assert!(
        !config.contains(&parts.password),
        "saved config must not contain the plaintext password: {config}"
    );

    let (code, stdout) =
        run_plenum(&home, &["query", "--name", "cmdconn", "--sql", "SELECT count(*) FROM orders"]);
    assert_eq!(code, 0, "query via password_command connection failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/data/rows/0/0").and_then(Value::as_i64),
        Some(3),
        "expected the 3 seeded orders: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

// ===== introspect =====

/// Column type names on the seeded `type_matrix` table, including the
/// PostgreSQL-specific families: arrays, JSONB, and enum types.
///
/// The engine reports `information_schema.columns.data_type` verbatim, so
/// arrays surface as `ARRAY` and enum columns as `USER-DEFINED`. These
/// assertions pin that observed contract; improving element-type fidelity
/// is tracked separately.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_introspect_type_matrix_columns() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("types");

    let table = introspect_table(&home, &dsn, &[], "type_matrix");
    assert_eq!(table["name"].as_str(), Some("type_matrix"));
    assert_eq!(table["schema"].as_str(), Some("public"));
    assert_eq!(
        string_vec(&table["primary_key"]),
        vec!["id".to_string()],
        "type_matrix primary key: {table}"
    );

    let expected_types = [
        ("c_smallint", "smallint"),
        ("c_integer", "integer"),
        ("c_bigint", "bigint"),
        ("c_numeric", "numeric"),
        ("c_real", "real"),
        ("c_double", "double precision"),
        ("c_varchar", "character varying"),
        ("c_text", "text"),
        ("c_date", "date"),
        ("c_time", "time without time zone"),
        ("c_timestamp", "timestamp without time zone"),
        ("c_timestamptz", "timestamp with time zone"),
        ("c_bytea", "bytea"),
        ("c_bool", "boolean"),
        ("c_mood", "USER-DEFINED"),
        ("c_tags", "ARRAY"),
        ("c_matrix", "ARRAY"),
        ("c_jsonb", "jsonb"),
        ("c_json", "json"),
    ];
    for (name, data_type) in expected_types {
        let col = column(&table, name);
        assert_eq!(
            col["data_type"].as_str(),
            Some(data_type),
            "unexpected data_type for column {name}: {col}"
        );
        assert_eq!(col["nullable"].as_bool(), Some(true), "{name} should be nullable: {col}");
    }

    let id = column(&table, "id");
    assert_eq!(id["data_type"].as_str(), Some("integer"), "id column: {id}");
    assert_eq!(id["nullable"].as_bool(), Some(false), "identity PK is NOT NULL: {id}");

    let _ = std::fs::remove_dir_all(&home);
}

/// Composite primary keys and composite foreign keys survive introspection
/// with column order intact.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_introspect_composite_pk_and_fk() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("compfk");

    // Single-column FK: orders → customers.
    let orders = introspect_table(&home, &dsn, &[], "orders");
    assert_eq!(
        string_vec(&orders["primary_key"]),
        vec!["customer_id".to_string(), "order_no".to_string()],
        "orders composite PK: {orders}"
    );
    let orders_fks = orders["foreign_keys"].as_array().expect("orders has foreign_keys");
    let fk_customer = orders_fks
        .iter()
        .find(|fk| fk["name"].as_str() == Some("fk_orders_customer"))
        .unwrap_or_else(|| panic!("fk_orders_customer missing: {orders}"));
    assert_eq!(string_vec(&fk_customer["columns"]), vec!["customer_id".to_string()]);
    assert_eq!(fk_customer["referenced_table"].as_str(), Some("customers"));
    assert_eq!(string_vec(&fk_customer["referenced_columns"]), vec!["id".to_string()]);

    // Composite FK: order_items → orders on (customer_id, order_no).
    let items = introspect_table(&home, &dsn, &[], "order_items");
    assert_eq!(
        string_vec(&items["primary_key"]),
        vec!["customer_id".to_string(), "order_no".to_string(), "line_no".to_string()],
        "order_items composite PK: {items}"
    );
    let item_fks = items["foreign_keys"].as_array().expect("order_items has foreign_keys");
    let fk_order = item_fks
        .iter()
        .find(|fk| fk["name"].as_str() == Some("fk_order_items_order"))
        .unwrap_or_else(|| panic!("fk_order_items_order missing: {items}"));
    assert_eq!(
        string_vec(&fk_order["columns"]),
        vec!["customer_id".to_string(), "order_no".to_string()],
        "composite FK columns: {items}"
    );
    assert_eq!(fk_order["referenced_table"].as_str(), Some("orders"));
    assert_eq!(
        string_vec(&fk_order["referenced_columns"]),
        vec!["customer_id".to_string(), "order_no".to_string()],
        "composite FK referenced columns: {items}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// Indexes: non-unique secondary indexes and unique constraints both appear,
/// via table details and via `--list-indexes`.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_introspect_indexes() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("indexes");

    let items = introspect_table(&home, &dsn, &[], "order_items");
    let indexes = items["indexes"].as_array().expect("order_items has indexes");
    let sku_idx = indexes
        .iter()
        .find(|ix| ix["name"].as_str() == Some("idx_order_items_sku"))
        .unwrap_or_else(|| panic!("idx_order_items_sku missing: {items}"));
    assert_eq!(sku_idx["unique"].as_bool(), Some(false), "sku index is non-unique: {sku_idx}");
    assert_eq!(string_vec(&sku_idx["columns"]), vec!["sku".to_string()]);

    let customers = introspect_table(&home, &dsn, &[], "customers");
    let indexes = customers["indexes"].as_array().expect("customers has indexes");
    let unique_email = indexes
        .iter()
        .find(|ix| ix["name"].as_str() == Some("uq_customers_email"))
        .unwrap_or_else(|| panic!("uq_customers_email missing: {customers}"));
    assert_eq!(unique_email["unique"].as_bool(), Some(true), "email index: {unique_email}");

    // --list-indexes filtered by table returns the same index with its table.
    let (code, stdout) =
        run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-indexes", "order_items"]);
    assert_eq!(code, 0, "introspect --list-indexes failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_matches_schema(&envelope, "introspect_success.json");
    let listed = envelope.pointer("/data/indexes").and_then(Value::as_array).cloned().unwrap();
    assert!(
        listed.iter().any(|ix| ix["name"].as_str() == Some("idx_order_items_sku")
            && ix["table"].as_str() == Some("order_items")),
        "idx_order_items_sku missing from --list-indexes: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// Views: name listing and per-view details (columns + SQL definition).
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_introspect_views() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("views");

    let (code, stdout) = run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-views"]);
    assert_eq!(code, 0, "introspect --list-views failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_matches_schema(&envelope, "introspect_success.json");
    let views = string_vec(&envelope["data"]["views"]);
    assert!(views.contains(&"v_order_totals".to_string()), "views listed: {views:?}");

    let (code, stdout) =
        run_plenum(&home, &["introspect", "--dsn", &dsn, "--view", "v_order_totals"]);
    assert_eq!(code, 0, "introspect --view failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    let view = &envelope["data"]["view"];
    assert_eq!(view["name"].as_str(), Some("v_order_totals"));
    let col_names: Vec<String> = view["columns"]
        .as_array()
        .expect("view has columns")
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        col_names,
        vec!["customer_id", "order_no", "status", "total"],
        "view columns: {view}"
    );
    let definition = view["definition"].as_str().unwrap_or_default();
    assert!(
        definition.to_lowercase().contains("order_items"),
        "view definition should reference order_items: {view}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// Multiple schemas: `--list-schemas` sees both, and `--schema` scopes table
/// listing and table details to the non-default schema.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_introspect_multiple_schemas() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("schemas");

    let (code, stdout) = run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-schemas"]);
    assert_eq!(code, 0, "introspect --list-schemas failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_matches_schema(&envelope, "introspect_success.json");
    let schemas = string_vec(&envelope["data"]["schemas"]);
    for expected in ["public", "analytics"] {
        assert!(schemas.contains(&expected.to_string()), "schemas listed: {schemas:?}");
    }

    let (code, stdout) =
        run_plenum(&home, &["introspect", "--dsn", &dsn, "--schema", "analytics", "--list-tables"]);
    assert_eq!(code, 0, "introspect --schema analytics --list-tables failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    let tables = string_vec(&envelope["data"]["tables"]);
    assert_eq!(tables, vec!["page_views".to_string()], "analytics tables: {tables:?}");

    let page_views = introspect_table(&home, &dsn, &["--schema", "analytics"], "page_views");
    assert_eq!(page_views["schema"].as_str(), Some("analytics"), "table: {page_views}");
    let meta_col = column(&page_views, "meta");
    assert_eq!(meta_col["data_type"].as_str(), Some("jsonb"), "meta column: {meta_col}");

    let _ = std::fs::remove_dir_all(&home);
}

// ===== query: allowed operations =====

/// SELECT round-trips seeded values — unicode/emoji strings, booleans, JSONB,
/// and an all-NULL row — with the column list intact.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_select_seeded_values() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("select");

    let (code, stdout) = run_plenum(
        &home,
        &[
            "query",
            "--dsn",
            &dsn,
            "--sql",
            "SELECT c_varchar, c_bool, c_mood::text AS c_mood, c_jsonb, c_tags::text AS c_tags \
             FROM type_matrix ORDER BY id",
        ],
    );
    assert_eq!(code, 0, "query failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_matches_schema(&envelope, "query_success.json");

    assert_eq!(
        string_vec(&envelope["data"]["columns"]),
        vec!["c_varchar", "c_bool", "c_mood", "c_jsonb", "c_tags"],
        "column names: {envelope}"
    );
    assert_eq!(envelope.pointer("/meta/rows_returned").and_then(Value::as_u64), Some(3));

    let rows = envelope["data"]["rows"].as_array().expect("rows array");
    assert_eq!(rows[0][0].as_str(), Some("café résumé 🚀"), "unicode round-trip: {rows:?}");
    assert_eq!(rows[0][1].as_bool(), Some(true));
    assert_eq!(rows[0][2].as_str(), Some("happy"), "enum value: {rows:?}");
    assert!(
        rows[0][3].to_string().contains("demo"),
        "jsonb round-trip should carry seeded content: {rows:?}"
    );
    assert!(
        rows[0][4].as_str().unwrap_or_default().contains("green 🌿"),
        "array round-trip: {rows:?}"
    );
    // Row 3 is the all-NULL row.
    for (idx, cell) in rows[2].as_array().expect("row array").iter().enumerate() {
        assert!(cell.is_null(), "row 3 column {idx} should be NULL: {rows:?}");
    }

    let _ = std::fs::remove_dir_all(&home);
}

/// EXPLAIN and EXPLAIN ANALYZE are permitted read operations that return
/// plan rows.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_explain_and_explain_analyze() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("explain");

    for sql in [
        "EXPLAIN SELECT * FROM customers WHERE id = 1",
        "EXPLAIN ANALYZE SELECT count(*) FROM order_items",
    ] {
        let (code, stdout) = run_plenum(&home, &["query", "--dsn", &dsn, "--sql", sql]);
        assert_eq!(code, 0, "{sql:?} failed, stdout={stdout}");
        let envelope = assert_envelope(&stdout, true, "query");
        let rows = envelope["data"]["rows"].as_array().expect("rows array");
        assert!(!rows.is_empty(), "{sql:?} should return plan rows: {envelope}");
    }

    let _ = std::fs::remove_dir_all(&home);
}

/// Transaction control statements are permitted (each invocation is its own
/// connection, so these are no-ops — but they must not be rejected).
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_transaction_control_allowed() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("txn");

    for sql in ["BEGIN", "COMMIT", "ROLLBACK"] {
        let (code, stdout) = run_plenum(&home, &["query", "--dsn", &dsn, "--sql", sql]);
        assert_eq!(code, 0, "{sql:?} should be permitted, stdout={stdout}");
        assert_envelope(&stdout, true, "query");
    }

    let _ = std::fs::remove_dir_all(&home);
}

// ===== query: denied operations =====

/// Every write/DDL statement class is rejected with `CAPABILITY_VIOLATION`
/// before execution, and the database state is proven unchanged afterwards.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_denied_writes_leave_state_unchanged() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("denied");

    let customer_count_sql = "SELECT count(*) FROM customers";
    let order_count_sql = "SELECT count(*) FROM orders";
    let first_customer_sql = "SELECT name FROM customers WHERE id = 1";
    let table_list = |home: &Path| -> Value {
        let (code, stdout) = run_plenum(home, &["introspect", "--dsn", &dsn, "--list-tables"]);
        assert_eq!(code, 0, "introspect failed, stdout={stdout}");
        assert_envelope(&stdout, true, "introspect")["data"].clone()
    };

    let customers_before = scalar_query(&home, &dsn, customer_count_sql);
    let orders_before = scalar_query(&home, &dsn, order_count_sql);
    let first_customer_before = scalar_query(&home, &dsn, first_customer_sql);
    let tables_before = table_list(&home);

    let denied_statements = [
        "INSERT INTO customers (id, name, email) VALUES (99, 'Mallory', 'mallory@example.com')",
        "UPDATE customers SET name = 'Hacked' WHERE id = 1",
        "DELETE FROM customers WHERE id = 1",
        "TRUNCATE TABLE order_items",
        "CREATE TABLE plenum_should_not_exist (id integer)",
        "DROP TABLE customers",
        "ALTER TABLE customers ADD COLUMN hacked integer",
    ];
    for sql in denied_statements {
        let (code, stdout) = run_plenum(&home, &["query", "--dsn", &dsn, "--sql", sql]);
        assert_ne!(code, 0, "{sql:?} must be rejected, stdout={stdout}");
        let envelope = assert_envelope(&stdout, false, "query");
        assert_matches_schema(&envelope, "error_envelope.json");
        assert_error_code(&envelope, "CAPABILITY_VIOLATION");
    }

    // Re-query: nothing changed.
    assert_eq!(
        scalar_query(&home, &dsn, customer_count_sql),
        customers_before,
        "customer count changed after denied writes"
    );
    assert_eq!(
        scalar_query(&home, &dsn, order_count_sql),
        orders_before,
        "order count changed after denied writes"
    );
    assert_eq!(
        scalar_query(&home, &dsn, first_customer_sql),
        first_customer_before,
        "customer row changed after denied UPDATE"
    );
    let tables_after = table_list(&home);
    assert_eq!(tables_before, tables_after, "table list changed after denied DDL");
    assert!(
        !tables_after.to_string().contains("plenum_should_not_exist"),
        "denied CREATE TABLE leaked a table: {tables_after}"
    );
    // TRUNCATE proof: order_items still has its 4 seeded rows.
    assert_eq!(
        scalar_query(&home, &dsn, "SELECT count(*) FROM order_items").as_i64(),
        Some(4),
        "order_items row count changed after denied TRUNCATE"
    );

    let _ = std::fs::remove_dir_all(&home);
}

// ===== safety constraints =====

/// `max_rows` truncates the >1,000-row table at a row boundary and signals
/// truncation + pagination metadata.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_max_rows_truncation() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("maxrows");

    // The seed guarantees >1,000 rows.
    assert_eq!(
        scalar_query(&home, &dsn, "SELECT count(*) FROM bulk_rows").as_i64(),
        Some(1500),
        "bulk_rows seed size"
    );

    let (code, stdout) = run_plenum(
        &home,
        &[
            "query",
            "--dsn",
            &dsn,
            "--sql",
            "SELECT n, label FROM bulk_rows ORDER BY n",
            "--max-rows",
            "100",
        ],
    );
    assert_eq!(code, 0, "max_rows query failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_matches_schema(&envelope, "query_success.json");

    let rows = envelope["data"]["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 100, "rows must be capped at max_rows: {envelope}");
    assert_eq!(envelope.pointer("/meta/rows_returned").and_then(Value::as_u64), Some(100));
    assert_eq!(envelope.pointer("/meta/rows_truncated").and_then(Value::as_bool), Some(true));
    assert_eq!(envelope.pointer("/meta/has_more").and_then(Value::as_bool), Some(true));
    assert_eq!(envelope.pointer("/meta/next_offset").and_then(Value::as_u64), Some(100));
    assert_eq!(rows[0][0].as_i64(), Some(1), "first page starts at n=1: {envelope}");

    // Pagination continues deterministically from next_offset.
    let (code, stdout) = run_plenum(
        &home,
        &[
            "query",
            "--dsn",
            &dsn,
            "--sql",
            "SELECT n, label FROM bulk_rows ORDER BY n",
            "--max-rows",
            "100",
            "--offset",
            "100",
        ],
    );
    assert_eq!(code, 0, "offset query failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/data/rows/0/0").and_then(Value::as_i64),
        Some(101),
        "second page starts at n=101: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}

/// `timeout_ms` exceeded via `pg_sleep()` surfaces a structured
/// `QUERY_TIMEOUT` error, not a hang or a driver panic.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_query_timeout_via_pg_sleep() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("timeout");

    let (code, stdout) = run_plenum(
        &home,
        &["query", "--dsn", &dsn, "--sql", "SELECT pg_sleep(5)", "--timeout-ms", "300"],
    );
    assert_ne!(code, 0, "pg_sleep past the timeout must fail, stdout={stdout}");
    let envelope = assert_envelope(&stdout, false, "query");
    assert_matches_schema(&envelope, "error_envelope.json");
    assert_error_code(&envelope, "QUERY_TIMEOUT");

    let _ = std::fs::remove_dir_all(&home);
}

// ===== envelope contract =====

/// Identical inputs produce identical outputs once `execution_ms` is
/// redacted, for both query and introspect, and stdout parses as exactly
/// one JSON document (no logs, no diagnostics).
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn postgres16_envelope_determinism_and_json_only_stdout() {
    let dsn = require_dsn(POSTGRES_DSN_VAR);
    let home = scratch_home("determinism");

    let query_args =
        ["query", "--dsn", &dsn, "--sql", "SELECT id, name, email FROM customers ORDER BY id"];
    let introspect_args = ["introspect", "--dsn", &dsn, "--table", "orders"];

    for args in [&query_args[..], &introspect_args[..]] {
        let (code_a, stdout_a) = run_plenum(&home, args);
        let (code_b, stdout_b) = run_plenum(&home, args);
        assert_eq!(code_a, 0, "first run failed, stdout={stdout_a}");
        assert_eq!(code_b, 0, "second run failed, stdout={stdout_b}");

        // Stdout is a single JSON document: parsing the entire stream as one
        // value succeeds only when nothing else is interleaved.
        let envelope_a: Value = serde_json::from_str(stdout_a.trim()).expect("stdout is JSON only");
        let envelope_b: Value = serde_json::from_str(stdout_b.trim()).expect("stdout is JSON only");

        assert_eq!(
            redact_execution_ms(envelope_a),
            redact_execution_ms(envelope_b),
            "identical inputs must produce identical outputs (execution_ms redacted) for {args:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&home);
}
