//! Connection Performance Benchmarks
//!
//! Benchmarks for database connection validation operations across different engines.
//! These benchmarks measure the overhead of:
//! - Opening connections
//! - Validating connections
//! - Retrieving connection metadata

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use plenum::{ConnectionConfig, DatabaseEngine};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

#[cfg(feature = "sqlite")]
fn bench_sqlite_connection_validation(c: &mut Criterion) {
    // Create a temporary SQLite database for benchmarking
    let temp_file = std::env::temp_dir().join("bench_connection.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let _conn = Connection::open(&temp_file).expect("Failed to create database");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());

    c.bench_function("sqlite_validate_connection", |b| {
        b.iter(|| {
            let result = SqliteEngine::validate_connection(black_box(&config));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
criterion_group!(benches, bench_sqlite_connection_validation);

#[cfg(not(feature = "sqlite"))]
criterion_group!(benches,);

criterion_main!(benches);
