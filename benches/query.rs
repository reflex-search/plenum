//! Query Execution Performance Benchmarks
//!
//! Benchmarks for SQL query execution operations.
//! These benchmarks measure the performance of:
//! - Simple SELECT queries
//! - Queries with WHERE clauses
//! - Queries with JOINs
//! - INSERT operations
//! - Large result set handling

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use plenum::{Capabilities, ConnectionConfig, DatabaseEngine};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

#[cfg(feature = "sqlite")]
fn bench_sqlite_simple_select(c: &mut Criterion) {
    // Create a test database with sample data
    let temp_file = std::env::temp_dir().join("bench_query_simple.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)", [])
            .expect("Failed to create table");

        for i in 1..=100 {
            conn.execute(
                "INSERT INTO users (name, age) VALUES (?, ?)",
                [format!("User {i}"), i.to_string()],
            )
            .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_select_all", |b| {
        b.iter(|| {
            let result = runtime.block_on(SqliteEngine::execute(
                black_box(&config),
                black_box("SELECT * FROM users"),
                black_box(&caps),
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
fn bench_sqlite_filtered_select(c: &mut Criterion) {
    let temp_file = std::env::temp_dir().join("bench_query_filtered.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)", [])
            .expect("Failed to create table");

        for i in 1..=1000 {
            conn.execute(
                "INSERT INTO users (name, age) VALUES (?, ?)",
                [format!("User {i}"), (i % 100).to_string()],
            )
            .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_select_where", |b| {
        b.iter(|| {
            let result = runtime.block_on(SqliteEngine::execute(
                black_box(&config),
                black_box("SELECT * FROM users WHERE age > 50"),
                black_box(&caps),
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
fn bench_sqlite_insert(c: &mut Criterion) {
    let temp_file = std::env::temp_dir().join("bench_query_insert.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");
        conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)", [])
            .expect("Failed to create table");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::with_write();

    let mut counter = 0;

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_insert_single", |b| {
        b.iter(|| {
            counter += 1;
            let sql = format!(
                "INSERT INTO users (name, age) VALUES ('User {}', {})",
                counter,
                counter % 100
            );
            let result = runtime.block_on(SqliteEngine::execute(
                black_box(&config),
                black_box(&sql),
                black_box(&caps),
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
fn bench_sqlite_large_result_set(c: &mut Criterion) {
    let temp_file = std::env::temp_dir().join("bench_query_large.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");
        conn.execute("CREATE TABLE large_table (id INTEGER PRIMARY KEY, value TEXT)", [])
            .expect("Failed to create table");

        for i in 1..=10000 {
            conn.execute("INSERT INTO large_table (value) VALUES (?)", [format!("Value {i}")])
                .expect("Failed to insert");
        }
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());
    let caps = Capabilities::read_only();

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_select_10000_rows", |b| {
        b.iter(|| {
            let result = runtime.block_on(SqliteEngine::execute(
                black_box(&config),
                black_box("SELECT * FROM large_table"),
                black_box(&caps),
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
criterion_group!(
    benches,
    bench_sqlite_simple_select,
    bench_sqlite_filtered_select,
    bench_sqlite_insert,
    bench_sqlite_large_result_set
);

#[cfg(not(feature = "sqlite"))]
criterion_group!(benches,);

criterion_main!(benches);
