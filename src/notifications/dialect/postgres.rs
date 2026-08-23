//! PostgreSQL statement text for the notification inbox.
//!
//! Placeholders are `$n`, numbered in order of appearance so the bind order
//! is the same as the `?` dialects.
//!
//! Every statement that reads or changes a row carries `notifiable_key`, and
//! that is not redundancy. It is what makes reaching another recipient's
//! notification impossible rather than merely forbidden: there is no
//! statement here that a handler could reach with an id alone.

/// Every statement the notification inbox issues against PostgreSQL.
pub(crate) mod sql {
    /// Insert a notification that must not already exist.
    /// `DO NOTHING` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `store` retry with a fresh id instead of
    /// parsing a driver-specific constraint name.
    /// Binds: id, notifiable key, kind, data, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT INTO arcature_notifications
    (id, notifiable_key, kind, data, created_at)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (id) DO NOTHING"#;

    /// One recipient's notifications, newest first.
    /// Binds: notifiable key, limit.
    pub(crate) const LIST: &str = r#"SELECT id, kind, data, read_at, created_at
  FROM arcature_notifications
 WHERE notifiable_key = $1
 ORDER BY created_at DESC, id
 LIMIT $2"#;

    /// The unread ones only, newest first.
    /// Binds: notifiable key, limit.
    pub(crate) const LIST_UNREAD: &str = r#"SELECT id, kind, data, read_at, created_at
  FROM arcature_notifications
 WHERE notifiable_key = $1 AND read_at IS NULL
 ORDER BY created_at DESC, id
 LIMIT $2"#;

    /// The badge count.
    /// Binds: notifiable key.
    pub(crate) const COUNT_UNREAD: &str =
        "SELECT COUNT(*) FROM arcature_notifications WHERE notifiable_key = $1 AND read_at IS NULL";

    /// Mark one notification read, if it belongs to this recipient and is not
    /// already read. `read_at IS NULL` in the predicate is what makes the
    /// first receipt the one that is kept.
    /// Binds: read at, notifiable key, id.
    pub(crate) const MARK_READ: &str = r#"UPDATE arcature_notifications
   SET read_at = $1
 WHERE notifiable_key = $2 AND id = $3 AND read_at IS NULL"#;

    /// Binds: read at, notifiable key.
    pub(crate) const MARK_ALL_READ: &str = r#"UPDATE arcature_notifications
   SET read_at = $1
 WHERE notifiable_key = $2 AND read_at IS NULL"#;

    /// Binds: notifiable key, id.
    pub(crate) const DELETE: &str =
        "DELETE FROM arcature_notifications WHERE notifiable_key = $1 AND id = $2";

    /// Binds: notifiable key.
    pub(crate) const DELETE_ALL: &str =
        "DELETE FROM arcature_notifications WHERE notifiable_key = $1";

    /// Retention: drop notifications that were read before a cutoff. Unread
    /// ones are never swept -- an inbox that quietly empties itself is worse
    /// than one that grows.
    /// Binds: cutoff.
    pub(crate) const DELETE_READ_BEFORE: &str =
        "DELETE FROM arcature_notifications WHERE read_at IS NOT NULL AND read_at < $1";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_notifications_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_notifications_schema_migrations WHERE version = $1";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str = "INSERT INTO arcature_notifications_schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING";

    /// Serialise concurrent migrators. Session-scoped, so it must be released.
    ///
    /// A key of its own, the next free one after the remember-token store's
    /// `71420005`. Sharing a key with another subsystem would make an
    /// application that migrates several of them at startup wait on itself.
    /// `tests/advisory_locks.rs` is the registry and fails if two subsystems
    /// ever claim the same number.
    pub(crate) const LOCK: Option<&str> = Some("SELECT pg_advisory_lock(71420006)");

    /// Release [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = Some("SELECT pg_advisory_unlock(71420006)");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/postgres/0001_notifications.sql");
}
