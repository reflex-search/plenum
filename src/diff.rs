//! Schema diff computation
//!
//! Computes a structural diff between two databases by calling existing `DatabaseEngine::introspect`
//! operations on each connection independently. No engine-specific SQL is added here — all data
//! comes from the standard suite of `IntrospectOperation` calls.

use std::collections::BTreeMap;

use crate::engine::{
    ColumnChange, ColumnInfo, ConnectionConfig, DatabaseEngine, DatabaseType, DefinitionChange,
    ForeignKeyInfo, IndexInfo, IntrospectOperation, IntrospectResult, PrimaryKeyChange, SchemaDiff,
    TableDiff, TableFields, TableInfo, ViewDiff, ViewInfo,
};
use crate::error::{PlenumError, Result};

#[cfg(feature = "duckdb")]
use crate::engine::duckdb::DuckDbEngine;
#[cfg(feature = "mysql")]
use crate::engine::mysql::MySqlEngine;
#[cfg(feature = "postgres")]
use crate::engine::postgres::PostgresEngine;
#[cfg(feature = "sqlite")]
use crate::engine::sqlite::SqliteEngine;

/// Dispatch an introspect operation to the appropriate engine.
async fn engine_introspect(
    config: &ConnectionConfig,
    operation: &IntrospectOperation,
    database: Option<&str>,
    schema: Option<&str>,
) -> Result<IntrospectResult> {
    match config.engine {
        #[cfg(feature = "sqlite")]
        DatabaseType::SQLite => SqliteEngine::introspect(config, operation, database, schema).await,
        #[cfg(not(feature = "sqlite"))]
        DatabaseType::SQLite => Err(PlenumError::invalid_input(
            "SQLite engine not enabled. Build with --features sqlite.",
        )),

        #[cfg(feature = "postgres")]
        DatabaseType::Postgres => {
            PostgresEngine::introspect(config, operation, database, schema).await
        }
        #[cfg(not(feature = "postgres"))]
        DatabaseType::Postgres => Err(PlenumError::invalid_input(
            "PostgreSQL engine not enabled. Build with --features postgres.",
        )),

        #[cfg(feature = "mysql")]
        DatabaseType::MySQL => MySqlEngine::introspect(config, operation, database, schema).await,
        #[cfg(not(feature = "mysql"))]
        DatabaseType::MySQL => Err(PlenumError::invalid_input(
            "MySQL engine not enabled. Build with --features mysql.",
        )),

        #[cfg(feature = "duckdb")]
        DatabaseType::DuckDB => DuckDbEngine::introspect(config, operation, database, schema).await,
        #[cfg(not(feature = "duckdb"))]
        DatabaseType::DuckDB => Err(PlenumError::invalid_input(
            "DuckDB engine not enabled. Build with --features duckdb.",
        )),
    }
}

/// Gather all tables with full details visible to a connection.
async fn gather_tables(
    config: &ConnectionConfig,
    database: Option<&str>,
    schema: Option<&str>,
) -> Result<Vec<TableInfo>> {
    let list =
        engine_introspect(config, &IntrospectOperation::ListTables, database, schema).await?;
    let IntrospectResult::TableList { tables: names } = list else {
        return Err(PlenumError::engine_error(
            config.engine.as_str(),
            "ListTables returned unexpected result type",
        ));
    };

    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        let op =
            IntrospectOperation::TableDetails { name: name.clone(), fields: TableFields::all() };
        let detail = engine_introspect(config, &op, database, schema).await?;
        match detail {
            IntrospectResult::TableDetails { table } => tables.push(table),
            _ => {
                return Err(PlenumError::engine_error(
                    config.engine.as_str(),
                    "TableDetails returned unexpected result type",
                ))
            }
        }
    }
    Ok(tables)
}

/// Gather all views with full details visible to a connection.
async fn gather_views(
    config: &ConnectionConfig,
    database: Option<&str>,
    schema: Option<&str>,
) -> Result<Vec<ViewInfo>> {
    let list = engine_introspect(config, &IntrospectOperation::ListViews, database, schema).await?;
    let IntrospectResult::ViewList { views: names } = list else {
        return Err(PlenumError::engine_error(
            config.engine.as_str(),
            "ListViews returned unexpected result type",
        ));
    };

    let mut views = Vec::with_capacity(names.len());
    for name in names {
        let op = IntrospectOperation::ViewDetails { name: name.clone() };
        let detail = engine_introspect(config, &op, database, schema).await?;
        match detail {
            IntrospectResult::ViewDetails { view } => views.push(view),
            _ => {
                return Err(PlenumError::engine_error(
                    config.engine.as_str(),
                    "ViewDetails returned unexpected result type",
                ))
            }
        }
    }
    Ok(views)
}

