-- 0001_api_tokens.sql (SQLite) -- hashed personal access tokens.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why only a digest of the secret half is stored and why `expires_at` has
-- no null state. Two storage differences SQLite forces:
--
--   * `id` and `secret_digest` are BLOBs. They are 16 and 32 raw bytes, and a
--     TEXT column would compare a byte string against a text encoding of it
--     and never match.
--   * `expires_at` and `created_at` are INTEGER epoch milliseconds. SQLite
--     has no timestamp type; text timestamps only compare correctly while
--     every writer agrees on the exact format, and integers always do.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_api_tokens (
    id            BLOB    PRIMARY KEY NOT NULL,
    secret_digest BLOB    NOT NULL,
    tokenable_id  TEXT    NOT NULL,
    name          TEXT    NOT NULL,
    abilities     TEXT    NOT NULL,
    expires_at    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)
--;;
-- Listing and mass revocation: every token belonging to one subject.
CREATE INDEX IF NOT EXISTS arcature_api_tokens_tokenable_idx
    ON arcature_api_tokens (tokenable_id)
--;;
-- The sweep path: expired rows, oldest first.
CREATE INDEX IF NOT EXISTS arcature_api_tokens_expires_at_idx
    ON arcature_api_tokens (expires_at)
