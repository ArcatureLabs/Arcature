//! SQLite statement text for the notification inbox.
//!
//! Placeholders are `?`, bound in order of appearance. Timestamps are epoch
//! milliseconds in an `INTEGER` column, so "now" is spelled
//! `CAST((julianday('now')-2440587.5)*86400000 AS INTEGER)` -- 2440587.5 is
//! the Julian day of the Unix epoch. SQLite has no `now()`, and
//! `CURRENT_TIMESTAMP` yields text whose format does not compare correctly
//! against anything else the store writes.
//!
//! Every statement that reads or changes a row carries `notifiable_key`; see
//! the PostgreSQL file for why that is structural and not redundant.

/// Every statement the notification inbox issues against SQLite.
pub(crate) mod sql {
    /// Insert a notification that must not already exist.
    ///
    /// `OR IGNORE` reports the clash as zero rows affected rather than as an
    /// error, which is what lets `store` retry with a fresh id instead of
    /// matching on a driver-specific constraint message.
    /// Binds: id, notifiable key, kind, data, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT OR IGNORE INTO arcature_notifications
    (id, notifiable_key, kind, data, created_at)
VALUES (?, ?, ?, ?, ?)"#;

    /// One recipient's notifications, newest first.
    /// Binds: notifiable key, limit.
    pub(crate) const LIST: &str = r#"SELECT id, kind, data, read_at, created_at
  FROM arcature_notifications
 WHERE notifiable_key = ?
 ORDER BY created_at DESC, id
 LIMIT ?"#;

    /// The unread ones only, newest first.
    /// Binds: notifiable key, limit.
    pub(crate) const LIST_UNREAD: &str = r#"SELECT id, kind, data, read_at, created_at
  FROM arcature_notifications
 WHERE notifiable_key = ? AND read_at IS NULL
 ORDER BY created_at DESC, id
 LIMIT ?"#;

    /// The badge count.
    /// Binds: notifiable key.
    pub(crate) const COUNT_UNREAD: &str =
        "SELECT COUNT(*) FROM arcature_notifications WHERE notifiable_key = ? AND read_at IS NULL";

    /// Mark one notification read, if it belongs to this recipient and is not
    /// already read. `read_at IS NULL` in the predicate is what makes the
    /// first receipt the one that is kept.
    /// Binds: read at, notifiable key, id.
    pub(crate) const MARK_READ: &str = r#"UPDATE arcature_notifications
   SET read_at = ?
 WHERE notifiable_key = ? AND id = ? AND read_at IS NULL"#;

    /// Binds: read at, notifiable key.
    pub(crate) const MARK_ALL_READ: &str = r#"UPDATE arcature_notifications
   SET read_at = ?
 WHERE notifiable_key = ? AND read_at IS NULL"#;

    /// Binds: notifiable key, id.
    pub(crate) const DELETE: &str =
        "DELETE FROM arcature_notifications WHERE notifiable_key = ? AND id = ?";

    /// Binds: notifiable key.
    pub(crate) const DELETE_ALL: &str =
        "DELETE FROM arcature_notifications WHERE notifiable_key = ?";

    /// Retention: drop notifications that were read before a cutoff. Unread
    /// ones are never swept -- an inbox that quietly empties itself is worse
    /// than one that grows.
    /// Binds: cutoff.
    pub(crate) const DELETE_READ_BEFORE: &str =
        "DELETE FROM arcature_notifications WHERE read_at IS NOT NULL AND read_at < ?";

    /// The migration history table.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_notifications_schema_migrations (
    version    TEXT PRIMARY KEY,
    applied_at INTEGER NOT NULL
        DEFAULT (CAST((julianday('now')-2440587.5)*86400000 AS INTEGER))
)"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_notifications_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT OR IGNORE INTO arcature_notifications_schema_migrations (version) VALUES (?)";

    /// SQLite has no advisory lock, and needs none here: every statement in
    /// the migration is idempotent (`IF NOT EXISTS`, `INSERT OR IGNORE`) and
    /// SQLite serialises writers anyway, so two migrators racing converge on
    /// the same schema instead of conflicting.
    pub(crate) const LOCK: Option<&str> = None;

    /// See [`LOCK`].
    pub(crate) const UNLOCK: Option<&str> = None;

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/sqlite/0001_notifications.sql");
}
