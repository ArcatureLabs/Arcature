//! Round-trip tests against a live database.
//!
//! Issue, consume, revoke, sweep -- against a real server, because every
//! property this store claims is a property of the SQL rather than of the
//! Rust around it. The link is single-use because `DELETE ... WHERE subject`
//! affects one row for exactly one of two racing callers; it expires because
//! `expires_at > now()` is a predicate the *database* evaluates, against a
//! column three dialects store in three different shapes. An in-memory double
//! would agree with whatever the Rust believes and prove none of that.
//!
//! Two of these could not be written any other way. The one that races two
//! redemptions of the same link is asking whether the database serialises the
//! delete, which is a question only a database can answer. The one that
//! presents an id the insert already holds is asking whether `ON CONFLICT DO
//! NOTHING` / `INSERT IGNORE` / `INSERT OR IGNORE` really report a clash as
//! zero rows rather than as an error -- the assumption
//! [`PasswordResets::issue`]'s retry loop is built on, and one that is a
//! different statement in each dialect.
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
//! There is one `arcature_password_resets` table, and both the sweep and the
//! row counting are blind to whose rows they are -- which is the whole of
//! what a sweep does. Two tests running concurrently would delete each
//! other's links and both would be right to fail.

use std::time::Duration;

use chrono::Utc;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, MutexGuard};

use super::dialect::{ResetPool, sql, stored_time};
use super::store::PasswordResets;
use super::token::{ID_BYTES, SECRET_BYTES, format_plaintext, parse_plaintext};

/// Serialises the database tests against the one shared table.
static TABLE: Mutex<()> = Mutex::const_new(());

/// Pool size. Above one so the concurrency test cannot pass because the pool
/// serialised it -- which would prove the pool works, not the SQL.
const CONNECTIONS: u32 = 4;

/// An hour, the ordinary lifetime for one of these.
const HOUR: Duration = Duration::from_secs(60 * 60);

// The statement below re-dates a row so that expiry can be tested in
// milliseconds rather than by sleeping through an hour. It is spelled per
// dialect, and as a literal rather than built with
// `crate::database::dialect::placeholder`, for the reason SQLx's `SqlSafeStr`
// bound exists: a `String` assembled at runtime has to be waved past that gate
// with `AssertSqlSafe`, and a test fixture is the last place that escape hatch
// should be normalised. Two lines of duplication is the cheaper half of that
// trade. Two spellings and not three, because SQLite and MySQL agree on `?`.

/// Binds: new deadline, id.
#[cfg(feature = "db-postgres")]
const SET_EXPIRY: &str = "UPDATE arcature_password_resets SET expires_at = $1 WHERE id = $2";
/// Binds: new deadline, id.
#[cfg(not(feature = "db-postgres"))]
const SET_EXPIRY: &str = "UPDATE arcature_password_resets SET expires_at = ? WHERE id = ?";

/// A migrated, empty `arcature_password_resets`, held exclusively until
/// dropped.
struct Fixture {
    store: PasswordResets,
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn store(&self) -> &PasswordResets {
        &self.store
    }

    fn pool(&self) -> &ResetPool {
        self.store.pool()
    }

    /// Every stored id, expiry ignored.
    ///
    /// Ignoring expiry is the point: it distinguishes "the query refused to
    /// return a lapsed link" from "the row is gone". Read as keys rather than
    /// counted, because `COUNT(*)` decodes to a different width on MySQL and
    /// a fixture failing to decode its own bookkeeping would look exactly
    /// like the store misbehaving.
    async fn ids(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM arcature_password_resets")
            .fetch_all(self.pool())
            .await
            .expect("read arcature_password_resets")
    }

    async fn rows(&self) -> usize {
        self.ids().await.len()
    }

    /// Move one link's deadline to `at`, so expiry can be tested without
    /// sleeping through an hour.
    async fn set_expiry(&self, plaintext: &str, at: chrono::DateTime<Utc>) {
        let (id, _) = parse_plaintext(plaintext).expect("a link this crate minted");
        let affected = sqlx::query(SET_EXPIRY)
            .bind(stored_time(at))
            .bind(id.as_bytes().to_vec())
            .execute(self.pool())
            .await
            .expect("move the deadline")
            .rows_affected();
        assert_eq!(affected, 1, "the link to re-date was not there");
    }