/// Compute the structural diff between two database schemas.
///
/// Both connections are opened, introspected, and closed independently (stateless).
/// The diff is deterministic: all arrays are sorted alphabetically by name.
///
/// "added" means present in `target` but not `base`.
/// "removed" means present in `base` but not `target`.
pub async fn compute_schema_diff(
    base: &ConnectionConfig,
    target: &ConnectionConfig,
    database: Option<&str>,
    schema: Option<&str>,
) -> Result<SchemaDiff> {
    let base_tables = gather_tables(base, database, schema).await?;
    let base_views = gather_views(base, database, schema).await?;
    let target_tables = gather_tables(target, database, schema).await?;
    let target_views = gather_views(target, database, schema).await?;

    let (tables_added, tables_removed, tables_changed) = diff_tables(&base_tables, &target_tables);
    let (views_added, views_removed, views_changed) = diff_views(&base_views, &target_views);

    Ok(SchemaDiff {
        tables_added,
        tables_removed,
        tables_changed,
        views_added,
        views_removed,
        views_changed,
    })
}

fn diff_tables(
    base: &[TableInfo],
    target: &[TableInfo],
) -> (Vec<String>, Vec<String>, Vec<TableDiff>) {
    let base_map: BTreeMap<&str, &TableInfo> = base.iter().map(|t| (t.name.as_str(), t)).collect();
    let target_map: BTreeMap<&str, &TableInfo> =
        target.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut added: Vec<String> = target_map
        .keys()
        .filter(|k| !base_map.contains_key(*k))
        .map(|k| (*k).to_string())
        .collect();

    let mut removed: Vec<String> = base_map
        .keys()
        .filter(|k| !target_map.contains_key(*k))
        .map(|k| (*k).to_string())
        .collect();

    let mut changed: Vec<TableDiff> = base_map
        .iter()
        .filter_map(|(name, base_t)| {
            target_map.get(name).and_then(|target_t| {
                let d = diff_single_table(base_t, target_t);
                if table_diff_is_nonempty(&d) {
                    Some(d)
                } else {
                    None
                }
            })
        })
        .collect();

    added.sort();
    removed.sort();
    changed.sort_by(|a, b| a.name.cmp(&b.name));

    (added, removed, changed)
}

fn table_diff_is_nonempty(d: &TableDiff) -> bool {
    !d.columns_added.is_empty()
        || !d.columns_removed.is_empty()
        || !d.columns_changed.is_empty()
        || d.primary_key_changed.is_some()
        || !d.indexes_added.is_empty()
        || !d.indexes_removed.is_empty()
        || !d.foreign_keys_added.is_empty()
        || !d.foreign_keys_removed.is_empty()
}

fn diff_single_table(base: &TableInfo, target: &TableInfo) -> TableDiff {
    let (columns_added, columns_removed, columns_changed) =
        diff_columns(&base.columns, &target.columns);
    let primary_key_changed =
        diff_primary_key(base.primary_key.as_deref(), target.primary_key.as_deref());
    let (indexes_added, indexes_removed) = diff_indexes(&base.indexes, &target.indexes);
    let (foreign_keys_added, foreign_keys_removed) =
        diff_foreign_keys(&base.foreign_keys, &target.foreign_keys);

    TableDiff {
        name: base.name.clone(),
        columns_added,
        columns_removed,
        columns_changed,
        primary_key_changed,
        indexes_added,
        indexes_removed,
        foreign_keys_added,
        foreign_keys_removed,
    }
}

fn diff_columns(
    base: &[ColumnInfo],
    target: &[ColumnInfo],
) -> (Vec<ColumnInfo>, Vec<ColumnInfo>, Vec<ColumnChange>) {
    let base_map: BTreeMap<&str, &ColumnInfo> = base.iter().map(|c| (c.name.as_str(), c)).collect();
    let target_map: BTreeMap<&str, &ColumnInfo> =
        target.iter().map(|c| (c.name.as_str(), c)).collect();

    let mut added: Vec<ColumnInfo> = target_map
        .values()
        .filter(|c| !base_map.contains_key(c.name.as_str()))
        .map(|c| (*c).clone())
        .collect();

    let mut removed: Vec<ColumnInfo> = base_map
        .values()
        .filter(|c| !target_map.contains_key(c.name.as_str()))
        .map(|c| (*c).clone())
        .collect();

    let mut changed: Vec<ColumnChange> = base_map
        .iter()
        .filter_map(|(name, base_c)| {
            target_map.get(name).and_then(|target_c| {
                if column_changed(base_c, target_c) {
                    Some(ColumnChange {
                        name: (*name).to_string(),
                        from: (*base_c).clone(),
                        to: (*target_c).clone(),
                    })
                } else {
                    None
                }
            })
        })
        .collect();

    added.sort_by(|a, b| a.name.cmp(&b.name));
    removed.sort_by(|a, b| a.name.cmp(&b.name));
    changed.sort_by(|a, b| a.name.cmp(&b.name));

    (added, removed, changed)
}

