//! The inbox itself: store, list, count, mark read, delete, prune.

use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::types::Json;

use super::channel::NotificationError;
use super::dialect::{
    NotificationDb, NotificationPool, StoredTime, restored_time, sql, stored_time,
};
use super::migrate;
use super::notification::DatabaseContent;
use super::stored::{ID_BYTES, NotificationId, StoredNotification};

/// The row type of the dialect this build speaks.
type NotificationRow = <NotificationDb as sqlx::Database>::Row;

/// How many fresh ids [`DatabaseNotifications::store`] will try before giving
/// up.
///
/// An id is 128 bits, so one clash is already a once-in-the-heat-death event
/// and eight in a row is not chance -- it is a random source that is not
/// random. Looping forever would turn that into a hang; reporting it turns it
/// into a log line someone can act on.
const STORE_ATTEMPTS: u32 = 8;

/// An in-app notification inbox in the application's own database.
///
/// # What a row is
///
/// One delivered notification, belonging to one recipient, holding whatever
/// JSON the notification rendered plus a `kind` naming its shape. That is the
/// whole schema. It is deliberately not a copy of the email: what an inbox
/// shows is a headline, a link, and a time, and a table that also held a
/// rendered HTML body would grow without bound in exchange for something no
/// screen displays.
///
/// # Every statement is scoped by the recipient
///
/// There is no method here that takes an id alone. [`mark_read`](Self::mark_read)
/// and [`delete`](Self::delete) each take the recipient key as well, and the
/// key is in the `WHERE` clause rather than checked in Rust afterwards. This
/// is the difference between an ownership check a handler can forget and one
/// it cannot reach around: pass somebody else's notification id and the
/// statement matches no rows, so the answer is `false` rather than a
/// deletion. An inbox is exactly the sort of endpoint that grows an
/// insecure-direct-object-reference bug, and the shape of the API is what
/// prevents it.
///
/// # Retention
///
/// Nothing expires on its own. A notification is not a credential -- an
/// unread one is still worth reading a month later -- so how long an inbox
/// keeps history is an application decision, made by calling
/// [`prune_read_before`](Self::prune_read_before) on whatever schedule suits.
/// That sweep only ever touches notifications that were *read*: an inbox that
/// quietly empties itself of things nobody has seen is worse than one that
/// grows.
///
/// # Example
///
/// ```no_run
/// // Needs a database, so this example is compiled and not run.
/// use arcature::notifications::{DatabaseContent, DatabaseNotifications};
///
/// # async fn example(pool: arcature::notifications::NotificationPool)
/// # -> Result<(), Box<dyn std::error::Error>> {
/// let inbox = DatabaseNotifications::new(pool);
/// inbox.migrate().await?;
///
/// let row = inbox
///     .store(
///         "user:42",
///         &DatabaseContent::new("invoice.paid", serde_json::json!({ "amount": 4200 })),
///     )
///     .await?;
///
/// assert_eq!(inbox.unread_count("user:42").await?, 1);
/// assert!(inbox.mark_read("user:42", row.id()).await?);
/// assert_eq!(inbox.unread_count("user:42").await?, 0);
///
/// // Somebody else's key reaches nothing, whatever id they hold.
/// assert!(!inbox.delete("user:7", row.id()).await?);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DatabaseNotifications {
    pool: NotificationPool,
}

impl DatabaseNotifications {
    /// Build an inbox over an existing pool.
    #[must_use]
    pub fn new(pool: NotificationPool) -> Self {
        Self { pool }
    }

    /// The pool the inbox runs over.
    #[must_use]
    pub fn pool(&self) -> &NotificationPool {
        &self.pool
    }

    /// Create `arcature_notifications` and its indexes if they are not there.
    ///
    /// Idempotent, and safe to run from every replica at once: the migration
    /// is applied under the dialect's advisory lock with a history table.
    /// Call it at startup. An inbox whose table is missing fails on the first
    /// notification instead, which is the same outage discovered later.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the database is unreachable
    /// or rejects a statement.
    pub async fn migrate(&self) -> Result<(), NotificationError> {
        migrate::apply(&self.pool).await
    }

    /// Write one notification into a recipient's inbox.
    ///
    /// # Errors
    ///
    /// * [`NotificationError::Entropy`] if the OS randomness source is
    ///   unavailable.
    /// * [`NotificationError::IdCollision`] if [`STORE_ATTEMPTS`] random ids
    ///   were all taken, which in practice means the randomness source is
    ///   broken.
    /// * [`NotificationError::Database`] if the database rejects the insert.
    pub async fn store(
        &self,
        notifiable_key: &str,
        content: &DatabaseContent,
    ) -> Result<StoredNotification, NotificationError> {
        // Bound explicitly rather than left to the column default, so the
        // value written is the one this call can also return without a second
        // round trip to read it back.
        let created_at = Utc::now();

        for _ in 0..STORE_ATTEMPTS {
            let mut id_bytes = [0u8; ID_BYTES];
            fill_random(&mut id_bytes)?;

            let written = sqlx::query(sql::INSERT_NEW)
                .bind(id_bytes.to_vec())
                .bind(notifiable_key)
                .bind(content.kind())
                .bind(Json(content.data()))
                .bind(stored_time(created_at))
                .execute(&self.pool)
                .await?
                .rows_affected();

            if written == 0 {
                // `DO NOTHING` / `INSERT IGNORE` / `OR IGNORE`: the id is
                // taken. Draw another rather than parsing a driver-specific
                // constraint name out of an error.
                continue;
            }

            return Ok(StoredNotification {
                id: NotificationId::from_bytes(id_bytes),
                notifiable_key: notifiable_key.to_owned(),
                kind: content.kind().to_owned(),
                data: content.data().clone(),
                read_at: None,
                created_at,
            });
        }

        Err(NotificationError::IdCollision {
            attempts: STORE_ATTEMPTS,
        })
    }

