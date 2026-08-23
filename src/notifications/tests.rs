//! Round-trip tests against a live database.
//!
//! Store, list, count, mark, delete, prune -- against a real server. Nine
//! statements per dialect, and almost everything the inbox promises is a
//! promise the SQL makes rather than the Rust: the listing order is an
//! `ORDER BY`, the badge is a `COUNT`, "already read keeps its first receipt"
//! is a `read_at IS NULL` in a `WHERE`, and recipient scoping is a predicate
//! repeated on every statement that touches a row. A double built in memory
//! would agree with whatever the Rust believes and prove none of them.
//!
//! Two are worth naming. The payload is a JSON column, which is `JSONB` on
//! PostgreSQL, `JSON` on MySQL, and `TEXT` on SQLite -- one
//! `sqlx::types::Json` over three storage types that only a round trip can
//! tell apart. And the retention sweep is deliberately *not* recipient
//! scoped: it reaches read rows in every inbox, which is a property worth an
//! assertion precisely because it looks like an omission.
//!
//! # Why these tests need `test-kit`
//!
//! For the reason the remember-me store's do: "skip, or fail because CI
//! promised a database" must never be wrong in the lenient direction, and
//! [`crate::test_kit::database`] already owns that decision together with the
//! refusal to write to any database not named `arcature_test_*`. Re-spelling
//! either rule here would leave two copies of something that must never
//! disagree.
//!
//! # Why one test at a time
//!
//! There is one `arcature_notifications` table, and the retention sweep and
//! the row counting are blind to whose rows they are. Two tests running
//! concurrently would delete each other's notifications and both would be
//! right to fail.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use sqlx::types::Json;
use tokio::sync::{Mutex, MutexGuard};

use super::dialect::{NotificationPool, sql, stored_time};
use super::notification::DatabaseContent;
use super::store::DatabaseNotifications;
use super::stored::{ID_BYTES, NotificationId, StoredNotification};

/// Serialises the database tests against the one shared table.
static TABLE: Mutex<()> = Mutex::const_new(());

/// Pool size. More than one so nothing here passes because the pool
/// serialised it.
const CONNECTIONS: u32 = 4;

/// A page of an inbox, comfortably larger than anything stored here.
const PAGE: u32 = 50;

/// How far apart two instants may be and still be the same instant.
///
/// SQLite stores epoch milliseconds, so a timestamp that goes in and comes
/// back has lost everything below a millisecond. Two of them is slack enough
/// for that and tight enough that a dialect storing the wrong value -- a
/// local time read back as UTC, say -- fails by hours.
const SLACK: TimeDelta = TimeDelta::milliseconds(2);

// The two statements below re-date a row so that ordering and retention can
// be tested in milliseconds rather than by waiting. They are spelled per
// dialect, and as literals rather than assembled at runtime, for the reason
// SQLx's `SqlSafeStr` bound exists: a `String` built at runtime has to be
// waved past that gate with `AssertSqlSafe`, and a test fixture is the last
// place that escape hatch should be normalised.

/// Binds: new creation stamp, id.
#[cfg(feature = "db-postgres")]
const SET_CREATED_AT: &str = "UPDATE arcature_notifications SET created_at = $1 WHERE id = $2";
/// Binds: new creation stamp, id.
#[cfg(not(feature = "db-postgres"))]
const SET_CREATED_AT: &str = "UPDATE arcature_notifications SET created_at = ? WHERE id = ?";

/// Binds: new read stamp, id.
#[cfg(feature = "db-postgres")]
const SET_READ_AT: &str = "UPDATE arcature_notifications SET read_at = $1 WHERE id = $2";
/// Binds: new read stamp, id.
#[cfg(not(feature = "db-postgres"))]
const SET_READ_AT: &str = "UPDATE arcature_notifications SET read_at = ? WHERE id = ?";

/// A migrated, empty `arcature_notifications`, held exclusively until
/// dropped.
struct Fixture {
    store: DatabaseNotifications,
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn store(&self) -> &DatabaseNotifications {
        &self.store
    }

    fn pool(&self) -> &NotificationPool {
        self.store.pool()
    }

    /// Every stored id, read state ignored.
    ///
    /// Read as keys rather than counted, because `COUNT(*)` decodes to a
    /// different width on MySQL and a fixture failing to decode its own
    /// bookkeeping would look exactly like the store misbehaving.
    async fn ids(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM arcature_notifications")
            .fetch_all(self.pool())
            .await
            .expect("read arcature_notifications")
    }

