//! Round-trip tests against a live database.
//!
//! Save, load, delete, expiry, and the sweep -- all of it against a real
//! server rather than a mock, because every property this store claims is a
//! property of the SQL. An in-memory double would agree with whatever the
//! Rust believes and prove nothing about `ON CONFLICT`, `REPLACE INTO`, or
//! whether `expires_at > now()` compares the two values the store thinks it
//! is comparing.
//!
//! # Why these tests need `test-kit`
//!
//! "Skip, or fail because CI promised a database" is a decision that must
//! never be wrong in the lenient direction: get it wrong and a green run
//! means nothing was tested. [`crate::test_kit::database`] already owns that
//! decision, together with the check that refuses to write to any database
//! whose name is not prefixed `arcature_test_`. Re-spelling either rule here
//! to save a feature flag would leave two copies of something that must never
//! disagree, so these tests are gated on `test-kit` and call the original.
//!
//! The consequence is that `cargo test` on the default features does not
//! compile them. `just db-test` and CI's `Database` matrix both enable
//! `test-kit` and set `ARCATURE_REQUIRE_TEST_DB`, which turns the skip below
//! into a failure -- so a leg whose database service never came up cannot
//! skip its way to a pass. SQLite needs no server, so one of the three
//! dialects is always runnable on a laptop too.
//!
//! # Why one test at a time
//!
//! There is one `arcature_sessions` table, and the sweep is deliberately
//! blind to whose rows it deletes -- that is the whole of what a sweep is.
//! Two tests running concurrently would delete each other's sessions and both
//! would be right to fail. The fixture hands out an exclusive lock for the
//! duration of a test and starts each one from an empty table, so a row count
//! is an answer rather than a guess.

use std::collections::HashMap;
use std::time::Duration as StdDuration;

use time::{Duration, OffsetDateTime};
use tokio::sync::{Mutex, MutexGuard};
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{ExpiredDeletion, SessionStore};

use super::DbSessionStore;
use super::dialect::SessionPool;

/// Serialises the database tests against the one shared table. See the module
/// comment.
static TABLE: Mutex<()> = Mutex::const_new(());

/// Pool size. Above one so a test cannot pass because the pool serialised it.
const CONNECTIONS: u32 = 4;

/// A migrated, empty `arcature_sessions`, held exclusively until dropped.
struct Fixture {
    store: DbSessionStore,
    /// Dropped with the fixture, which is what releases the next test.
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn store(&self) -> &DbSessionStore {
        &self.store
    }

    fn pool(&self) -> &SessionPool {
        self.store.pool()
    }

    /// Every stored row key, expiry ignored.
    ///
    /// Ignoring expiry is the point: it is what distinguishes "the query
    /// refused to return an expired session" from "the row is gone".
    ///
    /// The keys are read rather than counted with `COUNT(*)` because the
    /// width of a count differs by dialect -- MySQL hands back an unsigned
    /// bigint -- and a fixture that fails to decode its own bookkeeping query
    /// would look exactly like the store misbehaving. `.len()` answers the
    /// counting questions, and the keys themselves answer the one about what
    /// is actually written.
    async fn keys(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM arcature_sessions")
            .fetch_all(self.pool())
            .await
            .expect("read arcature_sessions")
    }

    /// How many rows the table holds.
    async fn rows(&self) -> usize {
        self.keys().await.len()
    }
}

/// A migrated, empty store, or `None` when this machine has no test database.
///
/// # Panics
///
/// Panics when a database is configured but unusable -- an unsafe name, a
/// refused connection, a migration the server rejects -- and when none is
/// configured while `ARCATURE_REQUIRE_TEST_DB` says one was promised.
async fn sessions() -> Option<Fixture> {
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
        // A database that is present but wrong is never a reason to skip.
        Err(error) => panic!("{error}"),
    };

    // Taken before the pool is opened, so a test that fails while holding it
    // cannot leave a half-built fixture visible to the next one.
    let exclusive = TABLE.lock().await;

    let pool = sqlx::pool::PoolOptions::<crate::database::Driver>::new()
        .max_connections(CONNECTIONS)
        .acquire_timeout(StdDuration::from_secs(30))
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect to the test database: {error}"));

    let store = DbSessionStore::new(pool);
    store
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("migrate arcature_sessions: {error}"));
    // Not `TRUNCATE`: PostgreSQL and MySQL have it, SQLite does not, and the
    // table is never big enough for the difference to matter.
    sqlx::query("DELETE FROM arcature_sessions")
        .execute(store.pool())
        .await
        .unwrap_or_else(|error| panic!("empty arcature_sessions: {error}"));

    Some(Fixture {
        store,
        _exclusive: exclusive,
    })
}

/// A session carrying one recognisable value, expiring `in_` from now.
///
/// A negative duration is how the expiry tests get a session that is already
/// dead without sleeping through its lifetime.
fn record(in_: Duration) -> Record {
    let mut data = HashMap::new();
    data.insert("user_id".to_owned(), serde_json::json!(7));
    data.insert("flash".to_owned(), serde_json::json!("saved"));
    Record {
        id: Id::default(),
        data,
        expiry_date: OffsetDateTime::now_utc() + in_,
    }
}

