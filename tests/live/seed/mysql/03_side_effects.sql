-- Plenum live-test seed: side-effect probes — MySQL dialect.
-- Runs via /docker-entrypoint-initdb.d/ after 01_schema.sql and 02_data.sql,
-- as root, with plenum_test as the default database.
--
-- These objects let the live suite prove that a statement which passes the
-- read-only classifier still cannot change anything.
--
-- This file is MySQL-specific by design. Do NOT share SQL with the
-- PostgreSQL seed (tests/live/seed/postgres/) — engine isolation is a
-- project rule.

USE plenum_test;

-- FILE lets the test user write under secure_file_priv
-- (/var/lib/mysql-files/ in the official image). Without it, an
-- `INTO OUTFILE` probe fails on privileges and hides a classifier bypass.
GRANT FILE ON *.* TO 'plenum'@'%';

-- Write target for every probe. Probes insert a per-run marker or set a
-- per-run table comment, and the suite checks for that marker afterwards.
-- It starts empty.
CREATE TABLE side_effect_canary (
    note VARCHAR(64) NOT NULL
) COMMENT = 'canary';

-- Binary logging is on, so a function that modifies data needs this flag to
-- be created. It applies to the init server only; the real server starts
-- with the default.
SET GLOBAL log_bin_trust_function_creators = 1;

-- A function that writes. `SELECT side_effect_write('x')` looks like a plain
-- SELECT to the classifier.
DELIMITER //
CREATE FUNCTION side_effect_write(marker VARCHAR(64)) RETURNS INT
MODIFIES SQL DATA
BEGIN
    INSERT INTO side_effect_canary (note) VALUES (marker);
    RETURN 1;
END //
DELIMITER ;
