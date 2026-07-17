//! Live `MySQL` integration tests (REF-274 / REF-276).
//!
//! Every test in this file drives the compiled `plenum` binary end-to-end
//! against a real `MySQL` server, so the CLI + JSON contract is what's under
//! test, not internal APIs. The full matrix runs against both `MySQL` 8.0 and
//! `MySQL` 8.4 via the `mysql_matrix!` macro; version-specific expectations are
//! asserted explicitly, never papered over.
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
//! - `PLENUM_TEST_MYSQL_DSN`   → `MySQL` 8.0 (e.g. `mysql://plenum:plenum_pw@127.0.0.1:43306/plenum_test`)
//! - `PLENUM_TEST_MYSQL84_DSN` → `MySQL` 8.4 (e.g. `mysql://plenum:plenum_pw@127.0.0.1:43307/plenum_test`)
//!
//! When run with `--include-ignored` and a DSN var is missing, tests fail
//! fast with a clear message. They never silently skip or pass.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

const MYSQL80_DSN_VAR: &str = "PLENUM_TEST_MYSQL_DSN";
const MYSQL84_DSN_VAR: &str = "PLENUM_TEST_MYSQL84_DSN";

/// Generate one `#[test]` per `MySQL` version for a shared body function.
/// Both versions run the identical assertions; anything version-dependent
/// (e.g. the reported server version) is derived from the DSN var inside
/// the body so differences stay explicit.
macro_rules! mysql_matrix {
    ($fn80:ident, $fn84:ident, $body:ident) => {
        #[test]
        #[ignore = "requires live DB (scripts/test-live.sh)"]
        fn $fn80() {
            $body(MYSQL80_DSN_VAR, stringify!($fn80));
        }

        #[test]
        #[ignore = "requires live DB (scripts/test-live.sh)"]
        fn $fn84() {
            $body(MYSQL84_DSN_VAR, stringify!($fn84));
        }
    };
}

/// Read a required live-DB DSN from the environment, failing fast (never
/// skipping) when it is absent so `--include-ignored` runs cannot
/// false-pass without a database.
fn require_dsn(var: &str) -> String {
    match std::env::var(var) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "{var} is not set. Live MySQL tests require a running, seeded server.\n\
             Start one with scripts/test-live.sh (add --keep to iterate), or export\n\
             {var}=mysql://user:pass@host:port/db to target an existing server."
        ),
    }
}

/// The `MySQL` version each DSN var is contractually bound to (see
/// `tests/live/compose.yaml`). Asserted against the server's self-reported
/// version so a mispointed DSN cannot silently test the wrong matrix row.
fn expected_version_prefix(dsn_var: &str) -> &'static str {
    match dsn_var {
        v if v == MYSQL80_DSN_VAR => "8.0.",
        v if v == MYSQL84_DSN_VAR => "8.4.",
        other => panic!("unknown DSN var {other}"),
    }
}

/// Connection pieces recovered from a `mysql://user:pass@host:port/db` DSN,
/// used to exercise `plenum connect` (which takes explicit flags, not a DSN).
struct DsnParts {
    user: String,
    password: String,
    host: String,
    port: String,
    database: String,
}