    /// One recipient's notifications, newest first, at most `limit` of them.
    ///
    /// There is no unbounded variant on purpose. An inbox read is a page
    /// render, and a method that could return every notification a long-lived
    /// account ever received is a memory spike waiting for the one account
    /// that has them.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the query fails, or
    /// [`NotificationError::Decode`] / [`NotificationError::Timestamp`] if a
    /// row does not hold what the schema promises.
    pub async fn inbox(
        &self,
        notifiable_key: &str,
        limit: u32,
    ) -> Result<Vec<StoredNotification>, NotificationError> {
        self.list(sql::LIST, notifiable_key, limit).await
    }

    /// The unread ones only, newest first, at most `limit` of them.
    ///
    /// # Errors
    ///
    /// As [`inbox`](Self::inbox).
    pub async fn unread(
        &self,
        notifiable_key: &str,
        limit: u32,
    ) -> Result<Vec<StoredNotification>, NotificationError> {
        self.list(sql::LIST_UNREAD, notifiable_key, limit).await
    }

    /// Run one of the two listing statements.
    async fn list(
        &self,
        statement: &'static str,
        notifiable_key: &str,
        limit: u32,
    ) -> Result<Vec<StoredNotification>, NotificationError> {
        let rows = sqlx::query(statement)
            .bind(notifiable_key)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await?;

        rows.iter().map(|row| decode(row, notifiable_key)).collect()
    }

    /// How many of a recipient's notifications are unread.
    ///
    /// This is the badge, and it is a `COUNT` rather than the length of a
    /// listing: the number shown next to a bell is asked for on far more page
    /// loads than the inbox is opened, and it should not cost the rows.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the query fails.
    pub async fn unread_count(&self, notifiable_key: &str) -> Result<u64, NotificationError> {
        // A count decodes as `i64` on all three dialects; see the same note
        // in `migrate`.
        let count: i64 = sqlx::query(sql::COUNT_UNREAD)
            .bind(notifiable_key)
            .fetch_one(&self.pool)
            .await?
            .try_get::<i64, _>(0)?;
        Ok(count.max(0).unsigned_abs())
    }

