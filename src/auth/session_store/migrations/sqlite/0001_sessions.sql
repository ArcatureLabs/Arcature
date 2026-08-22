-- 0001_sessions.sql (SQLite) -- the persistent session store.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why `id` is a digest rather than the session id. Two storage
-- differences SQLite forces:
--
--   * The digest is a BLOB. It is 32 raw bytes, and a TEXT column would
--     compare a byte string against a text encoding of it and never match.
--   * `expires_at` is INTEGER epoch milliseconds. SQLite has no timestamp
--     type; text timestamps only compare correctly while every writer agrees
--     on the exact format, and integers always do.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_sessions (
    id         BLOB    PRIMARY KEY NOT NULL,
    data       TEXT    NOT NULL,
    expires_at INTEGER NOT NULL
)
--;;
-- The sweep path: expired rows, oldest first.
CREATE INDEX IF NOT EXISTS arcature_sessions_expires_at_idx
    ON arcature_sessions (expires_at)
