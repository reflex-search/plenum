# Plenum Usage Examples

This document provides comprehensive examples for using Plenum's three commands: `connect`, `introspect`, and `query`. All examples show both the command invocation and the expected JSON output.

## Table of Contents

- [Connection Management](#connection-management)
- [Schema Introspection](#schema-introspection)
- [Query Execution](#query-execution)
- [Common Workflows](#common-workflows)
- [Error Handling](#error-handling)

---

## Connection Management

### Interactive Connection Picker

Launch the interactive connection picker when no arguments are provided:

```bash
plenum connect
```

This will:
1. Display existing named connections
2. Allow selection via numbered input
3. Include "--- Create New ---" option
4. Prompt for connection details interactively

### Create PostgreSQL Connection

```bash
plenum connect \
  --name prod-db \
  --engine postgres \
  --host db.production.example.com \
  --port 5432 \
  --user readonly_user \
  --password secretpassword \
  --database app_production \
  --save global
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "connect",
  "data": {
    "connection_name": "prod-db",
    "engine": "postgres",
    "saved_to": "global",
    "message": "Connection 'prod-db' saved successfully"
  },
  "meta": {
    "execution_ms": 245
  }
}
```

### Create MySQL Connection

```bash
plenum connect \
  --name local-mysql \
  --engine mysql \
  --host localhost \
  --port 3306 \
  --user root \
  --password rootpass \
  --database test_db \
  --save local
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "mysql",
  "command": "connect",
  "data": {
    "connection_name": "local-mysql",
    "engine": "mysql",
    "saved_to": "local",
    "message": "Connection 'local-mysql' saved successfully"
  },
  "meta": {
    "execution_ms": 198
  }
}
```

### Create SQLite Connection

```bash
plenum connect \
  --name app-cache \
  --engine sqlite \
  --file /var/lib/app/cache.db \
  --save local
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "sqlite",
  "command": "connect",
  "data": {
    "connection_name": "app-cache",
    "engine": "sqlite",
    "saved_to": "local",
    "message": "Connection 'app-cache' saved successfully"
  },
  "meta": {
    "execution_ms": 42
  }
}
```

### Validate Existing Connection

```bash
plenum connect --name prod-db
```

This re-validates an existing connection and displays connection metadata.

---

## Schema Introspection

### Introspect Using Named Connection

```bash
plenum introspect --name prod-db
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "introspect",
  "data": {
    "tables": [
      {
        "name": "users",
        "schema": "public",
        "columns": [
          {
            "name": "id",
            "data_type": "integer",
            "nullable": false,
            "default": "nextval('users_id_seq'::regclass)"
          },
          {
            "name": "email",
            "data_type": "character varying",
            "nullable": false,
            "default": null
          },
          {
            "name": "created_at",
            "data_type": "timestamp with time zone",
            "nullable": false,
            "default": "CURRENT_TIMESTAMP"
          }
        ],
        "primary_key": ["id"],
        "foreign_keys": [],
        "indexes": [
          {
            "name": "users_pkey",
            "columns": ["id"],
            "unique": true
          },
          {
            "name": "idx_users_email",
            "columns": ["email"],
            "unique": true
          }
        ]
      }
    ]
  },
  "meta": {
    "execution_ms": 523
  }
}
```

### Introspect Specific Schema

```bash
plenum introspect --name prod-db --schema analytics
```

Filters introspection to only the `analytics` schema (PostgreSQL/MySQL).

### Introspect with Explicit Connection

```bash
plenum introspect \
  --engine postgres \
  --host localhost \
  --port 5432 \
  --user admin \
  --password adminpass \
  --database myapp
```

Use explicit connection parameters without saving a named connection.

### SQLite Introspection

```bash
plenum introspect --engine sqlite --file ./app.db
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "sqlite",
  "command": "introspect",
  "data": {
    "tables": [
      {
        "name": "products",
        "schema": null,
        "columns": [
          {
            "name": "id",
            "data_type": "INTEGER",
            "nullable": false,
            "default": null
          },
          {
            "name": "name",
            "data_type": "TEXT",
            "nullable": false,
            "default": null
          },
          {
            "name": "price",
            "data_type": "REAL",
            "nullable": true,
            "default": "0.0"
          }
        ],
        "primary_key": ["id"],
        "foreign_keys": [],
        "indexes": []
      }
    ]
  },
  "meta": {
    "execution_ms": 38
  }
}
```

---

## Query Execution

### Read-Only Query (Default)

```bash
plenum query \
  --name prod-db \
  --sql "SELECT id, email, created_at FROM users WHERE id = 1"
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "query",
  "data": {
    "columns": ["id", "email", "created_at"],
    "rows": [
      {
        "id": 1,
        "email": "admin@example.com",
        "created_at": "2024-01-15T10:30:00Z"
      }
    ],
    "rows_affected": null
  },
  "meta": {
    "execution_ms": 87,
    "rows_returned": 1
  }
}
```

### Query with Row Limit

```bash
plenum query \
  --name prod-db \
  --sql "SELECT * FROM large_table" \
  --max-rows 1000
```

Limits the result set to 1000 rows, even if more exist.

### Query with Timeout

```bash
plenum query \
  --name prod-db \
  --sql "SELECT * FROM expensive_view" \
  --timeout-ms 5000
```

Sets a 5-second timeout for query execution.

### Write Query (INSERT)

```bash
plenum query \
  --name prod-db \
  --sql "INSERT INTO logs (level, message) VALUES ('INFO', 'Application started')" \
  --allow-write
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "query",
  "data": {
    "columns": [],
    "rows": [],
    "rows_affected": 1
  },
  "meta": {
    "execution_ms": 45,
    "rows_returned": 0
  }
}
```

### Write Query (UPDATE)

```bash
plenum query \
  --name prod-db \
  --sql "UPDATE users SET last_login = CURRENT_TIMESTAMP WHERE id = 42" \
  --allow-write
```

**Success Output:**
```json
{
  "ok": true,
  "engine": "postgres",
  "command": "query",
  "data": {
    "columns": [],
    "rows": [],
    "rows_affected": 1
  },
  "meta": {
    "execution_ms": 52,
    "rows_returned": 0
  }
}
```

### Write Query (DELETE)

```bash
plenum query \
  --name prod-db \
  --sql "DELETE FROM temp_cache WHERE created_at < NOW() - INTERVAL '1 hour'" \
  --allow-write
```

### DDL Query (CREATE TABLE)

```bash
plenum query \
  --name prod-db \
  --sql "CREATE TABLE temp_results (id SERIAL PRIMARY KEY, value TEXT)" \
  --allow-ddl
```

**Note:** `--allow-ddl` implicitly grants write permissions.

### DDL Query (CREATE INDEX)

```bash
plenum query \
  --name prod-db \
  --sql "CREATE INDEX idx_users_created_at ON users(created_at)" \
  --allow-ddl
```

### DDL Query (DROP TABLE)

```bash
plenum query \
  --name prod-db \
  --sql "DROP TABLE IF EXISTS temp_results" \
  --allow-ddl
```

### Query from File

```bash
plenum query \
  --name prod-db \
  --sql-file ./queries/report.sql
```

Reads SQL from a file instead of command line.

### Complex Query with JOIN

```bash
plenum query \
  --name prod-db \
  --sql "
    SELECT u.id, u.email, COUNT(p.id) as post_count
    FROM users u
    LEFT JOIN posts p ON u.id = p.user_id
    GROUP BY u.id, u.email
    ORDER BY post_count DESC
    LIMIT 10
  "
```

---

## Common Workflows

### Workflow 1: Setup and Validate Connection

```bash
# Step 1: Create connection
plenum connect \
  --name analytics-db \
  --engine postgres \
  --host analytics.example.com \
  --port 5432 \
  --user readonly \
  --password secret123 \
  --database analytics \
  --save global

# Step 2: Validate it works
plenum introspect --name analytics-db

# Step 3: Run a test query
plenum query --name analytics-db --sql "SELECT version()"
```

### Workflow 2: Explore Unknown Database

```bash
# Step 1: Connect and save
plenum connect --name unknown-db --engine mysql --host localhost \
  --port 3306 --user root --password rootpass --database mystery_db --save local

# Step 2: Discover schema
plenum introspect --name unknown-db > schema.json

# Step 3: Examine table contents
plenum query --name unknown-db --sql "SELECT * FROM table_name LIMIT 10"
```

### Workflow 3: Data Migration Check

```bash
# Check source row count
plenum query --name source-db \
  --sql "SELECT COUNT(*) as count FROM users"

# Check destination row count
plenum query --name dest-db \
  --sql "SELECT COUNT(*) as count FROM users"

# Verify data integrity
plenum query --name dest-db \
  --sql "SELECT id, email FROM users WHERE id NOT IN (SELECT id FROM source_users)"
```

### Workflow 4: Temporary Table Workflow

```bash
# Create temporary table
plenum query --name work-db \
  --sql "CREATE TEMPORARY TABLE analysis_tmp (id INT, score REAL)" \
  --allow-ddl

# Populate it
plenum query --name work-db \
  --sql "INSERT INTO analysis_tmp SELECT user_id, AVG(rating) FROM reviews GROUP BY user_id" \
  --allow-write

# Query results
plenum query --name work-db \
  --sql "SELECT * FROM analysis_tmp WHERE score > 4.5"

# No cleanup needed - temporary table drops when connection closes
```

---

## Error Handling

### Capability Violation (Missing --allow-write)

```bash
plenum query --name prod-db \
  --sql "UPDATE users SET email = 'test@example.com' WHERE id = 1"
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "Write operations require --allow-write flag"
  }
}
```

### Capability Violation (Missing --allow-ddl)

```bash
plenum query --name prod-db \
  --sql "DROP TABLE users" \
  --allow-write
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "CAPABILITY_VIOLATION",
    "message": "DDL operations require --allow-ddl flag"
  }
}
```

### Query Failed (Syntax Error)

```bash
plenum query --name prod-db \
  --sql "SELCT * FROM users"
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "QUERY_FAILED",
    "message": "Failed to prepare query: syntax error at or near \"SELCT\""
  }
}
```

### Query Failed (Table Not Found)

```bash
plenum query --name prod-db \
  --sql "SELECT * FROM nonexistent_table"
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "query",
  "error": {
    "code": "QUERY_FAILED",
    "message": "Failed to execute query: relation \"nonexistent_table\" does not exist"
  }
}
```

### Connection Failed

```bash
plenum connect --name bad-connection \
  --engine postgres --host invalid.host.example.com \
  --port 5432 --user test --password test --database test
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "postgres",
  "command": "connect",
  "error": {
    "code": "CONNECTION_FAILED",
    "message": "Failed to connect to database: connection refused"
  }
}
```

### Invalid Input (Missing Required Parameter)

```bash
plenum query --name prod-db
```

**Error Output:**
```json
{
  "ok": false,
  "engine": "",
  "command": "query",
  "error": {
    "code": "INVALID_INPUT",
    "message": "Either --sql or --sql-file is required"
  }
}
```

---

## Advanced Examples

### Using Environment Variables for Passwords

Create a connection that references an environment variable:

```bash
export DB_PASSWORD="super_secret_password"

plenum connect \
  --name secure-db \
  --engine postgres \
  --host db.example.com \
  --port 5432 \
  --user appuser \
  --password-env DB_PASSWORD \
  --database production \
  --save global
```

The password is read from `$DB_PASSWORD` at runtime, not stored in the config file.

### Parameterized Queries (Agent Responsibility)

**Important:** Plenum does NOT provide SQL parameterization. The calling agent MUST sanitize inputs:

```javascript
// Agent-side example (pseudo-code)
const userId = sanitize(userInput); // Agent must sanitize!
const sql = `SELECT * FROM users WHERE id = ${userId}`;
exec(`plenum query --name db --sql "${sql}"`);
```

Plenum assumes all SQL passed to it is safe and validated by the agent.

### Combining with JSON Processing

Using `jq` to extract specific fields from Plenum output:

```bash
# Get just the row count
plenum query --name db --sql "SELECT COUNT(*) as count FROM users" \
  | jq -r '.data.rows[0].count'

# Extract error code for conditional logic
RESULT=$(plenum query --name db --sql "SELECT * FROM users")
ERROR_CODE=$(echo "$RESULT" | jq -r '.error.code // empty')

if [ "$ERROR_CODE" == "CAPABILITY_VIOLATION" ]; then
  echo "Insufficient permissions"
fi
```

### Schema Diff Workflow

```bash
# Introspect source
plenum introspect --name source-db > source-schema.json

# Introspect destination
plenum introspect --name dest-db > dest-schema.json

# Compare (using external tool)
diff source-schema.json dest-schema.json
```

---

## Tips for AI Agents

1. **Always check the `ok` field first** - Don't assume success
2. **Match on `error.code`** for programmatic error handling
3. **Parse JSON output** - All Plenum output is valid JSON
4. **Sanitize user inputs** - Plenum passes SQL verbatim to the database
5. **Use named connections** - Avoid repeating connection parameters
6. **Start read-only** - Only add `--allow-write` or `--allow-ddl` when necessary
7. **Set max-rows for unknown queries** - Prevent accidentally fetching millions of rows
8. **Use timeouts for expensive queries** - Protect against long-running operations

---

## Configuration Files

### Local Configuration (.plenum/config.json)

```json
{
  "connections": {
    "default": {
      "engine": "sqlite",
      "file": "./app.db"
    },
    "prod": {
      "engine": "postgres",
      "host": "db.example.com",
      "port": 5432,
      "user": "readonly",
      "password_env": "PROD_DB_PASSWORD",
      "database": "production"
    }
  },
  "default_connection": "default"
}
```

### Global Configuration (~/.config/plenum/connections.json)

```json
{
  "connections": {
    "personal-pg": {
      "engine": "postgres",
      "host": "localhost",
      "port": 5432,
      "user": "postgres",
      "password": "localdev",
      "database": "myapp"
    }
  }
}
```

---

## Performance Considerations

- **Connection overhead**: Each command opens and closes a connection (stateless design)
- **Large result sets**: Use `--max-rows` to limit memory usage
- **Complex introspection**: Databases with 100+ tables may take several seconds to introspect
- **Network latency**: Remote database connections will be slower than local
- **SQLite is fastest**: No network overhead, synchronous driver

Run benchmarks to measure performance on your workload:

```bash
cargo bench --features sqlite
```