    async fn rows(&self) -> usize {
        self.ids().await.len()
    }

    /// Write a row with an id of the caller's choosing.
    ///
    /// `store` draws its own, which is right for an inbox and useless for
    /// testing an `ORDER BY` tiebreak on the id. This uses the store's own
    /// `INSERT_NEW`, so what it proves about the insert is what the store
    /// does -- including how each dialect reports an id already taken.
    async fn insert_raw(
        &self,
        id: [u8; ID_BYTES],
        notifiable_key: &str,
        kind: &str,
        created_at: DateTime<Utc>,
    ) -> u64 {
        sqlx::query(sql::INSERT_NEW)
            .bind(id.to_vec())
            .bind(notifiable_key)
            .bind(kind)
            .bind(Json(json!({})))
            .bind(stored_time(created_at))
            .execute(self.pool())
            .await
            .expect("insert a notification by hand")
            .rows_affected()
    }

    /// Move one notification's creation stamp, so ordering can be tested
    /// without waiting between writes.
    async fn set_created_at(&self, id: NotificationId, at: DateTime<Utc>) {
        let affected = sqlx::query(SET_CREATED_AT)
            .bind(stored_time(at))
            .bind(id.as_bytes().to_vec())
            .execute(self.pool())
            .await
            .expect("move the creation stamp")
            .rows_affected();
        assert_eq!(affected, 1, "the notification to re-date was not there");
    }

    /// Move one notification's read stamp, so retention can be tested without
    /// waiting out a cutoff.
    async fn set_read_at(&self, id: NotificationId, at: DateTime<Utc>) {
        let affected = sqlx::query(SET_READ_AT)
            .bind(stored_time(at))
            .bind(id.as_bytes().to_vec())
            .execute(self.pool())
            .await
            .expect("move the read stamp")
            .rows_affected();
        assert_eq!(affected, 1, "the notification to re-date was not there");
    }

    /// One recipient's inbox, whole.
    async fn inbox(&self, notifiable_key: &str) -> Vec<StoredNotification> {
        self.store()
            .inbox(notifiable_key, PAGE)
            .await
            .expect("inbox")
    }

    /// One recipient's unread notifications, whole.
    async fn unread(&self, notifiable_key: &str) -> Vec<StoredNotification> {
        self.store()
            .unread(notifiable_key, PAGE)
            .await
            .expect("unread")
    }
}

/// A migrated, empty store, or `None` when this machine has no test database.
///
/// # Panics
///
/// Panics when a database is configured but unusable, and when none is
/// configured while `ARCATURE_REQUIRE_TEST_DB` says one was promised.
async fn notifications() -> Option<Fixture> {
    use crate::test_kit::database::{
        REQUIRE_TEST_DB_VAR, TEST_DB_URL_VAR, TestDatabaseError, test_database_required,
        test_database_url,
    };

    let url = match test_database_url() {
        Ok(url) => url,
        Err(TestDatabaseError::NotConfigured) => {
            assert!(
                !test_database_required(),
                "{REQUIRE_TEST_DB_VAR} is set, so {TEST_DB_URL_VAR} has to be too"
            );
            return None;
        }
        Err(error) => panic!("{error}"),
    };

    let exclusive = TABLE.lock().await;

    let pool = sqlx::pool::PoolOptions::<crate::database::Driver>::new()
        .max_connections(CONNECTIONS)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect to the test database: {error}"));

    let store = DatabaseNotifications::new(pool);
    store
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("migrate arcature_notifications: {error}"));
    sqlx::query("DELETE FROM arcature_notifications")
        .execute(store.pool())
        .await
        .unwrap_or_else(|error| panic!("empty arcature_notifications: {error}"));

    Some(Fixture {
        store,
        _exclusive: exclusive,
    })
}

/// Run `body` with a fixture, or return quietly when there is no database.
macro_rules! with_notifications {
    (|$fixture:ident| $body:block) => {
        let Some($fixture) = notifications().await else {
            return;
        };
        $body
    };
}