fn parse_dsn(dsn: &str) -> DsnParts {
    let rest = dsn
        .strip_prefix("mysql://")
        .unwrap_or_else(|| panic!("expected mysql:// DSN, got {dsn:?}"));
    let (userinfo, remainder) =
        rest.split_once('@').expect("DSN must look like mysql://user:pass@host:port/db");
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
    let dir = std::env::temp_dir().join(format!("plenum_live_mysql_{tag}_{pid}_{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Spawn the compiled `plenum` binary with `args` and extra env vars,
/// HOME/XDG isolated to a scratch dir so no real user config leaks in.
/// Returns (exit code, stdout, stderr).
fn run_plenum_env(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_plenum");
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(home).env("HOME", home).env("XDG_CONFIG_HOME", home);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn plenum");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run_plenum(home: &Path, args: &[&str]) -> (i32, String) {
    let (code, stdout, _stderr) = run_plenum_env(home, args, &[]);
    (code, stdout)
}

/// Parse stdout as a single JSON envelope and assert the shared contract:
/// stdout is exactly one JSON document, with the expected `ok`, `engine`,
/// and `command` fields and the exact top-level key set required by
/// `schemas/{connect,introspect,query}_success.json` / `schemas/error_envelope.json`.
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
        Some("mysql"),
        "unexpected engine in envelope: {envelope}"
    );
    assert_eq!(
        envelope.get("command").and_then(Value::as_str),
        Some(command),
        "unexpected command in envelope: {envelope}"
    );

    let obj = envelope.as_object().expect("envelope is a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    let expected: &[&str] = if expect_ok {
        &["command", "data", "engine", "meta", "ok"]
    } else {
        &["command", "engine", "error", "meta", "ok"]
    };
    assert_eq!(keys, expected, "envelope top-level keys drifted from schema: {envelope}");

    let meta = envelope.get("meta").and_then(Value::as_object).expect("meta object");
    assert!(meta.contains_key("contract_version"), "meta missing contract_version: {envelope}");
    assert!(
        meta.get("execution_ms").is_some_and(Value::is_u64),
        "meta.execution_ms missing or not an integer: {envelope}"
    );
    if !expect_ok {
        let error = envelope.get("error").and_then(Value::as_object).expect("error object");
        assert!(
            error.get("code").is_some_and(Value::is_string)
                && error.get("message").is_some_and(Value::is_string),
            "error object must carry string code + message: {envelope}"
        );
    }
    envelope
}

fn error_code(envelope: &Value) -> &str {
    envelope.pointer("/error/code").and_then(Value::as_str).unwrap_or("<missing>")
}

/// Zero out timing metadata so envelopes can be compared for determinism.
/// Query envelopes carry timing both in `meta` and in `data` (the engine's
/// own `QueryResult.execution_ms`).
fn redact_execution_ms(envelope: &mut Value) {
    for pointer in ["/meta/execution_ms", "/data/execution_ms"] {
        if let Some(ms) = envelope.pointer_mut(pointer) {
            *ms = Value::from(0);
        }
    }
}

/// Additionally strip fields that legitimately differ between `MySQL` 8.0 and
/// 8.4 (server version strings, storage-engine row estimates) so everything
/// else can be asserted byte-identical across versions.
fn redact_version_specific(envelope: &mut Value) {
    redact_execution_ms(envelope);
    for pointer in ["/data/database_version", "/data/server_info"] {
        if let Some(field) = envelope.pointer_mut(pointer) {
            *field = Value::from("<redacted>");
        }
    }
    if let Some(estimate) = envelope.pointer_mut("/data/table/row_estimate") {
        *estimate = Value::Null;
    }
}

fn connect_test_args(parts: &DsnParts) -> Vec<&str> {
    vec![
        "connect",
        "--engine",
        "mysql",
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
    ]
}

fn query(home: &Path, dsn: &str, sql: &str) -> (i32, String) {
    run_plenum(home, &["query", "--dsn", dsn, "--sql", sql])
}

/// Run a query that must succeed and return its parsed envelope.
fn query_ok(home: &Path, dsn: &str, sql: &str) -> Value {
    let (code, stdout) = query(home, dsn, sql);
    assert_eq!(code, 0, "query {sql:?} failed, stdout={stdout}");
    assert_envelope(&stdout, true, "query")
}

/// Fetch full details for one table via `introspect --table`.
fn introspect_table(home: &Path, dsn: &str, table: &str) -> Value {
    let (code, stdout) = run_plenum(home, &["introspect", "--dsn", dsn, "--table", table]);
    assert_eq!(code, 0, "introspect --table {table} failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("table_details"),
        "unexpected introspect result type: {envelope}"
    );
    envelope
}

/// Column entry by name from a `table_details` envelope.
fn find_column<'a>(envelope: &'a Value, name: &str) -> &'a Value {
    envelope
        .pointer("/data/table/columns")
        .and_then(Value::as_array)
        .expect("table details carry columns")
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("column {name:?} missing from table details: {envelope}"))
}

// ============================================================================
// connect
// ============================================================================

