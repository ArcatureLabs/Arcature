//! Round-trip tests against a live database.
//!
//! Issue, present, rotate, detect, revoke, sweep -- against a real server,
//! because every property this store claims is a property of the SQL. The
//! rotation is a compare-and-swap in a `WHERE` clause; the theft rule reads a
//! `CASE` the database evaluates; expiry is a predicate rather than a check
//! in Rust. An in-memory double would agree with whatever the Rust believes
//! and prove none of them.
//!
//! Two of these tests could not be written any other way. The one that races
//! two presentations of the same cookie is asking whether the database
//! serialises the `UPDATE`, which is a question only a database can answer.
//! The one that checks the grace window is asking whether `rotated_at > $1`
//! compares the two instants the store thinks it is comparing, across three
//! dialects that store time in three different shapes.
//!
//! # Why these tests need `test-kit`
//!
//! For the reason the session store's do: "skip, or fail because CI promised
//! a database" must never be wrong in the lenient direction, and
//! [`crate::test_kit::database`] already owns that decision together with the
//! refusal to write to any database not named `arcature_test_*`. Re-spelling
//! either rule here would leave two copies of something that must never
//! disagree.
//!
//! # Why one test at a time
//!
//! There is one `arcature_remember_tokens` table, and both the sweep and the
//! theft cascade delete rows they did not create -- being blind to whose rows
//! they are is the whole of what they do. Two tests running concurrently
//! would delete each other's tokens and both would be right to fail.

use std::time::Duration;

use chrono::Utc;
use tokio::sync::{Mutex, MutexGuard};

use super::dialect::{RememberPool, stored_time};
use super::store::{RememberOutcome, RememberTokens};
use super::token::parse_plaintext;

/// Serialises the database tests against the one shared table.
static TABLE: Mutex<()> = Mutex::const_new(());

/// Pool size. Above one so the concurrency test cannot pass because the pool
/// serialised it -- which would prove the pool works, not the SQL.
const CONNECTIONS: u32 = 4;

/// A month, the ordinary lifetime for one of these.
const MONTH: Duration = Duration::from_secs(60 * 60 * 24 * 30);

// The two statements below re-date a row so that expiry and the grace window
// can be tested in milliseconds rather than by sleeping through a month or a
// minute. They are spelled per dialect, and as literals rather than built with
// `crate::database::dialect::placeholder`, for the reason SQLx's `SqlSafeStr`
// bound exists: a `String` assembled at runtime has to be waved past that gate
// with `AssertSqlSafe`, and a test fixture is the last place that escape hatch
// should be normalised. Three lines of duplication is the cheaper half of that
// trade.

/// Binds: new deadline, series.
#[cfg(feature = "db-postgres")]
const SET_EXPIRY: &str = "UPDATE arcature_remember_tokens SET expires_at = $1 WHERE series = $2";
/// Binds: new deadline, series.
#[cfg(not(feature = "db-postgres"))]
const SET_EXPIRY: &str = "UPDATE arcature_remember_tokens SET expires_at = ? WHERE series = ?";

/// Binds: new rotation stamp, series.
#[cfg(feature = "db-postgres")]
const SET_ROTATED_AT: &str =
    "UPDATE arcature_remember_tokens SET rotated_at = $1 WHERE series = $2";
/// Binds: new rotation stamp, series.
#[cfg(not(feature = "db-postgres"))]
const SET_ROTATED_AT: &str = "UPDATE arcature_remember_tokens SET rotated_at = ? WHERE series = ?";

/// A migrated, empty `arcature_remember_tokens`, held exclusively until
/// dropped.
struct Fixture {
    store: RememberTokens,
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn store(&self) -> &RememberTokens {
        &self.store
    }

    fn pool(&self) -> &RememberPool {
        self.store.pool()
    }

    /// Every stored series, expiry ignored.
    ///
    /// Ignoring expiry is the point: it distinguishes "the query refused to
    /// return a lapsed token" from "the row is gone". Read as keys rather
    /// than counted, because `COUNT(*)` decodes to a different width on MySQL
    /// and a fixture failing to decode its own bookkeeping would look exactly
    /// like the store misbehaving.
    async fn series(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT series FROM arcature_remember_tokens")
            .fetch_all(self.pool())
            .await
            .expect("read arcature_remember_tokens")
    }

    async fn rows(&self) -> usize {
        self.series().await.len()
    }