/// Assert two instants are the same one, allowing for SQLite's millisecond
/// storage.
fn same_instant(left: DateTime<Utc>, right: DateTime<Utc>, what: &str) {
    assert!(
        (left - right).abs() <= SLACK,
        "{what}: {left} and {right} are more than {SLACK:?} apart"
    );
}

/// The kinds in a listing, in the order the listing returned them.
fn kinds(listed: &[StoredNotification]) -> Vec<&str> {
    listed.iter().map(StoredNotification::kind).collect()
}

#[tokio::test]
async fn a_stored_notification_comes_back_out_of_the_inbox() {
    with_notifications!(|fixture| {
        let content = DatabaseContent::new("invoice.paid", json!({"amount": 1200}));
        let stored = fixture
            .store()
            .store("user:42", &content)
            .await
            .expect("store");

        assert_eq!(stored.notifiable_key(), "user:42");
        assert_eq!(stored.kind(), "invoice.paid");
        assert!(stored.read_at().is_none());
        assert!(!stored.is_read());

        let listed = fixture.inbox("user:42").await;
        assert_eq!(listed.len(), 1);
        let read_back = &listed[0];
        assert_eq!(read_back.id(), stored.id());
        assert_eq!(read_back.notifiable_key(), "user:42");
        assert_eq!(read_back.kind(), "invoice.paid");
        assert_eq!(read_back.data(), &json!({"amount": 1200}));
        assert!(read_back.read_at().is_none());
        same_instant(read_back.created_at(), stored.created_at(), "created_at");
    });
}

#[tokio::test]
async fn the_payload_survives_the_round_trip() {
    // The one column whose storage type varies by dialect: JSONB, JSON, and
    // TEXT, all decoded through `sqlx::types::Json`. Nesting, an array, an
    // explicit null, a float, a boolean and a non-ASCII string are here
    // because each is a shape a lax encoder mangles differently, and because
    // PostgreSQL's JSONB is the one of the three that does not preserve key
    // order -- so the assertion is `serde_json::Value` equality, which does
    // not depend on it.
    with_notifications!(|fixture| {
        let payload = json!({
            "invoice": {"id": "INV-7", "total": 12.5, "paid": true},
            "lines": [1, 2, 3],
            "note": "p\u{159}\u{ed}li\u{161} \u{17e}lu\u{165}ou\u{10d}k\u{fd}",
            "cancelled_at": null,
        });
        let stored = fixture
            .store()
            .store(
                "user:42",
                &DatabaseContent::new("invoice.paid", payload.clone()),
            )
            .await
            .expect("store");
        assert_eq!(stored.data(), &payload, "the value store returned");

        let listed = fixture.inbox("user:42").await;
        assert_eq!(listed[0].data(), &payload, "the value the database held");
    });
}

#[tokio::test]
async fn the_inbox_is_newest_first_and_ties_break_on_id() {
    // `ORDER BY created_at DESC, id`. The tiebreak is not decoration: two
    // notifications written in the same millisecond are ordinary, and without
    // it the page a user sees would reshuffle between requests. `store` draws
    // its own id and stamps its own clock, so the rows that make the order
    // observable are written by hand.
    with_notifications!(|fixture| {
        let now = Utc::now();
        // Deliberately inserted out of order.
        fixture
            .insert_raw(
                [3u8; ID_BYTES],
                "user:42",
                "oldest",
                now - TimeDelta::seconds(2),
            )
            .await;
        fixture
            .insert_raw([1u8; ID_BYTES], "user:42", "newest-low-id", now)
            .await;
        fixture
            .insert_raw(
                [2u8; ID_BYTES],
                "user:42",
                "middle",
                now - TimeDelta::seconds(1),
            )
            .await;
        fixture
            .insert_raw([4u8; ID_BYTES], "user:42", "newest-high-id", now)
            .await;

        assert_eq!(
            kinds(&fixture.inbox("user:42").await),
            ["newest-low-id", "newest-high-id", "middle", "oldest"]
        );
        // The unread listing is the same statement plus one predicate, and
        // has to agree: nothing here has been read.
        assert_eq!(
            kinds(&fixture.unread("user:42").await),
            ["newest-low-id", "newest-high-id", "middle", "oldest"]
        );
    });
}

