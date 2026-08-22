-- 0001_notifications.sql (PostgreSQL) -- the in-app notification inbox.
--
-- One table, arcature_notifications, holding one row per notification
-- delivered to the database channel.
--
-- Statements are separated by a line reading `--;;` and executed one at a
-- time, so the file never relies on multi-statement support.
--
-- `notifiable_key` is the recipient's key, the same "user:42" spelling the
-- rest of the crate uses. It is not a foreign key on purpose: a notification
-- is a record of something that was said, and it should outlive a soft-delete
-- or a merge of the account it was said to rather than vanish with it. It is
-- also the *scope* of every read and every write -- there is no statement in
-- `src/notifications/dialect/` that touches a row by id alone -- so one
-- reader cannot reach another reader's inbox even holding a valid id.
--
-- `kind` is an application-chosen name for what happened, not a Rust type
-- path. The row outlives the code that wrote it: a rename, a move between
-- modules, or a crate split must not silently change what a stored row means.
--
-- `read_at` is nullable, and null is the whole meaning of "unread". A boolean
-- would answer "has this been read" and nothing else; a timestamp answers
-- "when", which is what an inbox that groups by day and a support engineer
-- reading a complaint both need.
--
-- There is no expiry column. Unlike an API token, a notification is not a
-- credential and nothing gets safer by dropping it on a schedule -- so
-- deletion is a retention decision the application makes, via
-- `prune_read_before`, and not a rule the schema enforces.

CREATE TABLE IF NOT EXISTS arcature_notifications (
    id             BYTEA       PRIMARY KEY,
    notifiable_key TEXT        NOT NULL,
    kind           TEXT        NOT NULL,
    data           JSONB       NOT NULL,
    read_at        TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
)
--;;
-- The inbox read: one recipient's notifications, newest first.
CREATE INDEX IF NOT EXISTS arcature_notifications_inbox_idx
    ON arcature_notifications (notifiable_key, created_at DESC)
--;;
-- The unread badge, which is read on far more page loads than the inbox is.
CREATE INDEX IF NOT EXISTS arcature_notifications_unread_idx
    ON arcature_notifications (notifiable_key, read_at)