#[tokio::test]
async fn a_saved_session_comes_back_with_its_data() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let record = record(Duration::hours(1));

    fixture.store().save(&record).await.expect("save");
    let loaded = fixture
        .store()
        .load(&record.id)
        .await
        .expect("load")
        .expect("the session was saved a moment ago");

    assert_eq!(loaded.id, record.id);
    assert_eq!(loaded.data, record.data);
    // Not an exact equality: SQLite stores milliseconds, so a sub-millisecond
    // difference is the storage doing what it says it does rather than a bug.
    let drift = loaded.expiry_date - record.expiry_date;
    assert!(
        drift.abs() < Duration::milliseconds(2),
        "expiry came back as {} rather than {}",
        loaded.expiry_date,
        record.expiry_date
    );
}

#[tokio::test]
async fn create_moves_off_an_id_that_is_already_taken() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let mut first = record(Duration::hours(1));
    fixture.store().create(&mut first).await.expect("create");
    let taken = first.id;

    // The same id, a different session. `create` must not overwrite: the
    // holder of `taken` is a logged-in user, and handing their row to
    // somebody else would hand over their session.
    let mut second = record(Duration::hours(1));
    second.id = taken;
    second
        .data
        .insert("user_id".to_owned(), serde_json::json!(9));
    fixture.store().create(&mut second).await.expect("create");

    assert_ne!(second.id, taken, "create reused a taken id");
    assert_eq!(
        fixture.rows().await,
        2,
        "one of the two sessions is missing"
    );
    let survivor = fixture
        .store()
        .load(&taken)
        .await
        .expect("load")
        .expect("the first session must still be there");
    assert_eq!(survivor.data, first.data, "create overwrote the first row");
}

#[tokio::test]
async fn saving_the_same_id_twice_updates_the_one_row() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let mut record = record(Duration::hours(1));
    fixture.store().save(&record).await.expect("first save");

    record
        .data
        .insert("user_id".to_owned(), serde_json::json!(11));
    fixture.store().save(&record).await.expect("second save");

    assert_eq!(fixture.rows().await, 1, "the upsert inserted a second row");
    let loaded = fixture
        .store()
        .load(&record.id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.data, record.data);
}

#[tokio::test]
async fn deleting_a_session_makes_it_unloadable() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let record = record(Duration::hours(1));
    fixture.store().save(&record).await.expect("save");

    fixture.store().delete(&record.id).await.expect("delete");

    assert_eq!(fixture.rows().await, 0, "the row survived the delete");
    assert!(
        fixture
            .store()
            .load(&record.id)
            .await
            .expect("load")
            .is_none(),
        "a deleted session still loads"
    );
    // Deleting again is not an error: a logout that is retried, or two tabs
    // logging out at once, must not produce a 500.
    fixture
        .store()
        .delete(&record.id)
        .await
        .expect("deleting twice");
}

#[tokio::test]
async fn an_expired_session_does_not_load_even_though_its_row_is_still_there() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let record = record(Duration::minutes(-5));
    fixture.store().save(&record).await.expect("save");

    // The whole point: no sweep has run, the row is on disk, and the session
    // is already gone. Expiry is enforced by the query, so a cleanup task
    // that is late, misconfigured, or absent costs disk and not security.
    assert_eq!(fixture.rows().await, 1, "the row should still be on disk");
    assert!(
        fixture
            .store()
            .load(&record.id)
            .await
            .expect("load")
            .is_none(),
        "an expired session loaded"
    );
}

#[tokio::test]
async fn the_sweep_deletes_expired_rows_and_leaves_live_ones() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let expired = record(Duration::minutes(-5));
    let live = record(Duration::hours(1));
    fixture.store().save(&expired).await.expect("save expired");
    fixture.store().save(&live).await.expect("save live");

    let deleted = fixture.store().sweep_expired().await.expect("sweep");

    assert_eq!(deleted, 1, "the sweep took the wrong number of rows");
    assert_eq!(fixture.rows().await, 1);
    assert!(
        fixture
            .store()
            .load(&live.id)
            .await
            .expect("load")
            .is_some(),
        "the sweep took a live session"
    );

    // The trait method `tower_sessions` calls is the same sweep, and running
    // it against a table with nothing to delete is not an error.
    fixture
        .store()
        .delete_expired()
        .await
        .expect("delete_expired");
    assert_eq!(fixture.rows().await, 1);
}

#[tokio::test]
async fn the_row_key_is_a_digest_rather_than_the_session_id() {
    let Some(fixture) = sessions().await else {
        return;
    };
    let record = record(Duration::hours(1));
    fixture.store().save(&record).await.expect("save");

    let keys = fixture.keys().await;

    assert_eq!(keys.len(), 1);
    let stored = &keys[0];
    assert_eq!(stored.len(), 32, "the stored key is not a SHA-256 digest");
    assert_ne!(
        stored.as_slice(),
        &record.id.0.to_le_bytes()[..],
        "the session id itself reached the database"
    );
    // And a dump of the key is not a cookie: it does not even decode to one,
    // because it is twice the length of an id.
    assert_ne!(stored.len(), record.id.0.to_le_bytes().len());
}