#[tokio::test]
async fn the_inbox_is_scoped_to_one_recipient_and_honours_the_limit() {
    // The limit is why there is no unbounded read: an inbox render must not
    // be able to pull an account's entire history into memory. Taken with the
    // ordering, "the newest three" is also the only useful meaning a limit
    // could have here, so both are asserted at once.
    with_notifications!(|fixture| {
        let now = Utc::now();
        for (index, id) in [(0i64, 1u8), (1, 2), (2, 3), (3, 4)] {
            fixture
                .insert_raw(
                    [id; ID_BYTES],
                    "user:42",
                    &format!("mine-{index}"),
                    now - TimeDelta::seconds(index),
                )
                .await;
        }
        fixture
            .insert_raw([9u8; ID_BYTES], "user:7", "theirs", now)
            .await;

        assert_eq!(
            kinds(&fixture.store().inbox("user:42", 3).await.expect("inbox")),
            ["mine-0", "mine-1", "mine-2"],
            "the limit must take the newest, not an arbitrary three"
        );
        assert_eq!(kinds(&fixture.inbox("user:7").await), ["theirs"]);
        assert!(fixture.inbox("user:nobody").await.is_empty());
        assert_eq!(fixture.rows().await, 5);
    });
}

#[tokio::test]
async fn the_badge_agrees_with_the_unread_listing() {
    // `COUNT_UNREAD` and `LIST_UNREAD` are two statements answering one
    // question, and the badge is read on far more page loads than the inbox
    // is opened -- so they are allowed to differ in cost and never in answer.
    with_notifications!(|fixture| {
        let mut ids = Vec::new();
        for index in 0..3 {
            ids.push(
                fixture
                    .store()
                    .store(
                        "user:42",
                        &DatabaseContent::new(format!("kind-{index}"), json!({})),
                    )
                    .await
                    .expect("store")
                    .id(),
            );
        }
        fixture
            .store()
            .store("user:7", &DatabaseContent::new("theirs", json!({})))
            .await
            .expect("store");

        assert_eq!(
            fixture
                .store()
                .unread_count("user:42")
                .await
                .expect("count"),
            3
        );
        assert_eq!(fixture.unread("user:42").await.len(), 3);

        assert!(
            fixture
                .store()
                .mark_read("user:42", ids[0])
                .await
                .expect("mark read")
        );
        assert_eq!(
            fixture
                .store()
                .unread_count("user:42")
                .await
                .expect("count"),
            2
        );
        assert_eq!(fixture.unread("user:42").await.len(), 2);
        assert_eq!(
            fixture.inbox("user:42").await.len(),
            3,
            "still in the inbox"
        );

        // The other recipient's badge never moved.
        assert_eq!(
            fixture.store().unread_count("user:7").await.expect("count"),
            1
        );

        let marked = fixture
            .store()
            .mark_all_read("user:42")
            .await
            .expect("mark all read");
        assert_eq!(marked, 2, "the one already read is not marked twice");
        assert_eq!(
            fixture
                .store()
                .unread_count("user:42")
                .await
                .expect("count"),
            0
        );
        assert!(fixture.unread("user:42").await.is_empty());
        assert_eq!(
            fixture.store().unread_count("user:7").await.expect("count"),
            1
        );
        // And a recipient with nothing has a badge of zero rather than an
        // error -- a count over no rows still returns a row.
        assert_eq!(
            fixture
                .store()
                .unread_count("user:nobody")
                .await
                .expect("count"),
            0
        );
    });
}

#[tokio::test]
async fn marking_read_twice_keeps_the_first_receipt() {
    // `read_at IS NULL` in the predicate is what makes "when did they first
    // see this" survive a second click. Without it a re-render of the inbox
    // would quietly overwrite the receipt every time.
    with_notifications!(|fixture| {
        let stored = fixture
            .store()
            .store("user:42", &DatabaseContent::new("invoice.paid", json!({})))
            .await
            .expect("store");

        assert!(
            fixture
                .store()
                .mark_read("user:42", stored.id())
                .await
                .expect("mark read")
        );
        let first = fixture.inbox("user:42").await[0]
            .read_at()
            .expect("read_at is set once it is read");

        // Backdate the receipt so a second mark overwriting it would be
        // unmissable rather than a sub-millisecond difference.
        fixture
            .set_read_at(stored.id(), first - TimeDelta::seconds(3600))
            .await;
        let backdated = fixture.inbox("user:42").await[0]
            .read_at()
            .expect("still read");

        assert!(
            !fixture
                .store()
                .mark_read("user:42", stored.id())
                .await
                .expect("mark read"),
            "a second marking has nothing to mark"
        );
        let after = fixture.inbox("user:42").await[0]
            .read_at()
            .expect("still read");
        same_instant(after, backdated, "read_at after a second marking");
        assert!(fixture.unread("user:42").await.is_empty());
    });
}

