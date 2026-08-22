-- 0001_sessions.sql (MySQL 8) -- the persistent session store.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why `id` is a digest rather than the session id. MySQL specifics:
--
--   * The digest is BINARY(32) -- fixed width, no collation, compared as
--     bytes. A VARCHAR would bring a collation into a comparison that is not
--     text.
--   * `expires_at` is DATETIME(6) holding UTC. The store writes and compares
--     it with UTC_TIMESTAMP(6), never NOW(), so the session time zone cannot
--     move an expiry.
--   * The index is declared inside CREATE TABLE: MySQL has no
--     `CREATE INDEX IF NOT EXISTS`, so a separate statement would fail on the
--     second run.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_sessions (
    id         BINARY(32)  NOT NULL PRIMARY KEY,
    data       JSON        NOT NULL,
    expires_at DATETIME(6) NOT NULL,

    KEY arcature_sessions_expires_at_idx (expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4
