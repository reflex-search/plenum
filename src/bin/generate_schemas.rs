//! Generates and writes JSON Schema files for all Plenum output envelopes.
//!
//! Run with: `cargo run --bin generate-schemas`
//!
//! Output files are written to `schemas/` relative to the workspace root.
//! Run this whenever output types change to keep the checked-in schemas in sync.
//! The drift test in `tests/schema_drift.rs` fails if schemas are stale.

use plenum::{ConnectionInfo, ErrorEnvelope, IntrospectResult, QueryResult, SuccessEnvelope};
use schemars::schema_for;
use std::fs;

fn main() {
    let schemas: &[(&str, schemars::schema::RootSchema)] = &[
        ("schemas/error_envelope.json", schema_for!(ErrorEnvelope)),
        ("schemas/connect_success.json", schema_for!(SuccessEnvelope<ConnectionInfo>)),
        ("schemas/introspect_success.json", schema_for!(SuccessEnvelope<IntrospectResult>)),
        ("schemas/query_success.json", schema_for!(SuccessEnvelope<QueryResult>)),
    ];

    for (path, schema) in schemas {
        let json = serde_json::to_string_pretty(schema).expect("schema serialization");
        fs::write(path, format!("{json}\n")).unwrap_or_else(|e| panic!("write {path}: {e}"));
        println!("Generated {path}");
    }
}