    /// Mark one notification read, and report whether that changed anything.
    ///
    /// `false` means the statement matched no row, and the caller cannot tell
    /// which reason applies: no such notification, somebody else's
    /// notification, or one that was already read. That is deliberate. A
    /// handler that could distinguish "not yours" from "does not exist" would
    /// be an oracle for which ids exist, and none of the three cases calls for
    /// a different response.
    ///
    /// A notification that was already read keeps its original read time --
    /// the statement carries `read_at IS NULL` -- so "when did they first see
    /// this" survives a second click.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the statement fails.
    pub async fn mark_read(
        &self,
        notifiable_key: &str,
        id: NotificationId,
    ) -> Result<bool, NotificationError> {
        let affected = sqlx::query(sql::MARK_READ)
            .bind(stored_time(Utc::now()))
            .bind(notifiable_key)
            .bind(id.as_bytes().to_vec())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Mark everything unread in a recipient's inbox as read, and report how
    /// many rows that was.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the statement fails.
    pub async fn mark_all_read(&self, notifiable_key: &str) -> Result<u64, NotificationError> {
        let result = sqlx::query(sql::MARK_ALL_READ)
            .bind(stored_time(Utc::now()))
            .bind(notifiable_key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete one notification, and report whether it existed.
    ///
    /// Scoped by the recipient for the reason given on
    /// [`mark_read`](Self::mark_read): an id from another inbox matches
    /// nothing.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the statement fails.
    pub async fn delete(
        &self,
        notifiable_key: &str,
        id: NotificationId,
    ) -> Result<bool, NotificationError> {
        let affected = sqlx::query(sql::DELETE)
            .bind(notifiable_key)
            .bind(id.as_bytes().to_vec())
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Empty one recipient's inbox, and report how many rows that was.
    ///
    /// This is the "clear all" button, and it is also what an account deletion
    /// calls: the table has no foreign key onto the application's user table,
    /// so nothing cascades on its behalf.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the statement fails.
    pub async fn delete_all_for(&self, notifiable_key: &str) -> Result<u64, NotificationError> {
        let result = sqlx::query(sql::DELETE_ALL)
            .bind(notifiable_key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete every notification that was *read* before `cutoff`, across all
    /// recipients, and report how many.
    ///
    /// Unread notifications are never swept, however old. This is the one
    /// statement here that is not scoped to a recipient, and it is safe to be
    /// so precisely because it cannot reach anything a recipient has not
    /// already seen.
    ///
    /// # Errors
    ///
    /// Returns [`NotificationError::Database`] if the statement fails.
    pub async fn prune_read_before(&self, cutoff: DateTime<Utc>) -> Result<u64, NotificationError> {
        let result = sqlx::query(sql::DELETE_READ_BEFORE)
            .bind(stored_time(cutoff))
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}

/// Decode one row of a listing.
///
/// Columns by index, not by name: the three dialects agree on the order the
/// statement asks for -- `id, kind, data, read_at, created_at` -- and nothing
/// else has to be true.
fn decode(
    row: &NotificationRow,
    notifiable_key: &str,
) -> Result<StoredNotification, NotificationError> {
    let id_bytes: Vec<u8> = row.try_get(0)?;
    let id: [u8; ID_BYTES] = id_bytes.try_into().map_err(|bytes: Vec<u8>| {
        NotificationError::Decode(format!(
            "a notification id column held {} bytes, not {ID_BYTES}",
            bytes.len()
        ))
    })?;

    let kind: String = row.try_get(1)?;
    let data: Json<serde_json::Value> = row
        .try_get(2)
        .map_err(|error| NotificationError::Decode(error.to_string()))?;
    let read_at: Option<StoredTime> = row.try_get(3)?;
    let read_at = read_at.map(restored_time).transpose()?;
    let created_at = restored_time(row.try_get(4)?)?;

    Ok(StoredNotification {
        id: NotificationId::from_bytes(id),
        notifiable_key: notifiable_key.to_owned(),
        kind,
        data: data.0,
        read_at,
        created_at,
    })
}

/// Fill a buffer from the OS randomness source.
///
/// No fallback. If the OS cannot produce randomness the honest outcome is an
/// error the operator sees, not an id drawn from a clock.
fn fill_random(buffer: &mut [u8]) -> Result<(), NotificationError> {
    getrandom::fill(buffer).map_err(|_| NotificationError::Entropy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_random_source_fills_the_whole_buffer() {
        // Not a randomness test -- it is a wiring test. A buffer left
        // untouched by a silently failing call would give every notification
        // the same all-zero id, and the retry loop would then spend eight
        // attempts discovering that.
        let mut first = [0u8; ID_BYTES];
        let mut second = [0u8; ID_BYTES];
        fill_random(&mut first).expect("the OS randomness source is available");
        fill_random(&mut second).expect("the OS randomness source is available");
        assert_ne!(first, [0u8; ID_BYTES]);
        assert_ne!(first, second);
    }

    #[test]
    fn every_per_row_statement_names_the_recipient() {
        // The property the whole store rests on, asserted against the
        // statement text rather than trusted to review: there is no statement
        // a handler can reach with an id alone. `DELETE_READ_BEFORE` is the
        // deliberate exception -- it is scoped by `read_at` instead, so it
        // cannot touch anything a recipient has not already seen.
        for (name, statement) in [
            ("INSERT_NEW", sql::INSERT_NEW),
            ("LIST", sql::LIST),
            ("LIST_UNREAD", sql::LIST_UNREAD),
            ("COUNT_UNREAD", sql::COUNT_UNREAD),
            ("MARK_READ", sql::MARK_READ),
            ("MARK_ALL_READ", sql::MARK_ALL_READ),
            ("DELETE", sql::DELETE),
            ("DELETE_ALL", sql::DELETE_ALL),
        ] {
            assert!(
                statement.contains("notifiable_key"),
                "{name} is not scoped by its recipient: {statement}"
            );
        }

        assert!(
            !sql::DELETE_READ_BEFORE.contains("notifiable_key"),
            "the retention sweep is now recipient-scoped; this test needs rewriting"
        );
        assert!(
            sql::DELETE_READ_BEFORE.contains("read_at IS NOT NULL"),
            "the retention sweep can reach unread notifications"
        );
    }

    #[test]
    fn marking_read_keeps_the_first_receipt() {
        // Without `read_at IS NULL` a second click would overwrite the time
        // the recipient actually first saw the notification.
        assert!(
            sql::MARK_READ.contains("read_at IS NULL"),
            "MARK_READ would overwrite an existing read time: {}",
            sql::MARK_READ
        );
    }

    #[test]
    fn both_listings_agree_on_their_column_order() {
        // `decode` reads by index, so the two statements must ask for the
        // same columns in the same order. They are separate constants, which
        // is exactly how they would drift.
        let columns = "SELECT id, kind, data, read_at, created_at";
        assert!(sql::LIST.starts_with(columns), "{}", sql::LIST);
        assert!(
            sql::LIST_UNREAD.starts_with(columns),
            "{}",
            sql::LIST_UNREAD
        );
    }
}
