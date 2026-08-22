-- 0001_notifications.sql (SQLite) -- the in-app notification inbox.
--
-- Same table and same rules as the other dialects; see the PostgreSQL file
-- for why `notifiable_key` is not a foreign key, why `kind` is an
-- application-chosen name rather than a Rust type path, and why `read_at` is
-- a nullable timestamp rather than a boolean. Two storage differences SQLite
-- forces:
--
--   * `id` is a BLOB. It is 16 raw bytes, and a TEXT column would compare a
--     byte string against a text encoding of it and never match.
--   * `read_at` and `created_at` are INTEGER epoch milliseconds. SQLite has
--     no timestamp type; text timestamps only compare correctly while every
--     writer agrees on the exact format, and integers always do. `read_at`
--     stays nullable, so "unread" is still the absence of a value and not a
--     sentinel number some future reader has to know about.
--
-- Statements are separated by a line reading `--;;`.

CREATE TABLE IF NOT EXISTS arcature_notifications (
    id             BLOB    PRIMARY KEY NOT NULL,
    notifiable_key TEXT    NOT NULL,
    kind           TEXT    NOT NULL,
    data           TEXT    NOT NULL,
    read_at        INTEGER,
    created_at     INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)
--;;
-- The inbox read: one recipient's notifications, newest first.
CREATE INDEX IF NOT EXISTS arcature_notifications_inbox_idx
    ON arcature_notifications (notifiable_key, created_at DESC)
--;;
-- The unread badge, which is read on far more page loads than the inbox is.
CREATE INDEX IF NOT EXISTS arcature_notifications_unread_idx
    ON arcature_notifications (notifiable_key, read_at)
