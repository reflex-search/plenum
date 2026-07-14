//! Drift test: fails if the checked-in JSON Schema files differ from what
//! schemars generates from the current Rust types.
//!
//! When this test fails, run `cargo run --bin generate-schemas` to regenerate.

use plenum::{ConnectionInfo, ErrorEnvelope, IntrospectResult, QueryResult, SuccessEnvelope};
use schemars::schema_for;

fn expected_schema(schema: &schemars::schema::RootSchema) -> String {
    format!("{}\n", serde_json::to_string_pretty(schema).expect("serialize schema"))
}

fn on_disk(filename: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas").join(filename);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("Schema file missing: {path:?}\nRun: cargo run --bin generate-schemas"))
}

#[test]
fn error_envelope_schema_not_stale() {
    let generated = expected_schema(&schema_for!(ErrorEnvelope));
    let on_disk = on_disk("error_envelope.json");
    assert_eq!(
        on_disk, generated,
        "schemas/error_envelope.json is stale — run: cargo run --bin generate-schemas"
    );
}

#[test]
fn connect_success_schema_not_stale() {
    let generated = expected_schema(&schema_for!(SuccessEnvelope<ConnectionInfo>));
    let on_disk = on_disk("connect_success.json");
    assert_eq!(
        on_disk, generated,
        "schemas/connect_success.json is stale — run: cargo run --bin generate-schemas"
    );
}

#[test]
fn introspect_success_schema_not_stale() {
    let generated = expected_schema(&schema_for!(SuccessEnvelope<IntrospectResult>));
    let on_disk = on_disk("introspect_success.json");
    assert_eq!(
        on_disk, generated,
        "schemas/introspect_success.json is stale — run: cargo run --bin generate-schemas"
    );
}

#[test]
fn query_success_schema_not_stale() {
    let generated = expected_schema(&schema_for!(SuccessEnvelope<QueryResult>));
    let on_disk = on_disk("query_success.json");
    assert_eq!(
        on_disk, generated,
        "schemas/query_success.json is stale — run: cargo run --bin generate-schemas"
    );
}