#[tokio::test]
async fn marking_read_reaches_one_row_and_only_its_owner() {
    // Every statement here carries `notifiable_key`, so an id lifted from
    // somebody else's inbox matches nothing. `false` is the same answer as
    // "no such notification" on purpose: the alternative is an oracle for
    // which ids exist.
    with_notifications!(|fixture| {
        let mine = fixture
            .store()
            .store("user:42", &DatabaseContent::new("mine", json!({})))
            .await
            .expect("store");
        fixture
            .store()
            .store("user:42", &DatabaseContent::new("also mine", json!({})))
            .await
            .expect("store");
        let theirs = fixture
            .store()
            .store("user:7", &DatabaseContent::new("theirs", json!({})))
            .await
            .expect("store");

        assert!(
            !fixture
                .store()
                .mark_read("user:42", theirs.id())
                .await
                .expect("mark read"),
            "a foreign id must not match"
        );
        assert!(
            !fixture
                .store()
                .mark_read("user:42", NotificationId::from_bytes([0u8; ID_BYTES]))
                .await
                .expect("mark read"),
            "an id nobody minted must not match"
        );
        assert_eq!(
            fixture.store().unread_count("user:7").await.expect("count"),
            1
        );

        assert!(
            fixture
                .store()
                .mark_read("user:42", mine.id())
                .await
                .expect("mark read")
        );
        assert_eq!(
            kinds(&fixture.unread("user:42").await),
            ["also mine"],
            "only the named row was marked"
        );
    });
}

#[tokio::test]
async fn deleting_reaches_one_row_and_only_its_owner() {
    with_notifications!(|fixture| {
        let mine = fixture
            .store()
            .store("user:42", &DatabaseContent::new("mine", json!({})))
            .await
            .expect("store");
        let theirs = fixture
            .store()
            .store("user:7", &DatabaseContent::new("theirs", json!({})))
            .await
            .expect("store");

        assert!(
            !fixture
                .store()
                .delete("user:42", theirs.id())
                .await
                .expect("delete"),
            "a foreign id must not match"
        );
        assert_eq!(fixture.rows().await, 2, "the foreign row survived");

        assert!(
            fixture
                .store()
                .delete("user:42", mine.id())
                .await
                .expect("delete")
        );
        assert!(
            !fixture
                .store()
                .delete("user:42", mine.id())
                .await
                .expect("delete"),
            "a second delete has nothing to delete"
        );
        assert_eq!(fixture.ids().await, vec![theirs.id().as_bytes().to_vec()]);
    });
}

#[tokio::test]
async fn clearing_an_inbox_leaves_every_other_inbox() {
    with_notifications!(|fixture| {
        for index in 0..3 {
            fixture
                .store()
                .store(
                    "user:42",
                    &DatabaseContent::new(format!("kind-{index}"), json!({})),
                )
                .await
                .expect("store");
        }
        let theirs = fixture
            .store()
            .store("user:7", &DatabaseContent::new("theirs", json!({})))
            .await
            .expect("store");

        let cleared = fixture
            .store()
            .delete_all_for("user:42")
            .await
            .expect("delete all");
        assert_eq!(cleared, 3);
        assert!(fixture.inbox("user:42").await.is_empty());
        assert_eq!(fixture.ids().await, vec![theirs.id().as_bytes().to_vec()]);

        // Clearing an inbox that is already empty is a no-op, not an error.
        assert_eq!(
            fixture
                .store()
                .delete_all_for("user:42")
                .await
                .expect("delete all"),
            0
        );
    });
}