    /// Move one token's deadline to `at`, so expiry can be tested without
    /// sleeping through a month.
    async fn set_expiry(&self, plaintext: &str, at: chrono::DateTime<Utc>) {
        let (series, _) = parse_plaintext(plaintext).expect("a token this crate minted");
        let affected = sqlx::query(SET_EXPIRY)
            .bind(stored_time(at))
            .bind(series.as_bytes().to_vec())
            .execute(self.pool())
            .await
            .expect("move the deadline")
            .rows_affected();
        assert_eq!(affected, 1, "the token to re-date was not there");
    }

    /// Move one token's `rotated_at` to `at`, so the grace window can be
    /// tested without waiting out a minute.
    async fn set_rotated_at(&self, plaintext: &str, at: chrono::DateTime<Utc>) {
        let (series, _) = parse_plaintext(plaintext).expect("a token this crate minted");
        let affected = sqlx::query(SET_ROTATED_AT)
            .bind(stored_time(at))
            .bind(series.as_bytes().to_vec())
            .execute(self.pool())
            .await
            .expect("move the rotation stamp")
            .rows_affected();
        assert_eq!(affected, 1, "the token to re-date was not there");
    }
}

/// A migrated, empty store, or `None` when this machine has no test database.
///
/// # Panics
///
/// Panics when a database is configured but unusable, and when none is
/// configured while `ARCATURE_REQUIRE_TEST_DB` says one was promised.
async fn tokens() -> Option<Fixture> {
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

    let store = RememberTokens::new(pool);
    store
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("migrate arcature_remember_tokens: {error}"));
    sqlx::query("DELETE FROM arcature_remember_tokens")
        .execute(store.pool())
        .await
        .unwrap_or_else(|error| panic!("empty arcature_remember_tokens: {error}"));

    Some(Fixture {
        store,
        _exclusive: exclusive,
    })
}

/// Run `body` with a fixture, or return quietly when there is no database.
macro_rules! with_tokens {
    (|$fixture:ident| $body:block) => {
        let Some($fixture) = tokens().await else {
            return;
        };
        $body
    };
}

/// The subject and replacement of an [`RememberOutcome::Accepted`], or a
/// panic naming what came back instead.
fn accepted(outcome: RememberOutcome) -> (String, Option<String>) {
    match outcome {
        RememberOutcome::Accepted {
            subject,
            replacement,
        } => (subject, replacement.map(|token| token.expose().to_owned())),
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[tokio::test]
async fn a_freshly_issued_token_signs_its_subject_in() {
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        assert_eq!(issued.subject(), "ada@example.test");

        let outcome = fixture
            .store()
            .present(issued.plaintext().expose())
            .await
            .expect("present");
        let (subject, replacement) = accepted(outcome);
        assert_eq!(subject, "ada@example.test");
        assert!(
            replacement.is_some(),
            "the first use of a token must spend it"
        );
    });
}

