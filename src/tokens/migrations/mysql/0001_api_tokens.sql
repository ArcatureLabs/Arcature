-- 0001_api_tokens.sql (MySQL 8) -- hashed personal access tokens.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why only a digest of the secret half is stored and why `expires_at` has
-- no null state. MySQL specifics:
--
--   * `id` is BINARY(16) and `secret_digest` BINARY(32) -- fixed width, no
--     collation, compared as bytes. A VARCHAR would bring a collation into a
--     comparison that is not text.
--   * Timestamps are DATETIME(6) holding UTC. The store writes and compares
--     them with UTC_TIMESTAMP(6), never NOW(), so the session time zone
--     cannot move an expiry.
--   * The indexes are declared inside CREATE TABLE: MySQL has no
--     `CREATE INDEX IF NOT EXISTS`, so separate statements would fail on the
--     second run.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_api_tokens (
    id            BINARY(16)   NOT NULL PRIMARY KEY,
    secret_digest BINARY(32)   NOT NULL,
    tokenable_id  VARCHAR(191) NOT NULL,
    name          VARCHAR(191) NOT NULL,
    abilities     JSON         NOT NULL,
    expires_at    DATETIME(6)  NOT NULL,
    created_at    DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    KEY arcature_api_tokens_tokenable_idx (tokenable_id),
    KEY arcature_api_tokens_expires_at_idx (expires_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4