#[tokio::test]
async fn pruning_reaches_read_rows_in_every_inbox_and_never_an_unread_one() {
    // `DELETE_READ_BEFORE` names no recipient, and that is deliberate rather
    // than an omission: retention is an operator's decision about the whole
    // table, and it is safe to be unscoped precisely because it cannot reach
    // anything a recipient has not already seen. Both halves of that are
    // asserted here -- it crosses inboxes, and an unread row survives however
    // old it is.
    with_notifications!(|fixture| {
        let now = Utc::now();
        let cutoff = now - TimeDelta::seconds(3600);

        let old_mine = fixture
            .store()
            .store("user:42", &DatabaseContent::new("old and read", json!({})))
            .await
            .expect("store");
        let old_theirs = fixture
            .store()
            .store(
                "user:7",
                &DatabaseContent::new("theirs, old and read", json!({})),
            )
            .await
            .expect("store");
        let recently_read = fixture
            .store()
            .store(
                "user:42",
                &DatabaseContent::new("read after the cutoff", json!({})),
            )
            .await
            .expect("store");
        let ancient_unread = fixture
            .store()
            .store(
                "user:42",
                &DatabaseContent::new("ancient and unread", json!({})),
            )
            .await
            .expect("store");

        // Two read long before the cutoff, one read after it, one never read
        // and older than any of them.
        fixture
            .set_read_at(old_mine.id(), cutoff - TimeDelta::seconds(60))
            .await;
        fixture
            .set_read_at(old_theirs.id(), cutoff - TimeDelta::seconds(60))
            .await;
        fixture
            .set_read_at(recently_read.id(), cutoff + TimeDelta::seconds(60))
            .await;
        fixture
            .set_created_at(ancient_unread.id(), cutoff - TimeDelta::seconds(86_400))
            .await;

        let pruned = fixture
            .store()
            .prune_read_before(cutoff)
            .await
            .expect("prune");
        assert_eq!(
            pruned, 2,
            "the sweep crosses inboxes: one row of user:42's and one of user:7's"
        );

        let mut left = fixture.ids().await;
        left.sort_unstable();
        let mut expected = vec![
            recently_read.id().as_bytes().to_vec(),
            ancient_unread.id().as_bytes().to_vec(),
        ];
        expected.sort_unstable();
        assert_eq!(left, expected);
        assert!(
            fixture.inbox("user:7").await.is_empty(),
            "the other recipient's read row went too"
        );
        assert_eq!(
            kinds(&fixture.unread("user:42").await),
            ["ancient and unread"],
            "an unread row survives however old it is"
        );
    });
}

#[tokio::test]
async fn an_id_that_is_already_taken_is_reported_as_zero_rows_rather_than_an_error() {
    // What `store`'s retry loop is built on. `ON CONFLICT DO NOTHING`,
    // `INSERT IGNORE`, and `INSERT OR IGNORE` are three different statements
    // that must all report a clash the same way -- as zero rows affected --
    // because the alternative is parsing a driver-specific constraint name
    // out of an error, which is exactly the code this store does not have.
    with_notifications!(|fixture| {
        let now = Utc::now();
        assert_eq!(
            fixture
                .insert_raw([7u8; ID_BYTES], "user:42", "first", now)
                .await,
            1
        );
        assert_eq!(
            fixture
                .insert_raw([7u8; ID_BYTES], "user:7", "second", now)
                .await,
            0,
            "a taken id must be reported as zero rows, not raised as an error"
        );
        // And the clash left the first row exactly as it was.
        assert_eq!(kinds(&fixture.inbox("user:42").await), ["first"]);
        assert!(fixture.inbox("user:7").await.is_empty());
    });
}

#[tokio::test]
async fn migrating_twice_is_a_no_op() {
    // The fixture already migrated once. The second run has to find its own
    // history row and do nothing -- which is a different question per
    // dialect, because MySQL declares its indexes inside `CREATE TABLE` for
    // want of `CREATE INDEX IF NOT EXISTS` and PostgreSQL takes an advisory
    // lock it must also release.
    with_notifications!(|fixture| {
        let stored = fixture
            .store()
            .store("user:42", &DatabaseContent::new("before", json!({"n": 1})))
            .await
            .expect("store");

        fixture.store().migrate().await.expect("migrate again");

        assert_eq!(fixture.rows().await, 1, "the second run kept the row");
        let listed = fixture.inbox("user:42").await;
        assert_eq!(listed[0].id(), stored.id());
        assert_eq!(listed[0].data(), &json!({"n": 1}));
        fixture
            .store()
            .store("user:42", &DatabaseContent::new("after", json!({})))
            .await
            .expect("store after a second migration");
        assert_eq!(fixture.rows().await, 2);
    });
}