fn column_changed(a: &ColumnInfo, b: &ColumnInfo) -> bool {
    a.data_type != b.data_type
        || a.nullable != b.nullable
        || a.default != b.default
        || a.comment != b.comment
}

fn diff_primary_key(
    base: Option<&[String]>,
    target: Option<&[String]>,
) -> Option<PrimaryKeyChange> {
    if base == target {
        return None;
    }
    Some(PrimaryKeyChange {
        from: base.map(<[std::string::String]>::to_vec),
        to: target.map(<[std::string::String]>::to_vec),
    })
}

fn diff_indexes(base: &[IndexInfo], target: &[IndexInfo]) -> (Vec<IndexInfo>, Vec<IndexInfo>) {
    let base_map: BTreeMap<&str, &IndexInfo> = base.iter().map(|i| (i.name.as_str(), i)).collect();
    let target_map: BTreeMap<&str, &IndexInfo> =
        target.iter().map(|i| (i.name.as_str(), i)).collect();

    let mut added: Vec<IndexInfo> = target_map
        .values()
        .filter(|i| !base_map.contains_key(i.name.as_str()))
        .map(|i| (*i).clone())
        .collect();

    let mut removed: Vec<IndexInfo> = base_map
        .values()
        .filter(|i| !target_map.contains_key(i.name.as_str()))
        .map(|i| (*i).clone())
        .collect();

    added.sort_by(|a, b| a.name.cmp(&b.name));
    removed.sort_by(|a, b| a.name.cmp(&b.name));

    (added, removed)
}

fn diff_foreign_keys(
    base: &[ForeignKeyInfo],
    target: &[ForeignKeyInfo],
) -> (Vec<ForeignKeyInfo>, Vec<ForeignKeyInfo>) {
    let base_map: BTreeMap<&str, &ForeignKeyInfo> =
        base.iter().map(|f| (f.name.as_str(), f)).collect();
    let target_map: BTreeMap<&str, &ForeignKeyInfo> =
        target.iter().map(|f| (f.name.as_str(), f)).collect();

    let mut added: Vec<ForeignKeyInfo> = target_map
        .values()
        .filter(|f| !base_map.contains_key(f.name.as_str()))
        .map(|f| (*f).clone())
        .collect();

    let mut removed: Vec<ForeignKeyInfo> = base_map
        .values()
        .filter(|f| !target_map.contains_key(f.name.as_str()))
        .map(|f| (*f).clone())
        .collect();

    added.sort_by(|a, b| a.name.cmp(&b.name));
    removed.sort_by(|a, b| a.name.cmp(&b.name));

    (added, removed)
}

fn diff_views(base: &[ViewInfo], target: &[ViewInfo]) -> (Vec<String>, Vec<String>, Vec<ViewDiff>) {
    let base_map: BTreeMap<&str, &ViewInfo> = base.iter().map(|v| (v.name.as_str(), v)).collect();
    let target_map: BTreeMap<&str, &ViewInfo> =
        target.iter().map(|v| (v.name.as_str(), v)).collect();

    let mut added: Vec<String> = target_map
        .keys()
        .filter(|k| !base_map.contains_key(*k))
        .map(|k| (*k).to_string())
        .collect();

    let mut removed: Vec<String> = base_map
        .keys()
        .filter(|k| !target_map.contains_key(*k))
        .map(|k| (*k).to_string())
        .collect();

    let mut changed: Vec<ViewDiff> = base_map
        .iter()
        .filter_map(|(name, base_v)| {
            target_map.get(name).and_then(|target_v| {
                let d = diff_single_view(base_v, target_v);
                if view_diff_is_nonempty(&d) {
                    Some(d)
                } else {
                    None
                }
            })
        })
        .collect();

    added.sort();
    removed.sort();
    changed.sort_by(|a, b| a.name.cmp(&b.name));

    (added, removed, changed)
}

fn view_diff_is_nonempty(d: &ViewDiff) -> bool {
    d.definition_changed.is_some()
        || !d.columns_added.is_empty()
        || !d.columns_removed.is_empty()
        || !d.columns_changed.is_empty()
}