#[tokio::test]
async fn a_used_token_stops_working_and_its_replacement_starts() {
    // The property the whole scheme rests on. A copy taken from a log or a
    // backup is worthless once the real browser has made one request -- but
    // only if presenting the spent secret is *rejected*, not merely
    // "different".
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let first = issued.plaintext().expose().to_owned();

        let (_, replacement) = accepted(fixture.store().present(&first).await.expect("present"));
        let second = replacement.expect("the first use returns a replacement");
        assert_ne!(second, first, "rotation must change the cookie");

        // The replacement works.
        let (subject, _) = accepted(fixture.store().present(&second).await.expect("present"));
        assert_eq!(subject, "ada@example.test");

        // And the row survived both: rotation replaces a secret, it does not
        // mint a second token.
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn presenting_a_secret_from_two_rotations_ago_is_reported_as_theft() {
    // The other half of the scheme, and the half that is easy to get wrong by
    // treating any non-match as merely unrecognised. The series is live; the
    // secret is one the store retired and then retired again. Nothing
    // legitimate produces that.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let stolen = issued.plaintext().expose().to_owned();

        let (_, second) = accepted(fixture.store().present(&stolen).await.expect("present"));
        let second = second.expect("replacement");
        let (_, third) = accepted(fixture.store().present(&second).await.expect("present"));
        third.expect("replacement");

        match fixture.store().present(&stolen).await.expect("present") {
            RememberOutcome::Theft { subject } => assert_eq!(subject, "ada@example.test"),
            other => panic!("expected Theft, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn a_theft_report_has_already_signed_every_device_out() {
    // The ordering the outcome documentation promises: the tokens are gone
    // before the caller is told, so an application that is slow to react has
    // not left a window open. Three devices, one compromised, all three dead.
    with_tokens!(|fixture| {
        let laptop = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let phone = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let tablet = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        // Somebody else's, to prove the cascade is scoped to one subject.
        let bystander = fixture
            .store()
            .issue("grace@example.test", MONTH)
            .await
            .expect("issue");
        assert_eq!(fixture.rows().await, 4);

        let stolen = laptop.plaintext().expose().to_owned();
        let (_, replacement) = accepted(fixture.store().present(&stolen).await.expect("present"));
        replacement.expect("replacement");
        // Wait out the grace window by moving the rotation into the past,
        // rather than sleeping: what makes this a theft is that the retired
        // secret is no longer recent.
        fixture
            .set_rotated_at(&stolen, Utc::now() - chrono::TimeDelta::seconds(600))
            .await;

        match fixture.store().present(&stolen).await.expect("present") {
            RememberOutcome::Theft { .. } => {}
            other => panic!("expected Theft, got {other:?}"),
        }

        assert_eq!(
            fixture.rows().await,
            1,
            "only the bystander's token should survive"
        );
        assert!(matches!(
            fixture
                .store()
                .present(phone.plaintext().expose())
                .await
                .expect("present"),
            RememberOutcome::Unrecognised
        ));
        assert!(matches!(
            fixture
                .store()
                .present(tablet.plaintext().expose())
                .await
                .expect("present"),
            RememberOutcome::Unrecognised
        ));
        let (subject, _) = accepted(
            fixture
                .store()
                .present(bystander.plaintext().expose())
                .await
                .expect("present"),
        );
        assert_eq!(subject, "grace@example.test");
    });
}

#[tokio::test]
async fn a_client_one_rotation_behind_is_accepted_inside_the_grace_window() {
    // The false positive that would make strict rotation unusable: a browser
    // that kept the retired secret because it never processed the response
    // that carried the new one. Accepted, and deliberately *not* rotated
    // again -- a second rotation would put the client two behind.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let first = issued.plaintext().expose().to_owned();
        let (_, replacement) = accepted(fixture.store().present(&first).await.expect("present"));
        replacement.expect("replacement");

        let (subject, again) = accepted(fixture.store().present(&first).await.expect("present"));
        assert_eq!(subject, "ada@example.test");
        assert!(
            again.is_none(),
            "a grace-window acceptance must not rotate again"
        );
    });
}

#[tokio::test]
async fn the_same_retired_secret_is_theft_once_the_window_has_passed() {
    // The pair to the test above, and the reason the window is a window
    // rather than a permanent second chance. Same input, same store, only the
    // age of the rotation differs.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let first = issued.plaintext().expose().to_owned();
        let (_, replacement) = accepted(fixture.store().present(&first).await.expect("present"));
        replacement.expect("replacement");

        fixture
            .set_rotated_at(&first, Utc::now() - chrono::TimeDelta::seconds(3600))
            .await;

        match fixture.store().present(&first).await.expect("present") {
            RememberOutcome::Theft { subject } => assert_eq!(subject, "ada@example.test"),
            other => panic!("expected Theft, got {other:?}"),
        }
    });
}

