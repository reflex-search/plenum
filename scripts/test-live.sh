#!/usr/bin/env bash
# Live-DB test harness for Plenum (REF-274 / REF-275).
#
# Brings up seeded MySQL 8.0 / MySQL 8.4 / PostgreSQL 16 containers, exports
# the PLENUM_TEST_*_DSN env contract, runs the live integration suites, and
# tears everything down.
#
# Usage:
#   scripts/test-live.sh          # up --wait → test → down --volumes
#   scripts/test-live.sh --keep   # leave containers running for iteration
#
# Manual teardown after --keep:
#   docker compose -f tests/live/compose.yaml down --volumes
#
# Host ports are env-overridable (defaults match tests/live/compose.yaml):
#   PLENUM_TEST_MYSQL80_PORT    (default 43306)
#   PLENUM_TEST_MYSQL84_PORT    (default 43307)
#   PLENUM_TEST_POSTGRES16_PORT (default 45432)

set -euo pipefail

usage() {
    sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

KEEP=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --keep) KEEP=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$REPO_ROOT/tests/live/compose.yaml"

if ! docker compose version >/dev/null 2>&1; then
    echo "error: 'docker compose' is required (Docker with the compose plugin)." >&2
    exit 1
fi

compose() {
    docker compose -f "$COMPOSE_FILE" "$@"
}

teardown() {
    echo "==> Tearing down live test databases" >&2
    compose down --volumes --remove-orphans >&2
}

if [[ "$KEEP" -eq 0 ]]; then
    trap teardown EXIT
fi

MYSQL80_PORT="${PLENUM_TEST_MYSQL80_PORT:-43306}"
MYSQL84_PORT="${PLENUM_TEST_MYSQL84_PORT:-43307}"
POSTGRES16_PORT="${PLENUM_TEST_POSTGRES16_PORT:-45432}"

echo "==> Starting live test databases (compose up --wait)" >&2
compose up --wait

export PLENUM_TEST_MYSQL_DSN="mysql://plenum:plenum_pw@127.0.0.1:${MYSQL80_PORT}/plenum_test"
export PLENUM_TEST_MYSQL84_DSN="mysql://plenum:plenum_pw@127.0.0.1:${MYSQL84_PORT}/plenum_test"
export PLENUM_TEST_POSTGRES_DSN="postgres://plenum:plenum_pw@127.0.0.1:${POSTGRES16_PORT}/plenum_test"

echo "==> Running live test suites" >&2
cd "$REPO_ROOT"
cargo test --test live_mysql --test live_postgres -- --include-ignored

if [[ "$KEEP" -eq 1 ]]; then
    cat >&2 <<EOF
==> Containers left running (--keep). DSNs for manual runs:
    export PLENUM_TEST_MYSQL_DSN="$PLENUM_TEST_MYSQL_DSN"
    export PLENUM_TEST_MYSQL84_DSN="$PLENUM_TEST_MYSQL84_DSN"
    export PLENUM_TEST_POSTGRES_DSN="$PLENUM_TEST_POSTGRES_DSN"
    Tear down with: docker compose -f tests/live/compose.yaml down --volumes
EOF
fi
