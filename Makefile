# Plenum developer targets.
#
# `make duckdb-test` is the full DuckDB verification: the offline parity
# suite followed by an end-to-end evidence run (seed a real .duckdb file,
# then drive the release binary against it, printing every JSON envelope
# to stdout — including the rejected write).

TARGET_DIR := $(or $(CARGO_TARGET_DIR),target)
PLENUM     := $(TARGET_DIR)/release/plenum
DEMO_DB    := $(TARGET_DIR)/duckdb-demo/demo.duckdb
DEMO_DSN := duckdb:$(DEMO_DB)

.PHONY: help duckdb-test duckdb-parity duckdb-evidence duckdb-seed clean-duckdb-demo

help:
	@echo "Targets:"
	@echo "  duckdb-test       full DuckDB test: parity suite + end-to-end evidence run"
	@echo "  duckdb-parity     offline DuckDB parity suite (cargo test --test duckdb_parity)"
	@echo "  duckdb-evidence   seed a demo .duckdb file and run plenum against it (evidence on stdout)"
	@echo "  duckdb-seed       (re)create the seeded demo database at $(DEMO_DB)"
	@echo "  clean-duckdb-demo remove the demo database"

duckdb-test: duckdb-parity duckdb-evidence

duckdb-parity:
	cargo test --test duckdb_parity

$(PLENUM): Cargo.toml $(shell find src -name '*.rs')
	cargo build --release

duckdb-seed:
	cargo run --quiet --example seed_duckdb -- $(DEMO_DB)

duckdb-evidence: $(PLENUM) duckdb-seed
	@echo "=== 1/5 introspect: list tables, then full detail for customers ==="
	$(PLENUM) introspect --dsn "$(DEMO_DSN)" --list-tables
	$(PLENUM) introspect --dsn "$(DEMO_DSN)" --table customers
	@echo
	@echo "=== 2/5 query: SELECT over customers (unicode round-trips) ==="
	$(PLENUM) query --dsn "$(DEMO_DSN)" --sql "SELECT id, name, email FROM customers ORDER BY id"
	@echo
	@echo "=== 3/5 query: aggregate view v_order_totals (JOIN + SUM over DECIMAL) ==="
	$(PLENUM) query --dsn "$(DEMO_DSN)" --sql "SELECT * FROM v_order_totals ORDER BY customer_id, order_no"
	@echo
	@echo "=== 4/5 query: max_rows truncation (1500-row table, --max-rows 5) ==="
	$(PLENUM) query --dsn "$(DEMO_DSN)" --sql "SELECT n, label FROM bulk_rows ORDER BY n" --max-rows 5
	@echo
	@echo "=== 5/5 query: INSERT must be rejected (CAPABILITY_VIOLATION expected) ==="
	@if $(PLENUM) query --dsn "$(DEMO_DSN)" --sql "INSERT INTO customers (id, name, email) VALUES (99, 'Eve', 'eve@example.com')"; then \
		echo "FAIL: write was not rejected" >&2; exit 1; \
	else \
		echo "OK: write rejected before execution"; \
	fi
	@echo
	@echo "duckdb-evidence: all 5 checks passed against $(DEMO_DB)"

clean-duckdb-demo:
	rm -f $(DEMO_DB)