#[tokio::test]
async fn two_requests_carrying_one_cookie_both_sign_in_and_only_one_rotates() {
    // A browser restoring a window of tabs. Both requests read the same row
    // and both match, so the compare-and-swap is the only thing standing
    // between "normal Tuesday" and "signed out everywhere". Exactly one
    // replacement must come back: two would leave the client holding whichever
    // response happened to arrive last, and the other secret would be a
    // retired one it never had.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let cookie = issued.plaintext().expose().to_owned();

        let (left, right) = tokio::join!(
            fixture.store().present(&cookie),
            fixture.store().present(&cookie),
        );
        let (left_subject, left_replacement) = accepted(left.expect("present"));
        let (right_subject, right_replacement) = accepted(right.expect("present"));

        assert_eq!(left_subject, "ada@example.test");
        assert_eq!(right_subject, "ada@example.test");
        let replacements =
            usize::from(left_replacement.is_some()) + usize::from(right_replacement.is_some());
        assert_eq!(
            replacements, 1,
            "exactly one of two concurrent presentations may spend the token"
        );
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn a_lapsed_token_is_unrecognised_before_any_sweep_runs() {
    // Expiry is a predicate on every read, not a property of the sweep. A
    // deployment that never sweeps is wasteful, not insecure, and this is the
    // assertion that says so.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let cookie = issued.plaintext().expose().to_owned();
        fixture
            .set_expiry(&cookie, Utc::now() - chrono::TimeDelta::seconds(1))
            .await;

        assert!(matches!(
            fixture.store().present(&cookie).await.expect("present"),
            RememberOutcome::Unrecognised
        ));
        // Still on disk: nothing deleted it, the query declined to see it.
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn the_sweep_reclaims_lapsed_rows_and_leaves_live_ones() {
    with_tokens!(|fixture| {
        let live = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let dead = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        fixture
            .set_expiry(
                dead.plaintext().expose(),
                Utc::now() - chrono::TimeDelta::seconds(1),
            )
            .await;

        let swept = fixture.store().sweep_expired().await.expect("sweep");
        assert_eq!(swept, 1);
        assert_eq!(fixture.rows().await, 1);
        let (subject, _) = accepted(
            fixture
                .store()
                .present(live.plaintext().expose())
                .await
                .expect("present"),
        );
        assert_eq!(subject, "ada@example.test");
    });
}

#[tokio::test]
async fn issuing_leaves_the_subjects_other_devices_signed_in() {
    // The documented departure from the password-reset store, pinned. Two
    // live reset links are a bug; two live remember-me tokens are a phone and
    // a laptop.
    with_tokens!(|fixture| {
        let laptop = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let phone = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        assert_eq!(fixture.rows().await, 2);

        for cookie in [laptop.plaintext().expose(), phone.plaintext().expose()] {
            let (subject, _) = accepted(fixture.store().present(cookie).await.expect("present"));
            assert_eq!(subject, "ada@example.test");
        }
    });
}

#[tokio::test]
async fn revoking_one_device_leaves_the_others_alone() {
    with_tokens!(|fixture| {
        let laptop = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let phone = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");

        assert!(
            fixture
                .store()
                .revoke(laptop.plaintext().expose())
                .await
                .expect("revoke")
        );
        assert!(matches!(
            fixture
                .store()
                .present(laptop.plaintext().expose())
                .await
                .expect("present"),
            RememberOutcome::Unrecognised
        ));
        let (subject, _) = accepted(
            fixture
                .store()
                .present(phone.plaintext().expose())
                .await
                .expect("present"),
        );
        assert_eq!(subject, "ada@example.test");
    });
}

#[tokio::test]
async fn revoking_everything_reports_what_it_deleted() {
    with_tokens!(|fixture| {
        for _ in 0..3 {
            fixture
                .store()
                .issue("ada@example.test", MONTH)
                .await
                .expect("issue");
        }
        fixture
            .store()
            .issue("grace@example.test", MONTH)
            .await
            .expect("issue");

        let revoked = fixture
            .store()
            .revoke_all_for("ada@example.test")
            .await
            .expect("revoke all");
        assert_eq!(revoked, 3);
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn a_series_nobody_issued_is_unrecognised_rather_than_theft() {
    // The distinction the outcome type exists to make. A cookie from a
    // database that was reset, or a token whose row was swept, is the system
    // working -- and reporting it as theft would sign people out for the
    // crime of having an old cookie.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let cookie = issued.plaintext().expose().to_owned();
        assert!(fixture.store().revoke(&cookie).await.expect("revoke"));

        assert!(matches!(
            fixture.store().present(&cookie).await.expect("present"),
            RememberOutcome::Unrecognised
        ));
    });
}

#[tokio::test]
async fn the_database_never_holds_a_working_cookie() {
    // A stolen backup, or a reporting account with SELECT on this table,
    // yields digests. The assertion is against the raw bytes of every column
    // that could plausibly carry one, not against the store's own accessors.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        let cookie = issued.plaintext().expose().to_owned();
        // Rotate once, so `previous_digest` is populated too.
        accepted(fixture.store().present(&cookie).await.expect("present"));

        let rows = sqlx::query_as::<_, (Vec<u8>, Option<Vec<u8>>)>(
            "SELECT secret_digest, previous_digest FROM arcature_remember_tokens",
        )
        .fetch_all(fixture.pool())
        .await
        .expect("read the digests");
        assert_eq!(rows.len(), 1);

        let (_, secret) = parse_plaintext(&cookie).expect("a token this crate minted");
        let (current, previous) = &rows[0];
        assert_ne!(current.as_slice(), &secret[..]);
        assert_eq!(current.len(), 32);
        let previous = previous.as_ref().expect("a rotated row has a previous");
        assert_ne!(previous.as_slice(), &secret[..]);
        assert_eq!(previous.len(), 32);
        // The retired secret is stored as its digest, not in the clear -- and
        // the digest of what we presented is what should be there.
        assert_eq!(previous.as_slice(), &super::store::digest_of(&secret)[..]);
    });
}

#[tokio::test]
async fn migrating_twice_is_a_no_op() {
    with_tokens!(|fixture| {
        fixture.store().migrate().await.expect("migrate again");
        fixture
            .store()
            .issue("ada@example.test", MONTH)
            .await
            .expect("issue");
        assert_eq!(fixture.rows().await, 1);
    });
}
