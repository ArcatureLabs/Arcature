-- 0001_sessions.sql (PostgreSQL) -- the persistent session store.
--
-- One table, arcature_sessions, holding one row per live session.
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `id` is not the session id. It is the SHA-256 digest of it, 32 raw bytes.
-- The session id travels in a cookie and is a bearer credential: whoever
-- holds it is the user until it expires. A table that stored it verbatim
-- would turn a backup, a replica, or a `SELECT` by a read-only reporting
-- account into a pile of usable logins. A digest is enough to look a row up
-- by -- the lookup hashes the id the request presented and compares -- and
-- the id itself is 128 bits of randomness, so the digest cannot be walked
-- back.
--
-- `expires_at` is not decoration. Every read carries `expires_at > now()`,
-- so an expired session is gone the moment it expires whether or not the
-- sweep has run. The index below is for the sweep, not for the read.

CREATE TABLE IF NOT EXISTS arcature_sessions (
    id         BYTEA       PRIMARY KEY,
    data       JSONB       NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
)
--;;
-- The sweep path: expired rows, oldest first.
CREATE INDEX IF NOT EXISTS arcature_sessions_expires_at_idx
    ON arcature_sessions (expires_at)