    /// Write a row the store's own `issue` would never leave behind -- a
    /// second live link for a subject that already has one.
    ///
    /// This uses the store's `INSERT_NEW`, not a statement of its own, so
    /// what it proves about the insert is what the store does. Returns the
    /// rows affected, which is how the caller sees a clash: the three
    /// dialects each spell "ignore the conflict" differently and all three
    /// must report it as zero rather than as an error.
    async fn insert_extra(&self, id: [u8; 16], subject: &str) -> u64 {
        let expires_at = Utc::now() + chrono::TimeDelta::seconds(3600);
        sqlx::query(sql::INSERT_NEW)
            .bind(id.to_vec())
            .bind(vec![0u8; 32])
            .bind(subject)
            .bind(stored_time(expires_at))
            .bind(stored_time(Utc::now()))
            .execute(self.pool())
            .await
            .expect("insert a second link by hand")
            .rows_affected()
    }
}

/// A migrated, empty store, or `None` when this machine has no test database.
///
/// # Panics
///
/// Panics when a database is configured but unusable, and when none is
/// configured while `ARCATURE_REQUIRE_TEST_DB` says one was promised.
async fn resets() -> Option<Fixture> {
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

    let store = PasswordResets::new(pool);
    store
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("migrate arcature_password_resets: {error}"));
    sqlx::query("DELETE FROM arcature_password_resets")
        .execute(store.pool())
        .await
        .unwrap_or_else(|error| panic!("empty arcature_password_resets: {error}"));

    Some(Fixture {
        store,
        _exclusive: exclusive,
    })
}

/// Run `body` with a fixture, or return quietly when there is no database.
macro_rules! with_resets {
    (|$fixture:ident| $body:block) => {
        let Some($fixture) = resets().await else {
            return;
        };
        $body
    };
}

/// Splice the public half of one link onto the secret half of another.
///
/// Both halves are spellings the encoder itself produced, so the result
/// parses -- which is the point. A string that failed `parse_plaintext` would
/// never reach the database and would prove nothing about the statement.
fn with_foreign_secret(id_from: &str, secret_from: &str) -> String {
    let (id_half, _) = id_from.split_once('.').expect("a link this crate minted");
    let (_, secret_half) = secret_from
        .split_once('.')
        .expect("a link this crate minted");
    format!("{id_half}.{secret_half}")
}

#[tokio::test]
async fn a_freshly_issued_link_redeems_once_and_then_never_again() {
    // The property the whole flow rests on. The second redemption is not
    // "different", it is refused -- and the row is gone rather than flagged,
    // because a spent row left in the table is a row some future query can
    // forget to filter.
    with_resets!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue");
        assert_eq!(issued.subject(), "ada@example.test");
        let link = issued.plaintext().expose().to_owned();
        assert_eq!(fixture.rows().await, 1);

        let subject = fixture
            .store()
            .consume(&link)
            .await
            .expect("consume")
            .expect("a link just issued redeems");
        assert_eq!(subject, "ada@example.test");
        assert_eq!(fixture.rows().await, 0, "a spent link leaves no row");

        assert!(
            fixture
                .store()
                .consume(&link)
                .await
                .expect("consume")
                .is_none(),
            "a spent link must be refused, not merely different"
        );
    });
}

#[tokio::test]
async fn issuing_again_clears_the_link_the_subject_already_had() {
    // The documented departure from the remember-me store, pinned. Two live
    // reset links mean an old message in an old inbox stays armed after the
    // user asked for a new one, which is the opposite of what asking again
    // means.
    with_resets!(|fixture| {
        let first = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        let second = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        assert_ne!(first, second);
        assert_eq!(fixture.rows().await, 1, "one subject, one live link");

        assert!(
            fixture
                .store()
                .consume(&first)
                .await
                .expect("consume")
                .is_none(),
            "the superseded link must be dead"
        );
        assert_eq!(
            fixture
                .store()
                .consume(&second)
                .await
                .expect("consume")
                .as_deref(),
            Some("ada@example.test")
        );
    });
}

