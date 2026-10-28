-- Plenum live-test seed: side-effect probes — PostgreSQL dialect.
-- Runs via /docker-entrypoint-initdb.d/ after 01_schema.sql and 02_data.sql,
-- connected to plenum_test as plenum (a superuser in the official image).
--
-- These objects let the live suite prove that a statement which passes the
-- read-only classifier still cannot change anything. The worst case is a
-- superuser connection, so that is what the suite runs as.
--
-- This file is PostgreSQL-specific by design. Do NOT share SQL with the
-- MySQL seed (tests/live/seed/mysql/) — engine isolation is a project rule.

-- dblink ships with the postgres image (contrib). It opens a second session,
-- which does not inherit Plenum's session-level read-only setting.
CREATE EXTENSION IF NOT EXISTS dblink;

-- Write target for every probe. Probes insert a per-run marker, and the
-- suite counts rows carrying that marker afterwards. It starts empty.
CREATE TABLE side_effect_canary (
    note text NOT NULL
);

-- A volatile function that writes. `SELECT side_effect_write('x')` looks
-- like a plain SELECT to the classifier.
CREATE FUNCTION side_effect_write(marker text) RETURNS integer
LANGUAGE sql VOLATILE
AS $$
    INSERT INTO side_effect_canary (note) VALUES (marker) RETURNING 1
$$;

-- A large object with a fixed OID, so lo_export() has something to write
-- to the server filesystem.
SELECT lo_from_bytea(4242, 'plenum side-effect probe');
