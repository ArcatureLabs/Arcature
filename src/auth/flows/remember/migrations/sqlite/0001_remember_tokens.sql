-- 0001_remember_tokens.sql (SQLite) -- rotating remember-me tokens.
--
-- The PostgreSQL file carries the full commentary; this one records only what
-- differs.
--
-- Timestamps are epoch milliseconds in an INTEGER column rather than text.
-- SQLite has no timestamp type, and a text timestamp only compares correctly
-- while every writer agrees on the format down to the digit -- including
-- writers that are not this store.
--
-- `series`, `secret_digest` and `previous_digest` are BLOB. SQLite's flexible
-- typing would accept text in any of them, so the store binds bytes and never
-- a string; the column type records the intent for whoever reads the schema.
--
-- `previous_digest` and `rotated_at` are the only nullable columns, and they
-- are nullable together: NULL in both is how a token that has never rotated is
-- spelled.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_remember_tokens (
    series          BLOB    PRIMARY KEY,
    secret_digest   BLOB    NOT NULL,
    previous_digest BLOB,
    rotated_at      INTEGER,
    subject         TEXT    NOT NULL,
    expires_at      INTEGER NOT NULL,
    created_at      INTEGER NOT NULL
)
--;;
CREATE INDEX IF NOT EXISTS arcature_remember_tokens_subject_idx
    ON arcature_remember_tokens (subject)
--;;
CREATE INDEX IF NOT EXISTS arcature_remember_tokens_expires_at_idx
    ON arcature_remember_tokens (expires_at)
