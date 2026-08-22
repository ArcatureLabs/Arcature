//! MySQL 8 statement text for the notification inbox.
//!
//! Placeholders are `?`, bound in order of appearance. Timestamps are
//! `DATETIME(6)` holding UTC, and every value the store writes is one it
//! produced itself -- never `NOW()`, whose meaning depends on the session
//! time zone. Where a default is unavoidable in DDL it is written
//! `CURRENT_TIMESTAMP(6)`, and the store overwrites it on every insert.
//!
//! Every statement that reads or changes a row carries `notifiable_key`; see
//! the PostgreSQL file for why that is structural and not redundant.

/// Every statement the notification inbox issues against MySQL.
pub(crate) mod sql {
    /// Insert a notification that must not already exist.
    ///
    /// `INSERT IGNORE` reports the clash as zero rows affected rather than as
    /// an error, which is what lets `store` retry with a fresh id instead of
    /// parsing a driver-specific constraint message.
    /// Binds: id, notifiable key, kind, data, created at.
    pub(crate) const INSERT_NEW: &str = r#"INSERT IGNORE INTO arcature_notifications
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

    /// The migration history table. `version` is VARCHAR(191) rather than
    /// TEXT because it is the primary key, and 191 is the widest utf8mb4
    /// prefix that fits the historic 767-byte index limit.
    pub(crate) const CREATE_HISTORY: &str = r#"CREATE TABLE IF NOT EXISTS arcature_notifications_schema_migrations (
    version    VARCHAR(191) NOT NULL PRIMARY KEY,
    applied_at DATETIME(6)  NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"#;

    /// Binds: version.
    pub(crate) const COUNT_APPLIED: &str =
        "SELECT COUNT(*) FROM arcature_notifications_schema_migrations WHERE version = ?";

    /// Binds: version. Idempotent so a racing migrator cannot fail on the
    /// primary key.
    pub(crate) const RECORD_APPLIED: &str =
        "INSERT IGNORE INTO arcature_notifications_schema_migrations (version) VALUES (?)";

    /// Serialise concurrent migrators. Named rather than numbered because
    /// MySQL's advisory locks take a string, and the name says which schema
    /// it guards -- the job queue, the session store, and the API tokens each
    /// hold their own, so an application that migrates all of them does not
    /// wait on itself.
    pub(crate) const LOCK: Option<&str> =
        Some("SELECT GET_LOCK('arcature_notifications_migrate', 10)");

    /// Release [`LOCK`]. MySQL releases session locks when the connection
    /// closes, but the migrator returns the connection to a pool that keeps
    /// it open, so releasing explicitly is what stops the next migrator from
    /// waiting out the timeout for nothing.
    pub(crate) const UNLOCK: Option<&str> =
        Some("SELECT RELEASE_LOCK('arcature_notifications_migrate')");

    /// The schema, one statement per `--;;` separated chunk.
    pub(crate) const SCHEMA: &str = include_str!("../migrations/mysql/0001_notifications.sql");
}
