-- 0001_password_resets.sql (SQLite) -- one-time password-reset tokens.
--
-- The PostgreSQL file carries the full commentary; this one records only what
-- differs.
--
-- Timestamps are epoch milliseconds in an INTEGER column rather than text.
-- SQLite has no timestamp type, and a text timestamp only compares correctly
-- while every writer agrees on the format down to the digit -- including
-- writers that are not this store.
--
-- `id` and `secret_digest` are BLOB. SQLite's flexible typing would accept
-- text in either, so the store binds bytes and never a string; the column type
-- records the intent for whoever reads the schema.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_password_resets (
    id            BLOB    PRIMARY KEY,
    secret_digest BLOB    NOT NULL,
    subject       TEXT    NOT NULL,
    expires_at    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL
)
--;;
CREATE INDEX IF NOT EXISTS arcature_password_resets_subject_idx
    ON arcature_password_resets (subject)
--;;
CREATE INDEX IF NOT EXISTS arcature_password_resets_expires_at_idx
    ON arcature_password_resets (expires_at)