/// Valid credentials via explicit flags: `connect --test` succeeds and
/// surfaces server metadata — including the `MySQL` server version, asserted
/// against the version this DSN var is bound to.
fn connect_test_reports_server_version(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    let (code, stdout, stderr) = run_plenum_env(&home, &connect_test_args(&parts), &[]);
    assert_eq!(code, 0, "connect --test failed, stdout={stdout}");
    assert!(stderr.is_empty(), "success path must not write to stderr: {stderr:?}");
    let envelope = assert_envelope(&stdout, true, "connect");

    let version = envelope
        .pointer("/data/database_version")
        .and_then(Value::as_str)
        .expect("connect data carries database_version");
    let expected_prefix = expected_version_prefix(dsn_var);
    assert!(
        version.starts_with(expected_prefix),
        "{dsn_var} must point at a MySQL {expected_prefix}x server, got {version}"
    );
    let server_info = envelope
        .pointer("/data/server_info")
        .and_then(Value::as_str)
        .expect("connect data carries server_info");
    assert!(server_info.starts_with("MySQL "), "unexpected server_info: {server_info}");
    assert_eq!(
        envelope.pointer("/data/connected_database").and_then(Value::as_str),
        Some(parts.database.as_str()),
        "unexpected connected_database: {envelope}"
    );
    // MySQL reports the authenticated account as `user@host` (CURRENT_USER()).
    let user =
        envelope.pointer("/data/user").and_then(Value::as_str).expect("connect data carries user");
    assert!(
        user.starts_with(&format!("{}@", parts.user)),
        "unexpected user (expected {}@<host>): {envelope}",
        parts.user
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_connect_test_reports_server_version,
    mysql84_connect_test_reports_server_version,
    connect_test_reports_server_version
);

/// Wrong password: normalized `CONNECTION_FAILED` error envelope on stdout,
/// non-zero exit, and the bad credential itself never leaks into the output.
fn connect_wrong_password_normalized_error(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    let bad_password = "definitely-not-the-password-598712";
    let (code, stdout) = run_plenum(
        &home,
        &[
            "connect",
            "--engine",
            "mysql",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password",
            bad_password,
            "--database",
            &parts.database,
            "--test",
        ],
    );
    assert_ne!(code, 0, "connect with a wrong password must exit non-zero, stdout={stdout}");
    let envelope = assert_envelope(&stdout, false, "connect");
    assert_eq!(error_code(&envelope), "CONNECTION_FAILED", "envelope: {envelope}");
    assert!(!stdout.contains(bad_password), "credential must never appear in output: {stdout}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_connect_wrong_password_normalized_error,
    mysql84_connect_wrong_password_normalized_error,
    connect_wrong_password_normalized_error
);

/// `--password-env`: the password is resolved from an environment variable
/// at connection time; the inline `--password` flag is not used at all.
fn connect_password_env_source(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    let (code, stdout, _stderr) = run_plenum_env(
        &home,
        &[
            "connect",
            "--engine",
            "mysql",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password-env",
            "PLENUM_LIVE_TEST_PW",
            "--database",
            &parts.database,
            "--test",
        ],
        &[("PLENUM_LIVE_TEST_PW", &parts.password)],
    );
    assert_eq!(code, 0, "connect --password-env failed, stdout={stdout}");
    assert_envelope(&stdout, true, "connect");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_connect_password_env_source,
    mysql84_connect_password_env_source,
    connect_password_env_source
);

/// `--password-command`: the password comes from the trimmed stdout of a
/// shell command run via `sh -c` at connection time.
fn connect_password_command_source(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    let password_command = format!("echo '{}'", parts.password);
    let (code, stdout) = run_plenum(
        &home,
        &[
            "connect",
            "--engine",
            "mysql",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password-command",
            &password_command,
            "--database",
            &parts.database,
            "--test",
        ],
    );
    assert_eq!(code, 0, "connect --password-command failed, stdout={stdout}");
    assert_envelope(&stdout, true, "connect");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_connect_password_command_source,
    mysql84_connect_password_command_source,
    connect_password_command_source
);

// ============================================================================
// introspect
// ============================================================================

/// `--list-tables` returns the seeded base tables (and not the view) with the
/// stable `table_list` result shape.
fn introspect_list_tables(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let (code, stdout) = run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-tables"]);
    assert_eq!(code, 0, "introspect --list-tables failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("table_list"),
        "unexpected introspect result type: {envelope}"
    );
    let tables: Vec<&str> = envelope
        .pointer("/data/tables")
        .and_then(Value::as_array)
        .expect("table_list carries tables array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for table in ["bulk_rows", "customers", "order_items", "orders", "type_matrix"] {
        assert!(tables.contains(&table), "seeded table {table:?} missing: {tables:?}");
    }
    assert!(
        !tables.contains(&"v_order_totals"),
        "views must not appear in --list-tables: {tables:?}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_list_tables,
    mysql84_introspect_list_tables,
    introspect_list_tables
);

/// `--table type_matrix` surfaces exotic column types by name: ENUM, SET,
/// JSON, and the STORED generated column.
fn introspect_type_matrix_columns(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);
    let envelope = introspect_table(&home, &dsn, "type_matrix");

    for (column, type_fragment) in [
        ("c_enum", "enum"),
        ("c_set", "set"),
        ("c_json", "json"),
        ("c_generated", "bigint"),
        ("c_decimal", "decimal"),
        ("c_varbinary", "varbinary"),
    ] {
        let data_type = find_column(&envelope, column)
            .get("data_type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("column {column:?} missing data_type: {envelope}"));
        assert!(
            data_type.to_lowercase().contains(type_fragment),
            "column {column:?}: expected data_type containing {type_fragment:?}, got {data_type:?}"
        );
    }

    assert_eq!(
        envelope.pointer("/data/table/primary_key"),
        Some(&serde_json::json!(["id"])),
        "type_matrix primary key: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_type_matrix_columns,
    mysql84_introspect_type_matrix_columns,
    introspect_type_matrix_columns
);

/// `--table orders`: composite primary key and single-column FK to customers.
fn introspect_orders_keys(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);
    let envelope = introspect_table(&home, &dsn, "orders");

    assert_eq!(
        envelope.pointer("/data/table/primary_key"),
        Some(&serde_json::json!(["customer_id", "order_no"])),
        "orders composite primary key: {envelope}"
    );

    let fks = envelope
        .pointer("/data/table/foreign_keys")
        .and_then(Value::as_array)
        .expect("orders carries foreign_keys");
    let fk = fks
        .iter()
        .find(|fk| fk.get("name").and_then(Value::as_str) == Some("fk_orders_customer"))
        .unwrap_or_else(|| panic!("fk_orders_customer missing: {envelope}"));
    assert_eq!(fk.get("columns"), Some(&serde_json::json!(["customer_id"])), "fk: {fk}");
    assert_eq!(fk.get("referenced_table").and_then(Value::as_str), Some("customers"), "fk: {fk}");
    assert_eq!(fk.get("referenced_columns"), Some(&serde_json::json!(["id"])), "fk: {fk}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_orders_keys,
    mysql84_introspect_orders_keys,
    introspect_orders_keys
);

/// `--table order_items`: COMPOSITE foreign key (`customer_id`, `order_no`) →
/// orders, plus the secondary index on sku.
fn introspect_composite_fk_and_indexes(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);
    let envelope = introspect_table(&home, &dsn, "order_items");

    let fks = envelope
        .pointer("/data/table/foreign_keys")
        .and_then(Value::as_array)
        .expect("order_items carries foreign_keys");
    let fk = fks
        .iter()
        .find(|fk| fk.get("name").and_then(Value::as_str) == Some("fk_order_items_order"))
        .unwrap_or_else(|| panic!("fk_order_items_order missing: {envelope}"));
    assert_eq!(
        fk.get("columns"),
        Some(&serde_json::json!(["customer_id", "order_no"])),
        "composite FK columns: {fk}"
    );
    assert_eq!(fk.get("referenced_table").and_then(Value::as_str), Some("orders"), "fk: {fk}");
    assert_eq!(
        fk.get("referenced_columns"),
        Some(&serde_json::json!(["customer_id", "order_no"])),
        "composite FK referenced columns: {fk}"
    );

    let indexes = envelope
        .pointer("/data/table/indexes")
        .and_then(Value::as_array)
        .expect("order_items carries indexes");
    let sku_index = indexes
        .iter()
        .find(|idx| idx.get("name").and_then(Value::as_str) == Some("idx_order_items_sku"))
        .unwrap_or_else(|| panic!("idx_order_items_sku missing: {envelope}"));
    assert_eq!(sku_index.get("columns"), Some(&serde_json::json!(["sku"])), "index: {sku_index}");
    assert_eq!(sku_index.get("unique"), Some(&Value::Bool(false)), "index: {sku_index}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_composite_fk_and_indexes,
    mysql84_introspect_composite_fk_and_indexes,
    introspect_composite_fk_and_indexes
);

/// `--list-indexes <table>` returns the index summaries for that table.
fn introspect_list_indexes(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let (code, stdout) =
        run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-indexes", "bulk_rows"]);
    assert_eq!(code, 0, "introspect --list-indexes failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("index_list"),
        "unexpected introspect result type: {envelope}"
    );
    let indexes = envelope
        .pointer("/data/indexes")
        .and_then(Value::as_array)
        .expect("index_list carries indexes");
    assert!(
        indexes
            .iter()
            .any(|idx| idx.get("name").and_then(Value::as_str) == Some("idx_bulk_rows_label")),
        "idx_bulk_rows_label missing: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_list_indexes,
    mysql84_introspect_list_indexes,
    introspect_list_indexes
);

/// `--list-views` + `--view`: the seeded view is listed and its details carry
/// a definition and columns.
fn introspect_views(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let (code, stdout) = run_plenum(&home, &["introspect", "--dsn", &dsn, "--list-views"]);
    assert_eq!(code, 0, "introspect --list-views failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("view_list"),
        "unexpected introspect result type: {envelope}"
    );
    let views: Vec<&str> = envelope
        .pointer("/data/views")
        .and_then(Value::as_array)
        .expect("view_list carries views")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(views.contains(&"v_order_totals"), "seeded view missing: {views:?}");

    let (code, stdout) =
        run_plenum(&home, &["introspect", "--dsn", &dsn, "--view", "v_order_totals"]);
    assert_eq!(code, 0, "introspect --view failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("view_details"),
        "unexpected introspect result type: {envelope}"
    );
    let definition = envelope
        .pointer("/data/view/definition")
        .and_then(Value::as_str)
        .expect("view details carry a definition");
    assert!(
        definition.to_lowercase().contains("sum"),
        "view definition should contain the aggregate: {definition:?}"
    );
    let columns: Vec<&str> = envelope
        .pointer("/data/view/columns")
        .and_then(Value::as_array)
        .expect("view details carry columns")
        .iter()
        .filter_map(|c| c.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(columns, ["customer_id", "order_no", "status", "total"], "view columns");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(mysql80_introspect_views, mysql84_introspect_views, introspect_views);

/// `--list-databases` (wildcard `--database "*"` connection — `MySQL` always
/// requires an explicit database argument) includes the seeded database.
fn introspect_list_databases(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    let (code, stdout) = run_plenum(
        &home,
        &[
            "introspect",
            "--engine",
            "mysql",
            "--host",
            &parts.host,
            "--port",
            &parts.port,
            "--user",
            &parts.user,
            "--password",
            &parts.password,
            "--database",
            "*",
            "--list-databases",
        ],
    );
    assert_eq!(code, 0, "introspect --list-databases failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "introspect");
    assert_eq!(
        envelope.pointer("/data/type").and_then(Value::as_str),
        Some("database_list"),
        "unexpected introspect result type: {envelope}"
    );
    let databases: Vec<&str> = envelope
        .pointer("/data/databases")
        .and_then(Value::as_array)
        .expect("database_list carries databases")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(databases.contains(&"plenum_test"), "seeded database missing: {databases:?}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_introspect_list_databases,
    mysql84_introspect_list_databases,
    introspect_list_databases
);

// ============================================================================
// query — allowed statements
// ============================================================================

/// Plain SELECT over seeded data: exact rows, exact order, exact meta.
fn query_select_returns_exact_rows(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let envelope = query_ok(&home, &dsn, "SELECT id, name, email FROM customers ORDER BY id");
    assert_eq!(
        envelope.pointer("/data/columns"),
        Some(&serde_json::json!(["id", "name", "email"])),
        "columns: {envelope}"
    );
    // MySQL text-protocol results serialize every value as a JSON string —
    // this locks in the current (deterministic) output contract.
    assert_eq!(
        envelope.pointer("/data/rows"),
        Some(&serde_json::json!([
            ["1", "Ada Lovelace", "ada@example.com"],
            ["2", "Grace Hopper 🌟", "grace@example.com"],
            ["3", "Annie Easley", "annie@example.com"]
        ])),
        "rows: {envelope}"
    );
    assert_eq!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64),
        Some(3),
        "rows_returned: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_query_select_returns_exact_rows,
    mysql84_query_select_returns_exact_rows,
    query_select_returns_exact_rows
);

/// EXPLAIN, SHOW, and DESCRIBE are read-only introspection statements and
/// must all execute successfully through the text protocol.
fn query_explain_show_describe_allowed(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let envelope = query_ok(&home, &dsn, "EXPLAIN SELECT id FROM customers WHERE id = 1");
    assert!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64) >= Some(1),
        "EXPLAIN returned no plan rows: {envelope}"
    );

    let envelope = query_ok(&home, &dsn, "SHOW TABLES");
    let serialized = envelope.pointer("/data/rows").map(Value::to_string).unwrap_or_default();
    assert!(serialized.contains("customers"), "SHOW TABLES missing seeded table: {envelope}");

    let envelope = query_ok(&home, &dsn, "DESCRIBE customers");
    let serialized = envelope.pointer("/data/rows").map(Value::to_string).unwrap_or_default();
    for column in ["id", "name", "email"] {
        assert!(serialized.contains(column), "DESCRIBE missing column {column:?}: {envelope}");
    }

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_query_explain_show_describe_allowed,
    mysql84_query_explain_show_describe_allowed,
    query_explain_show_describe_allowed
);

/// Transaction control statements are permitted (each invocation is its own
/// connection, so these are no-ops — but they must not be rejected).
fn query_transaction_control_allowed(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    for sql in ["BEGIN", "START TRANSACTION", "COMMIT", "ROLLBACK"] {
        let (code, stdout) = query(&home, &dsn, sql);
        assert_eq!(code, 0, "transaction control {sql:?} rejected, stdout={stdout}");
        assert_envelope(&stdout, true, "query");
    }

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_query_transaction_control_allowed,
    mysql84_query_transaction_control_allowed,
    query_transaction_control_allowed
);

// ============================================================================
// query — denied statements (capability enforcement BEFORE execution)
// ============================================================================

/// Every write/DDL statement is rejected with `CAPABILITY_VIOLATION` (the
/// capability layer, not the server — a server rejection would surface as
/// `QUERY_FAILED`), and a follow-up read proves the database is unchanged.
fn query_denied_writes_leave_state_unchanged(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    // (denied statement, probe SQL, probe assertion)
    struct DeniedCase {
        sql: &'static str,
        probe: &'static str,
        expect: fn(&Value) -> bool,
    }
    let cases = [
        DeniedCase {
            sql: "INSERT INTO customers (id, name, email) VALUES (99, 'Mallory', 'mallory@example.com')",
            probe: "SELECT COUNT(*) FROM customers",
            expect: |v| v.pointer("/data/rows/0/0") == Some(&Value::from("3")),
        },
        DeniedCase {
            sql: "UPDATE customers SET name = 'Hacked' WHERE id = 1",
            probe: "SELECT name FROM customers WHERE id = 1",
            expect: |v| v.pointer("/data/rows/0/0") == Some(&Value::from("Ada Lovelace")),
        },
        DeniedCase {
            sql: "DELETE FROM customers",
            probe: "SELECT COUNT(*) FROM customers",
            expect: |v| v.pointer("/data/rows/0/0") == Some(&Value::from("3")),
        },
        DeniedCase {
            sql: "CREATE TABLE denied_probe (id INT PRIMARY KEY)",
            probe: "SHOW TABLES LIKE 'denied_probe'",
            expect: |v| v.pointer("/meta/rows_returned") == Some(&Value::from(0)),
        },
        DeniedCase {
            sql: "DROP TABLE bulk_rows",
            probe: "SELECT COUNT(*) FROM bulk_rows",
            expect: |v| v.pointer("/data/rows/0/0") == Some(&Value::from("1500")),
        },
        DeniedCase {
            sql: "ALTER TABLE customers ADD COLUMN hacked INT",
            probe: "DESCRIBE customers",
            expect: |v| {
                !v.pointer("/data/rows").map(Value::to_string).unwrap_or_default().contains("hacked")
            },
        },
        DeniedCase {
            sql: "TRUNCATE TABLE bulk_rows",
            probe: "SELECT COUNT(*) FROM bulk_rows",
            expect: |v| v.pointer("/data/rows/0/0") == Some(&Value::from("1500")),
        },
    ];

    for case in &cases {
        let (code, stdout) = query(&home, &dsn, case.sql);
        assert_ne!(code, 0, "denied statement {:?} must exit non-zero, stdout={stdout}", case.sql);
        let envelope = assert_envelope(&stdout, false, "query");
        assert_eq!(
            error_code(&envelope),
            "CAPABILITY_VIOLATION",
            "denied statement {:?} must be blocked by the capability layer, \
             not the server: {envelope}",
            case.sql
        );

        let probe_envelope = query_ok(&home, &dsn, case.probe);
        assert!(
            (case.expect)(&probe_envelope),
            "database state changed after denied statement {:?} — probe {:?} returned {probe_envelope}",
            case.sql,
            case.probe
        );
    }

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_query_denied_writes_leave_state_unchanged,
    mysql84_query_denied_writes_leave_state_unchanged,
    query_denied_writes_leave_state_unchanged
);

// ============================================================================
// safety constraints
// ============================================================================

/// `--max-rows` truncates the 1,500-row table and reports it in meta; a cap
/// larger than the table returns everything untruncated.
fn safety_max_rows_truncation(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

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
    assert_eq!(code, 0, "query --max-rows failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64),
        Some(100),
        "rows_returned under --max-rows 100: {envelope}"
    );
    assert_eq!(
        envelope.pointer("/meta/rows_truncated"),
        Some(&Value::Bool(true)),
        "rows_truncated must be reported: {envelope}"
    );
    // truncated_by is only emitted for byte-budget truncation; row-count
    // truncation is signalled via has_more + next_offset instead.
    assert_eq!(
        envelope.pointer("/meta/truncated_by"),
        None,
        "truncated_by must be absent for row-count truncation: {envelope}"
    );
    assert_eq!(
        envelope.pointer("/meta/has_more"),
        Some(&Value::Bool(true)),
        "has_more: {envelope}"
    );
    assert_eq!(
        envelope.pointer("/meta/next_offset").and_then(Value::as_u64),
        Some(100),
        "next_offset: {envelope}"
    );
    let rows = envelope.pointer("/data/rows").and_then(Value::as_array).expect("rows array");
    assert_eq!(rows.len(), 100, "row payload must honor the cap: got {}", rows.len());
    assert_eq!(rows[0], serde_json::json!(["1", "row-0001"]), "deterministic first row");

    // Cap above table size: everything comes back, nothing is truncated.
    let (code, stdout) = run_plenum(
        &home,
        &[
            "query",
            "--dsn",
            &dsn,
            "--sql",
            "SELECT n FROM bulk_rows ORDER BY n",
            "--max-rows",
            "2000",
        ],
    );
    assert_eq!(code, 0, "query --max-rows 2000 failed, stdout={stdout}");
    let envelope = assert_envelope(&stdout, true, "query");
    assert_eq!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64),
        Some(1500),
        "full table under a generous cap: {envelope}"
    );
    assert_ne!(
        envelope.pointer("/meta/rows_truncated"),
        Some(&Value::Bool(true)),
        "no truncation may be reported under a generous cap: {envelope}"
    );

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_safety_max_rows_truncation,
    mysql84_safety_max_rows_truncation,
    safety_max_rows_truncation
);

/// `--timeout-ms` exceeded via `SLEEP()`: structured `QUERY_TIMEOUT` error that
/// names the configured budget (locks in the REF-258 timeout-as-error fix).
/// No wall-clock assertions — only the structured outcome is checked.
fn safety_timeout_structured_error(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let (code, stdout) = run_plenum(
        &home,
        &["query", "--dsn", &dsn, "--sql", "SELECT SLEEP(5)", "--timeout-ms", "500"],
    );
    assert_ne!(code, 0, "timed-out query must exit non-zero, stdout={stdout}");
    let envelope = assert_envelope(&stdout, false, "query");
    assert_eq!(error_code(&envelope), "QUERY_TIMEOUT", "envelope: {envelope}");
    let message =
        envelope.pointer("/error/message").and_then(Value::as_str).expect("error message");
    assert!(message.contains("500"), "timeout error must name the configured budget: {message:?}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_safety_timeout_structured_error,
    mysql84_safety_timeout_structured_error,
    safety_timeout_structured_error
);

// ============================================================================
// envelope contract
// ============================================================================

/// Identical inputs produce byte-identical envelopes once `execution_ms` is
/// redacted, on both the success and error paths, and stdout carries exactly
/// one JSON document with nothing else (stderr stays empty on success).
fn envelope_determinism(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let home = scratch_home(tag);

    let sql = "SELECT id, name FROM customers ORDER BY id";
    let (code_a, stdout_a, stderr_a) =
        run_plenum_env(&home, &["query", "--dsn", &dsn, "--sql", sql], &[]);
    let (code_b, stdout_b, _) = run_plenum_env(&home, &["query", "--dsn", &dsn, "--sql", sql], &[]);
    assert_eq!(code_a, 0, "query failed, stdout={stdout_a}");
    assert_eq!(code_a, code_b, "exit codes must be deterministic");
    assert!(stderr_a.is_empty(), "success path must not write to stderr: {stderr_a:?}");

    let mut envelope_a = assert_envelope(&stdout_a, true, "query");
    let mut envelope_b = assert_envelope(&stdout_b, true, "query");
    redact_execution_ms(&mut envelope_a);
    redact_execution_ms(&mut envelope_b);
    assert_eq!(envelope_a, envelope_b, "success envelope must be deterministic");

    let denied = "DELETE FROM customers";
    let (_, stdout_a) = query(&home, &dsn, denied);
    let (_, stdout_b) = query(&home, &dsn, denied);
    let mut envelope_a = assert_envelope(&stdout_a, false, "query");
    let mut envelope_b = assert_envelope(&stdout_b, false, "query");
    redact_execution_ms(&mut envelope_a);
    redact_execution_ms(&mut envelope_b);
    assert_eq!(envelope_a, envelope_b, "error envelope must be deterministic");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(mysql80_envelope_determinism, mysql84_envelope_determinism, envelope_determinism);

// ============================================================================
// cross-version matrix
// ============================================================================

/// `MySQL` 8.0 and 8.4 must behave identically for a canonical slice of the
/// surface (introspection details, query rows, capability rejection, and
/// truncation) once version strings, row estimates, and timing are redacted.
/// Requires BOTH DSN vars — a real cross-version assertion, not two copies.
#[test]
#[ignore = "requires live DB (scripts/test-live.sh)"]
fn mysql_80_and_84_behave_identically() {
    let dsn80 = require_dsn(MYSQL80_DSN_VAR);
    let dsn84 = require_dsn(MYSQL84_DSN_VAR);
    let home = scratch_home("crossver");

    let canonical_invocations: &[&[&str]] = &[
        &["introspect", "--list-tables"],
        &["introspect", "--table", "order_items"],
        &["query", "--sql", "SELECT id, name, email FROM customers ORDER BY id"],
        &["query", "--sql", "DELETE FROM customers"],
        &["query", "--sql", "SELECT n, label FROM bulk_rows ORDER BY n", "--max-rows", "10"],
    ];

    for invocation in canonical_invocations {
        let with_dsn = |dsn: &str| -> Value {
            let mut args: Vec<&str> = vec![invocation[0], "--dsn", dsn];
            args.extend_from_slice(&invocation[1..]);
            let (_, stdout, _) = run_plenum_env(&home, &args, &[]);
            let mut envelope: Value = serde_json::from_str(stdout.trim())
                .unwrap_or_else(|e| panic!("stdout is not valid JSON ({e}): {stdout:?}"));
            redact_version_specific(&mut envelope);
            envelope
        };
        let envelope80 = with_dsn(&dsn80);
        let envelope84 = with_dsn(&dsn84);
        assert_eq!(
            envelope80, envelope84,
            "MySQL 8.0 and 8.4 diverged on {invocation:?} — surface the difference \
             explicitly in a version-specific assertion instead of ignoring it"
        );
    }

    let _ = std::fs::remove_dir_all(&home);
}

// ============================================================================
// smoke (connect → introspect → select round-trip)
// ============================================================================

/// Smoke: connect → introspect → SELECT round-trips through the CLI with
/// valid JSON envelopes on every step.
fn smoke_connect_introspect_select(dsn_var: &str, tag: &str) {
    let dsn = require_dsn(dsn_var);
    let parts = parse_dsn(&dsn);
    let home = scratch_home(tag);

    // 1. connect --test: liveness + server metadata, nothing saved.
    let (code, stdout) = run_plenum(&home, &connect_test_args(&parts));
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
    let envelope = query_ok(&home, &dsn, "SELECT id, name FROM customers ORDER BY id");
    assert_eq!(
        envelope.pointer("/meta/rows_returned").and_then(Value::as_u64),
        Some(3),
        "expected the 3 seeded customers: {envelope}"
    );
    let rows = envelope.pointer("/data/rows").map(Value::to_string).unwrap_or_default();
    assert!(rows.contains("Ada Lovelace"), "seeded row missing from query result: {envelope}");

    let _ = std::fs::remove_dir_all(&home);
}
mysql_matrix!(
    mysql80_smoke_connect_introspect_select,
    mysql84_smoke_connect_introspect_select,
    smoke_connect_introspect_select
);
