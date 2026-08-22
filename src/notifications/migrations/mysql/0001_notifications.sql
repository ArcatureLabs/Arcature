-- 0001_notifications.sql (MySQL 8) -- the in-app notification inbox.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why `notifiable_key` is not a foreign key, why `kind` is an
-- application-chosen name rather than a Rust type path, and why `read_at` is
-- a nullable timestamp rather than a boolean. MySQL specifics:
--
--   * `id` is BINARY(16) -- fixed width, no collation, compared as bytes. A
--     VARCHAR would bring a collation into a comparison that is not text.
--   * Text columns are VARCHAR(191), not TEXT: they are indexed, and 191 is
--     the widest prefix utf8mb4 fits in the historic 767-byte index limit.
--   * Timestamps are DATETIME(6) holding UTC. The store writes and compares
--     them with values it produced itself, never NOW(), so the session time
--     zone cannot move a read receipt.
--   * The indexes are declared inside CREATE TABLE: MySQL has no
--     `CREATE INDEX IF NOT EXISTS`, so separate statements would fail on the
--     second run.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_notifications (
    id             BINARY(16)   NOT NULL PRIMARY KEY,
    notifiable_key VARCHAR(191) NOT NULL,
    kind           VARCHAR(191) NOT NULL,
    data           JSON         NOT NULL,
    read_at        DATETIME(6)  NULL,
    created_at     DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6),

    KEY arcature_notifications_inbox_idx (notifiable_key, created_at DESC),
    KEY arcature_notifications_unread_idx (notifiable_key, read_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4
