//! Schema Introspection Performance Benchmarks
//!
//! Benchmarks for database schema introspection operations.
//! These benchmarks measure the performance of:
//! - Table discovery
//! - Column introspection
//! - Primary key detection
//! - Foreign key detection
//! - Index introspection

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use plenum::{ConnectionConfig, DatabaseEngine};

#[cfg(feature = "sqlite")]
use plenum::engine::sqlite::SqliteEngine;

#[cfg(feature = "sqlite")]
fn bench_sqlite_introspection_simple(c: &mut Criterion) {
    // Create a test database with a single table
    let temp_file = std::env::temp_dir().join("bench_introspect_simple.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");
        conn.execute(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                email TEXT
            )",
            [],
        )
        .expect("Failed to create table");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_introspect_single_table", |b| {
        b.iter(|| {
            use plenum::engine::IntrospectOperation;
            let result = runtime.block_on(SqliteEngine::introspect(
                black_box(&config),
                &IntrospectOperation::ListTables,
                None,
                None,
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
fn bench_sqlite_introspection_complex(c: &mut Criterion) {
    // Create a test database with multiple tables, foreign keys, and indexes
    let temp_file = std::env::temp_dir().join("bench_introspect_complex.db");
    let _ = std::fs::remove_file(&temp_file);

    {
        use rusqlite::Connection;
        let conn = Connection::open(&temp_file).expect("Failed to create database");

        // Create multiple tables with relationships
        conn.execute(
            "CREATE TABLE users (
                id INTEGER PRIMARY KEY,
                username TEXT NOT NULL UNIQUE,
                email TEXT NOT NULL
            )",
            [],
        )
        .expect("Failed to create users table");

        conn.execute(
            "CREATE TABLE posts (
                id INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                title TEXT NOT NULL,
                content TEXT,
                FOREIGN KEY (user_id) REFERENCES users(id)
            )",
            [],
        )
        .expect("Failed to create posts table");

        conn.execute(
            "CREATE TABLE comments (
                id INTEGER PRIMARY KEY,
                post_id INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                text TEXT NOT NULL,
                FOREIGN KEY (post_id) REFERENCES posts(id),
                FOREIGN KEY (user_id) REFERENCES users(id)
            )",
            [],
        )
        .expect("Failed to create comments table");

        // Create indexes
        conn.execute("CREATE INDEX idx_posts_user_id ON posts(user_id)", [])
            .expect("Failed to create index");
        conn.execute("CREATE INDEX idx_comments_post_id ON comments(post_id)", [])
            .expect("Failed to create index");
    }

    let config = ConnectionConfig::sqlite(temp_file.clone());

    // Create tokio runtime for async operations
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("sqlite_introspect_multiple_tables", |b| {
        b.iter(|| {
            use plenum::engine::IntrospectOperation;
            let result = runtime.block_on(SqliteEngine::introspect(
                black_box(&config),
                &IntrospectOperation::ListTables,
                None,
                None,
            ));
            assert!(result.is_ok());
            result
        });
    });

    // Cleanup
    let _ = std::fs::remove_file(&temp_file);
}

#[cfg(feature = "sqlite")]
criterion_group!(benches, bench_sqlite_introspection_simple, bench_sqlite_introspection_complex);

#[cfg(not(feature = "sqlite"))]
criterion_group!(benches,);

criterion_main!(benches);