#[tokio::test]
async fn a_lapsed_link_is_refused_before_any_sweep_runs() {
    // Expiry is a predicate on every read, not a property of the sweep. A
    // deployment that never sweeps is wasteful, not insecure, and this is the
    // assertion that says so -- evaluated by the server's clock against the
    // column's own storage shape, which is a timestamp on two dialects and an
    // integer on the third.
    with_resets!(|fixture| {
        let link = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        fixture
            .set_expiry(&link, Utc::now() - chrono::TimeDelta::seconds(1))
            .await;

        assert!(
            fixture
                .store()
                .consume(&link)
                .await
                .expect("consume")
                .is_none(),
            "a lapsed link must not redeem"
        );
        // Still on disk: nothing deleted it, the query declined to see it.
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn a_wrong_secret_is_refused_and_does_not_spend_the_link() {
    // The order of the two statements, asserted rather than reviewed. The
    // delete runs only after the digest comparison succeeds; the other order
    // would let anyone who knows a victim's *public* id -- half of a link
    // seen over a shoulder or in a proxy log -- burn every reset the victim
    // requests.
    with_resets!(|fixture| {
        let real = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        let other = fixture
            .store()
            .issue("grace@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();

        let forged = with_foreign_secret(&real, &other);
        assert_ne!(forged, real, "the splice did not change anything");
        assert!(
            fixture
                .store()
                .consume(&forged)
                .await
                .expect("consume")
                .is_none(),
            "a wrong secret must not redeem"
        );
        assert_eq!(fixture.rows().await, 2, "a wrong guess deleted a row");

        // And the real link still works, which is the half that matters: the
        // failed guess cost its owner nothing.
        assert_eq!(
            fixture
                .store()
                .consume(&real)
                .await
                .expect("consume")
                .as_deref(),
            Some("ada@example.test")
        );
    });
}

#[tokio::test]
async fn an_id_nobody_ever_issued_reaches_no_row() {
    // A link from a database that was reset, or one this store never minted
    // at all. The string is well formed, so it costs a query: `FIND_LIVE` has
    // to come back empty rather than error, and the store has to answer with
    // the same `None` a wrong secret gets. The other rows are there to prove
    // the query is a lookup and not a match on "any live link".
    with_resets!(|fixture| {
        let real = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();

        let unknown = format_plaintext(&[0u8; ID_BYTES], &[0u8; SECRET_BYTES]);
        assert!(
            parse_plaintext(&unknown).is_some(),
            "the fixture link has to be well formed, or nothing is queried"
        );
        assert!(
            fixture
                .store()
                .consume(&unknown)
                .await
                .expect("consume")
                .is_none(),
            "an id with no row must be refused, not error"
        );

        assert_eq!(fixture.rows().await, 1);
        assert_eq!(
            fixture
                .store()
                .consume(&real)
                .await
                .expect("consume")
                .as_deref(),
            Some("ada@example.test")
        );
    });
}

#[tokio::test]
async fn revoking_clears_every_link_the_subject_has_and_nobody_elses() {
    // `issue` keeps a subject to one link, so the rows that make this test
    // interesting are written by hand. They are not hypothetical: revocation
    // is also what an account disablement calls, and a schema that allows
    // several rows per subject has to be swept by a statement that removes
    // all of them.
    with_resets!(|fixture| {
        fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue");
        assert_eq!(fixture.insert_extra([1u8; 16], "ada@example.test").await, 1);
        assert_eq!(fixture.insert_extra([2u8; 16], "ada@example.test").await, 1);
        let grace = fixture
            .store()
            .issue("grace@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        assert_eq!(fixture.rows().await, 4);

        let revoked = fixture
            .store()
            .revoke_all_for("ada@example.test")
            .await
            .expect("revoke all");
        assert_eq!(revoked, 3);
        assert_eq!(fixture.rows().await, 1);
        assert_eq!(
            fixture
                .store()
                .consume(&grace)
                .await
                .expect("consume")
                .as_deref(),
            Some("grace@example.test"),
            "the bystander's link should be untouched"
        );
    });
}

#[tokio::test]
async fn the_sweep_reclaims_lapsed_links_and_leaves_live_ones() {
    with_resets!(|fixture| {
        let live = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        let dead = fixture
            .store()
            .issue("grace@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        fixture
            .set_expiry(&dead, Utc::now() - chrono::TimeDelta::seconds(1))
            .await;

        let swept = fixture.store().sweep_expired().await.expect("sweep");
        assert_eq!(swept, 1);
        assert_eq!(fixture.rows().await, 1);
        assert_eq!(
            fixture
                .store()
                .consume(&live)
                .await
                .expect("consume")
                .as_deref(),
            Some("ada@example.test")
        );
    });
}

#[tokio::test]
async fn two_requests_carrying_one_link_spend_it_exactly_once() {
    // A double-clicked link, or a mail client that prefetches. Both requests
    // read the same row and both pass the digest check, so the delete is the
    // only thing standing between "one password change" and "two". Exactly
    // one caller may be told the subject.
    with_resets!(|fixture| {
        let link = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();

        let (left, right) = tokio::join!(
            fixture.store().consume(&link),
            fixture.store().consume(&link),
        );
        let left = left.expect("consume");
        let right = right.expect("consume");

        let redeemed = usize::from(left.is_some()) + usize::from(right.is_some());
        assert_eq!(
            redeemed, 1,
            "exactly one of two concurrent redemptions may spend the link"
        );
        for outcome in [left, right].into_iter().flatten() {
            assert_eq!(outcome, "ada@example.test");
        }
        assert_eq!(fixture.rows().await, 0);
    });
}

#[tokio::test]
async fn an_id_that_is_already_taken_is_reported_as_zero_rows_rather_than_an_error() {
    // What `issue`'s retry loop is built on. `ON CONFLICT DO NOTHING`,
    // `INSERT IGNORE`, and `INSERT OR IGNORE` are three different statements
    // that must all report a clash the same way -- as zero rows affected --
    // because the alternative is parsing a driver-specific constraint name
    // out of an error, which is exactly the code this store does not have.
    with_resets!(|fixture| {
        assert_eq!(fixture.insert_extra([7u8; 16], "ada@example.test").await, 1);
        assert_eq!(
            fixture.insert_extra([7u8; 16], "grace@example.test").await,
            0,
            "a taken id must be reported as zero rows, not raised as an error"
        );
        // And the clash left the first row exactly as it was.
        assert_eq!(fixture.rows().await, 1);
        assert_eq!(
            fixture
                .store()
                .revoke_all_for("ada@example.test")
                .await
                .expect("revoke all"),
            1
        );
    });
}

#[tokio::test]
async fn the_database_never_holds_the_link() {
    // A stolen backup, or a reporting account with SELECT on this table,
    // yields a digest. The assertion is against the raw bytes of the columns,
    // not against the store's own accessors -- and against the digest the
    // documentation names, so a change of algorithm cannot pass unnoticed.
    with_resets!(|fixture| {
        let link = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();

        let rows = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, String)>(
            "SELECT id, secret_digest, subject FROM arcature_password_resets",
        )
        .fetch_all(fixture.pool())
        .await
        .expect("read the row");
        assert_eq!(rows.len(), 1);
        let (id_column, digest_column, subject_column) = &rows[0];

        let (id, secret) = parse_plaintext(&link).expect("a link this crate minted");
        assert_eq!(id_column.as_slice(), id.as_bytes(), "the id is the key");
        assert_eq!(subject_column, "ada@example.test");
        assert_eq!(digest_column.len(), 32);
        assert_ne!(digest_column.as_slice(), &secret[..]);
        let expected: [u8; 32] = Sha256::digest(secret).into();
        assert_eq!(digest_column.as_slice(), &expected[..]);
    });
}

#[tokio::test]
async fn migrating_twice_is_a_no_op() {
    // The fixture already migrated once. The second run has to find its own
    // history row and do nothing -- which is a different question per
    // dialect, because MySQL declares its indexes inside `CREATE TABLE` for
    // want of `CREATE INDEX IF NOT EXISTS` and PostgreSQL takes an advisory
    // lock it must also release.
    with_resets!(|fixture| {
        fixture.store().migrate().await.expect("migrate again");
        let link = fixture
            .store()
            .issue("ada@example.test", HOUR)
            .await
            .expect("issue")
            .plaintext()
            .expose()
            .to_owned();
        assert_eq!(fixture.rows().await, 1);
        assert_eq!(
            fixture
                .store()
                .consume(&link)
                .await
                .expect("consume")
                .as_deref(),
            Some("ada@example.test")
        );
    });
}