fn diff_single_view(base: &ViewInfo, target: &ViewInfo) -> ViewDiff {
    let definition_changed = if base.definition == target.definition {
        None
    } else {
        Some(DefinitionChange { from: base.definition.clone(), to: target.definition.clone() })
    };

    let (columns_added, columns_removed, columns_changed) =
        diff_columns(&base.columns, &target.columns);

    ViewDiff {
        name: base.name.clone(),
        definition_changed,
        columns_added,
        columns_removed,
        columns_changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ColumnInfo, ForeignKeyInfo, IndexInfo, TableInfo, ViewInfo};

    fn make_col(name: &str, data_type: &str, nullable: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable,
            default: None,
            comment: None,
        }
    }

    fn make_table(name: &str, cols: Vec<ColumnInfo>) -> TableInfo {
        TableInfo {
            name: name.to_string(),
            schema: None,
            columns: cols,
            primary_key: None,
            foreign_keys: vec![],
            indexes: vec![],
            comment: None,
            row_estimate: None,
        }
    }

    fn make_view(name: &str, definition: Option<&str>, cols: Vec<ColumnInfo>) -> ViewInfo {
        ViewInfo {
            name: name.to_string(),
            schema: None,
            definition: definition.map(str::to_string),
            columns: cols,
        }
    }

    #[test]
    fn test_identical_tables_produce_empty_diff() {
        let tables = vec![make_table("users", vec![make_col("id", "int", false)])];
        let (added, removed, changed) = diff_tables(&tables, &tables);
        assert!(added.is_empty());
        assert!(removed.is_empty());
        assert!(changed.is_empty());
    }

    #[test]
    fn test_table_added_in_target() {
        let base = vec![make_table("users", vec![make_col("id", "int", false)])];
        let target = vec![
            make_table("users", vec![make_col("id", "int", false)]),
            make_table("orders", vec![make_col("id", "int", false)]),
        ];
        let (added, removed, _) = diff_tables(&base, &target);
        assert_eq!(added, vec!["orders"]);
        assert!(removed.is_empty());
    }

    #[test]
    fn test_table_removed_from_target() {
        let base = vec![
            make_table("users", vec![make_col("id", "int", false)]),
            make_table("orders", vec![make_col("id", "int", false)]),
        ];
        let target = vec![make_table("users", vec![make_col("id", "int", false)])];
        let (added, removed, _) = diff_tables(&base, &target);
        assert!(added.is_empty());
        assert_eq!(removed, vec!["orders"]);
    }

    #[test]
    fn test_column_type_change() {
        let base = vec![make_table("users", vec![make_col("id", "int", false)])];
        let target = vec![make_table("users", vec![make_col("id", "bigint", false)])];
        let (_, _, changed) = diff_tables(&base, &target);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].name, "users");
        assert_eq!(changed[0].columns_changed.len(), 1);
        assert_eq!(changed[0].columns_changed[0].from.data_type, "int");
        assert_eq!(changed[0].columns_changed[0].to.data_type, "bigint");
    }

    #[test]
    fn test_column_nullability_change() {
        let base = vec![make_table("t", vec![make_col("name", "text", false)])];
        let target = vec![make_table("t", vec![make_col("name", "text", true)])];
        let (_, _, changed) = diff_tables(&base, &target);
        assert!(!changed[0].columns_changed[0].from.nullable);
        assert!(changed[0].columns_changed[0].to.nullable);
    }

    #[test]
    fn test_column_added() {
        let base = vec![make_table("users", vec![make_col("id", "int", false)])];
        let target = vec![make_table(
            "users",
            vec![make_col("id", "int", false), make_col("email", "varchar(255)", true)],
        )];
        let (_, _, changed) = diff_tables(&base, &target);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].columns_added.len(), 1);
        assert_eq!(changed[0].columns_added[0].name, "email");
        assert!(changed[0].columns_removed.is_empty());
    }

    #[test]
    fn test_column_removed() {
        let base = vec![make_table(
            "users",
            vec![make_col("id", "int", false), make_col("email", "varchar(255)", true)],
        )];
        let target = vec![make_table("users", vec![make_col("id", "int", false)])];
        let (_, _, changed) = diff_tables(&base, &target);
        assert_eq!(changed[0].columns_removed.len(), 1);
        assert_eq!(changed[0].columns_removed[0].name, "email");
    }

    #[test]
    fn test_primary_key_change() {
        let mut base_t = make_table("users", vec![make_col("id", "int", false)]);
        base_t.primary_key = Some(vec!["id".to_string()]);
        let mut target_t = make_table("users", vec![make_col("id", "int", false)]);
        target_t.primary_key = Some(vec!["id".to_string(), "tenant_id".to_string()]);

        let d = diff_single_table(&base_t, &target_t);
        assert!(d.primary_key_changed.is_some());
        let pk = d.primary_key_changed.unwrap();
        assert_eq!(pk.from, Some(vec!["id".to_string()]));
        assert_eq!(pk.to, Some(vec!["id".to_string(), "tenant_id".to_string()]));
    }

    #[test]
    fn test_primary_key_unchanged_produces_no_change() {
        let mut base_t = make_table("users", vec![make_col("id", "int", false)]);
        base_t.primary_key = Some(vec!["id".to_string()]);
        let mut target_t = make_table("users", vec![make_col("id", "int", false)]);
        target_t.primary_key = Some(vec!["id".to_string()]);

        let d = diff_single_table(&base_t, &target_t);
        assert!(d.primary_key_changed.is_none());
        assert!(!table_diff_is_nonempty(&d));
    }

    #[test]
    fn test_index_added() {
        let idx = IndexInfo {
            name: "idx_email".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
        };
        let base = vec![make_table("users", vec![make_col("id", "int", false)])];
        let mut target_t = make_table("users", vec![make_col("id", "int", false)]);
        target_t.indexes = vec![idx];
        let target = vec![target_t];

        let (_, _, changed) = diff_tables(&base, &target);
        assert_eq!(changed[0].indexes_added.len(), 1);
        assert_eq!(changed[0].indexes_added[0].name, "idx_email");
    }

    #[test]
    fn test_foreign_key_removed() {
        let fk = ForeignKeyInfo {
            name: "fk_user".to_string(),
            columns: vec!["user_id".to_string()],
            referenced_table: "users".to_string(),
            referenced_columns: vec!["id".to_string()],
        };
        let mut base_t = make_table("orders", vec![make_col("id", "int", false)]);
        base_t.foreign_keys = vec![fk];
        let target = vec![make_table("orders", vec![make_col("id", "int", false)])];
        let base = vec![base_t];

        let (_, _, changed) = diff_tables(&base, &target);
        assert_eq!(changed[0].foreign_keys_removed.len(), 1);
        assert_eq!(changed[0].foreign_keys_removed[0].name, "fk_user");
    }

    #[test]
    fn test_identical_views_produce_empty_diff() {
        let views =
            vec![make_view("v_active", Some("SELECT 1"), vec![make_col("id", "int", false)])];
        let (added, removed, changed) = diff_views(&views, &views);
        assert!(added.is_empty());
        assert!(removed.is_empty());
        assert!(changed.is_empty());
    }

    #[test]
    fn test_view_definition_change() {
        let base = vec![make_view(
            "v_users",
            Some("SELECT id FROM users"),
            vec![make_col("id", "int", false)],
        )];
        let target = vec![make_view(
            "v_users",
            Some("SELECT id, name FROM users"),
            vec![make_col("id", "int", false)],
        )];
        let (_, _, changed) = diff_views(&base, &target);
        assert_eq!(changed.len(), 1);
        let dc = changed[0].definition_changed.as_ref().unwrap();
        assert_eq!(dc.from.as_deref(), Some("SELECT id FROM users"));
        assert_eq!(dc.to.as_deref(), Some("SELECT id, name FROM users"));
    }

    #[test]
    fn test_stable_ordering_alphabetical() {
        let base_tables = vec![
            make_table("zebra", vec![make_col("id", "int", false)]),
            make_table("apple", vec![make_col("id", "int", false)]),
        ];
        let target_tables = vec![
            make_table("mango", vec![make_col("id", "int", false)]),
            make_table("apple", vec![make_col("id", "int", false)]),
        ];
        let (added, removed, _) = diff_tables(&base_tables, &target_tables);
        assert_eq!(added, vec!["mango"]);
        assert_eq!(removed, vec!["zebra"]);
    }

    #[test]
    fn test_schema_diff_empty_arrays_always_serialized() {
        let diff = SchemaDiff {
            tables_added: vec![],
            tables_removed: vec![],
            tables_changed: vec![],
            views_added: vec![],
            views_removed: vec![],
            views_changed: vec![],
        };
        let json = serde_json::to_string(&diff).unwrap();
        assert!(json.contains("\"tables_added\":[]"));
        assert!(json.contains("\"tables_removed\":[]"));
        assert!(json.contains("\"tables_changed\":[]"));
        assert!(json.contains("\"views_added\":[]"));
        assert!(json.contains("\"views_removed\":[]"));
        assert!(json.contains("\"views_changed\":[]"));
    }
}
