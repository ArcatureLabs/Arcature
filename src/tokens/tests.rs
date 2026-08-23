//! Round-trip tests against a live database.
//!
//! Issue, find, authenticate, list, revoke, sweep -- against a real server.
//! Seven statements per dialect are otherwise only ever compiled, and three
//! of the properties this store claims are properties of the SQL rather than
//! of the Rust around it: expiry is a predicate the database evaluates, the
//! listing order is an `ORDER BY` the database applies, and the abilities
//! column is JSONB on PostgreSQL, JSON on MySQL, and TEXT on SQLite -- one
//! `sqlx::types::Json` over three storage types that only a round trip can
//! tell apart.
//!
//! The fourth is structural rather than behavioural, and is tested here for
//! want of anywhere better: `FIND` and `AUTHENTICATE` differ only in that the
//! second also selects `secret_digest`, and the split is what makes every
//! other read *incapable* of loading a digest into memory. The assertion is
//! against the column names the server reports, not against the statement
//! text, because the statement text is what a mistake would be written in.
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
//! There is one `arcature_api_tokens` table, and the sweep and the row
//! counting are blind to whose rows they are. Two tests running concurrently
//! would delete each other's tokens and both would be right to fail.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use sqlx::types::Json;
use sqlx::{Column, Row};
use tokio::sync::{Mutex, MutexGuard};

use super::dialect::{TokenPool, sql, stored_time};
use super::store::{ApiTokens, digest_of};
use super::token::{ID_BYTES, parse_plaintext};
use super::{Abilities, ApiToken, ApiTokenId, NewApiToken};

/// Serialises the database tests against the one shared table.
static TABLE: Mutex<()> = Mutex::const_new(());

/// Pool size. More than one so nothing here passes because the pool
/// serialised it.
const CONNECTIONS: u32 = 4;

/// A month, an ordinary lifetime for one of these.
const MONTH: Duration = Duration::from_secs(60 * 60 * 24 * 30);

/// How far apart two instants may be and still be the same instant.
///
/// SQLite stores epoch milliseconds, so a timestamp that goes in and comes
/// back has lost everything below a millisecond. Two of them is slack enough
/// for that and tight enough that a dialect storing the wrong value -- a
/// local time read back as UTC, say -- fails by hours.
const SLACK: TimeDelta = TimeDelta::milliseconds(2);

/// A migrated, empty `arcature_api_tokens`, held exclusively until dropped.
struct Fixture {
    store: ApiTokens,
    _exclusive: MutexGuard<'static, ()>,
}

impl Fixture {
    fn store(&self) -> &ApiTokens {
        &self.store
    }

    fn pool(&self) -> &TokenPool {
        self.store.pool()
    }

    /// Every stored id, expiry ignored.
    ///
    /// Ignoring expiry is the point: it distinguishes "the query refused to
    /// return a lapsed token" from "the row is gone". Read as keys rather
    /// than counted, because `COUNT(*)` decodes to a different width on MySQL
    /// and a fixture failing to decode its own bookkeeping would look exactly
    /// like the store misbehaving.
    async fn ids(&self) -> Vec<Vec<u8>> {
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM arcature_api_tokens")
            .fetch_all(self.pool())
            .await
            .expect("read arcature_api_tokens")
    }

    async fn rows(&self) -> usize {
        self.ids().await.len()
    }

    /// Write a row with an id and a `created_at` of the caller's choosing.
    ///
    /// `issue` draws both, which is right for a credential and useless for
    /// testing an `ORDER BY` over them. This uses the store's own
    /// `INSERT_NEW` rather than a statement of its own, so what it proves
    /// about the insert is what the store does -- including how each dialect
    /// reports an id that is already taken.
    async fn insert_raw(
        &self,
        id: [u8; ID_BYTES],
        tokenable_id: &str,
        name: &str,
        expires_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    ) -> u64 {
        sqlx::query(sql::INSERT_NEW)
            .bind(id.to_vec())
            .bind(vec![0u8; 32])
            .bind(tokenable_id)
            .bind(name)
            .bind(Json(Vec::<String>::new()))
            .bind(stored_time(expires_at))
            .bind(stored_time(created_at))
            .execute(self.pool())
            .await
            .expect("insert a token by hand")
            .rows_affected()
    }

    /// The column names the server reports for one of the store's own
    /// statements, run for real against a row that exists.
    async fn columns_of(&self, statement: &'static str, id: ApiTokenId) -> Vec<String> {
        let row = sqlx::query(statement)
            .bind(id.as_bytes().to_vec())
            .fetch_one(self.pool())
            .await
            .expect("the statement has to match a row, or it reports no columns");
        row.columns()
            .iter()
            .map(|column| column.name().to_owned())
            .collect()
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

    let store = ApiTokens::new(pool);
    store
        .migrate()
        .await
        .unwrap_or_else(|error| panic!("migrate arcature_api_tokens: {error}"));
    sqlx::query("DELETE FROM arcature_api_tokens")
        .execute(store.pool())
        .await
        .unwrap_or_else(|error| panic!("empty arcature_api_tokens: {error}"));

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

/// Assert two instants are the same one, allowing for SQLite's millisecond
/// storage.
fn same_instant(left: DateTime<Utc>, right: DateTime<Utc>, what: &str) {
    assert!(
        (left - right).abs() <= SLACK,
        "{what}: {left} and {right} are more than {SLACK:?} apart"
    );
}

/// Splice the public half of one token onto the secret half of another.
///
/// Both halves are spellings the encoder itself produced, so the result
/// parses -- which is the point. A string that failed `parse_plaintext` would
/// never reach the database and would prove nothing about the statement.
fn with_foreign_secret(id_from: &str, secret_from: &str) -> String {
    // `rsplit_once`, not `split_once`: the prefix carries an underscore of
    // its own, and hex carries none, so the last one is the separator.
    let (id_half, _) = id_from.rsplit_once('_').expect("a token this crate minted");
    let (_, secret_half) = secret_from
        .rsplit_once('_')
        .expect("a token this crate minted");
    format!("{id_half}_{secret_half}")
}

/// The names in a listing, in the order the listing returned them.
fn names(tokens: &[ApiToken]) -> Vec<&str> {
    tokens.iter().map(ApiToken::name).collect()
}

#[tokio::test]
async fn a_freshly_issued_token_is_found_and_authenticates() {
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue(
                &NewApiToken::expiring_in("user:42", "laptop", MONTH)
                    .abilities(Abilities::of(["posts:read", "posts:write"])),
            )
            .await
            .expect("issue");
        let id = issued.token().id();
        let plaintext = issued.plaintext().expose().to_owned();

        let found = fixture
            .store()
            .find(id)
            .await
            .expect("find")
            .expect("a token just issued");
        assert_eq!(found.id(), id);
        assert_eq!(found.tokenable_id(), "user:42");
        assert_eq!(found.name(), "laptop");
        assert!(found.can("posts:write"));
        same_instant(
            found.expires_at(),
            issued.token().expires_at(),
            "expires_at",
        );
        same_instant(
            found.created_at(),
            issued.token().created_at(),
            "created_at",
        );

        let authenticated = fixture
            .store()
            .authenticate(&plaintext)
            .await
            .expect("authenticate")
            .expect("a token just issued");
        assert_eq!(authenticated.id(), id);
        assert_eq!(authenticated.tokenable_id(), "user:42");
        assert!(authenticated.can("posts:read"));
    });
}

#[tokio::test]
async fn the_digest_is_not_reachable_through_find() {
    // `FIND` and `AUTHENTICATE` differ by one column, and that difference is
    // the whole reason there are two statements: every read that is not an
    // authentication is structurally incapable of loading a digest. Asserted
    // against the column names the *server* reports for a real execution,
    // because a `SELECT *` or a copied statement would still pass a test that
    // only read the constant.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "laptop", MONTH))
            .await
            .expect("issue");
        let id = issued.token().id();

        let find = fixture.columns_of(sql::FIND, id).await;
        let authenticate = fixture.columns_of(sql::AUTHENTICATE, id).await;

        assert!(
            !find.iter().any(|column| column == "secret_digest"),
            "FIND reached the digest: {find:?}"
        );
        assert_eq!(
            find,
            [
                "tokenable_id",
                "name",
                "abilities",
                "expires_at",
                "created_at"
            ],
            "the store reads FIND's columns by index"
        );
        assert_eq!(
            authenticate[0], "secret_digest",
            "AUTHENTICATE reads the digest at index 0: {authenticate:?}"
        );
        assert_eq!(
            authenticate[1..],
            find[..],
            "the two statements must agree on everything after the digest"
        );
    });
}

#[tokio::test]
async fn abilities_survive_the_round_trip() {
    // The one column whose storage type varies by dialect: JSONB, JSON, and
    // TEXT, all decoded through `sqlx::types::Json<Vec<String>>`. An empty
    // set and a wildcard are here because both are values an application will
    // actually store, and both are the sort of thing a lax encoder turns into
    // `null`.
    with_tokens!(|fixture| {
        let listed =
            fixture
                .store()
                .issue(
                    &NewApiToken::expiring_in("user:42", "several", MONTH)
                        .abilities(Abilities::of(["posts:read", "billing:*", "sk\u{ed}ovat"])),
                )
                .await
                .expect("issue");
        let empty = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "none", MONTH))
            .await
            .expect("issue");
        let wildcard = fixture
            .store()
            .issue(
                &NewApiToken::expiring_in("user:42", "everything", MONTH)
                    .abilities(Abilities::all()),
            )
            .await
            .expect("issue");

        let found = fixture
            .store()
            .find(listed.token().id())
            .await
            .expect("find")
            .expect("just issued");
        assert_eq!(
            found.abilities().as_slice(),
            ["posts:read", "billing:*", "sk\u{ed}ovat"],
            "the set has to come back in the order it went in"
        );

        let found = fixture
            .store()
            .find(empty.token().id())
            .await
            .expect("find")
            .expect("just issued");
        assert!(found.abilities().as_slice().is_empty());
        assert!(!found.can("posts:read"));

        let found = fixture
            .store()
            .find(wildcard.token().id())
            .await
            .expect("find")
            .expect("just issued");
        assert!(found.abilities().is_all());
        assert!(found.can("anything:at:all"));
    });
}

#[tokio::test]
async fn listing_is_newest_first_and_ties_break_on_id() {
    // `ORDER BY created_at DESC, id`. The tiebreak is not decoration: two
    // tokens minted in the same millisecond are ordinary, and without it the
    // page a user sees would reshuffle between requests. `issue` draws its
    // own id and stamps its own clock, so the rows that make the order
    // observable are written by hand.
    with_tokens!(|fixture| {
        let now = Utc::now();
        let later = now + TimeDelta::seconds(3600);
        // Deliberately inserted out of order.
        fixture
            .insert_raw(
                [3u8; ID_BYTES],
                "user:42",
                "oldest",
                later,
                now - TimeDelta::seconds(2),
            )
            .await;
        fixture
            .insert_raw([1u8; ID_BYTES], "user:42", "newest-low-id", later, now)
            .await;
        fixture
            .insert_raw(
                [2u8; ID_BYTES],
                "user:42",
                "middle",
                later,
                now - TimeDelta::seconds(1),
            )
            .await;
        fixture
            .insert_raw([4u8; ID_BYTES], "user:42", "newest-high-id", later, now)
            .await;

        let listed = fixture.store().list_for("user:42").await.expect("list");
        assert_eq!(
            names(&listed),
            ["newest-low-id", "newest-high-id", "middle", "oldest"]
        );
    });
}

#[tokio::test]
async fn listing_is_scoped_to_one_subject_and_skips_expired_tokens() {
    with_tokens!(|fixture| {
        let now = Utc::now();
        let later = now + TimeDelta::seconds(3600);
        fixture
            .insert_raw([1u8; ID_BYTES], "user:42", "live", later, now)
            .await;
        fixture
            .insert_raw(
                [2u8; ID_BYTES],
                "user:42",
                "lapsed",
                now - TimeDelta::seconds(1),
                now - TimeDelta::seconds(2),
            )
            .await;
        fixture
            .insert_raw([3u8; ID_BYTES], "user:7", "somebody else's", later, now)
            .await;

        assert_eq!(
            names(&fixture.store().list_for("user:42").await.expect("list")),
            ["live"]
        );
        assert_eq!(
            names(&fixture.store().list_for("user:7").await.expect("list")),
            ["somebody else's"]
        );
        assert!(
            fixture
                .store()
                .list_for("user:nobody")
                .await
                .expect("list")
                .is_empty()
        );
        // The lapsed row is still on disk: the query declined to see it.
        assert_eq!(fixture.rows().await, 3);
    });
}

#[tokio::test]
async fn an_expired_token_does_not_authenticate_before_any_sweep_runs() {
    // Expiry is a predicate on every read, not a property of the sweep. A
    // deployment that never sweeps is wasteful, not insecure, and this is the
    // assertion that says so -- evaluated by the server's clock against the
    // column's own storage shape, which is a timestamp on two dialects and an
    // integer on the third.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue(&NewApiToken::new(
                "user:42",
                "yesterday's",
                Utc::now() - TimeDelta::seconds(1),
            ))
            .await
            .expect("issue");

        assert!(
            fixture
                .store()
                .authenticate(issued.plaintext().expose())
                .await
                .expect("authenticate")
                .is_none(),
            "an expired token must not authenticate"
        );
        assert!(
            fixture
                .store()
                .find(issued.token().id())
                .await
                .expect("find")
                .is_none()
        );
        assert_eq!(fixture.rows().await, 1, "nothing deleted it");
    });
}

#[tokio::test]
async fn a_wrong_secret_does_not_authenticate() {
    // The public id is not a credential, and this is what says so: a
    // well-formed token carrying somebody else's secret reaches the row and
    // is refused by the comparison.
    with_tokens!(|fixture| {
        let mine = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "mine", MONTH))
            .await
            .expect("issue");
        let theirs = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:7", "theirs", MONTH))
            .await
            .expect("issue");

        let forged = with_foreign_secret(mine.plaintext().expose(), theirs.plaintext().expose());
        assert!(
            fixture
                .store()
                .authenticate(&forged)
                .await
                .expect("authenticate")
                .is_none(),
            "a wrong secret must not authenticate"
        );
        // And the real one still does: a failed guess costs its owner
        // nothing.
        assert!(
            fixture
                .store()
                .authenticate(mine.plaintext().expose())
                .await
                .expect("authenticate")
                .is_some()
        );
    });
}

#[tokio::test]
async fn revoking_one_token_leaves_every_other() {
    with_tokens!(|fixture| {
        let laptop = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "laptop", MONTH))
            .await
            .expect("issue");
        fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "phone", MONTH))
            .await
            .expect("issue");

        assert!(
            fixture
                .store()
                .revoke(laptop.token().id())
                .await
                .expect("revoke")
        );
        assert!(
            !fixture
                .store()
                .revoke(laptop.token().id())
                .await
                .expect("revoke"),
            "a second revocation has nothing to revoke"
        );
        assert!(
            fixture
                .store()
                .authenticate(laptop.plaintext().expose())
                .await
                .expect("authenticate")
                .is_none()
        );
        assert_eq!(
            names(&fixture.store().list_for("user:42").await.expect("list")),
            ["phone"]
        );
        assert_eq!(fixture.rows().await, 1);
    });
}

#[tokio::test]
async fn revoking_a_subject_leaves_every_other_subject() {
    with_tokens!(|fixture| {
        for name in ["laptop", "phone", "ci"] {
            fixture
                .store()
                .issue(&NewApiToken::expiring_in("user:42", name, MONTH))
                .await
                .expect("issue");
        }
        let bystander = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:7", "theirs", MONTH))
            .await
            .expect("issue");

        let revoked = fixture
            .store()
            .revoke_all_for("user:42")
            .await
            .expect("revoke all");
        assert_eq!(revoked, 3);
        assert_eq!(fixture.rows().await, 1);
        assert!(
            fixture
                .store()
                .authenticate(bystander.plaintext().expose())
                .await
                .expect("authenticate")
                .is_some()
        );
    });
}

#[tokio::test]
async fn the_sweep_reclaims_expired_tokens_and_leaves_live_ones() {
    with_tokens!(|fixture| {
        let now = Utc::now();
        fixture
            .insert_raw(
                [1u8; ID_BYTES],
                "user:42",
                "live",
                now + TimeDelta::seconds(3600),
                now,
            )
            .await;
        fixture
            .insert_raw(
                [2u8; ID_BYTES],
                "user:42",
                "lapsed",
                now - TimeDelta::seconds(1),
                now,
            )
            .await;
        fixture
            .insert_raw(
                [3u8; ID_BYTES],
                "user:7",
                "somebody else's lapsed",
                now - TimeDelta::seconds(1),
                now,
            )
            .await;

        let swept = fixture.store().sweep_expired().await.expect("sweep");
        assert_eq!(swept, 2, "the sweep is not scoped to a subject");
        assert_eq!(fixture.ids().await, vec![vec![1u8; ID_BYTES]]);
    });
}

#[tokio::test]
async fn an_id_that_is_already_taken_is_reported_as_zero_rows_rather_than_an_error() {
    // What `issue`'s retry loop is built on. `ON CONFLICT DO NOTHING`,
    // `INSERT IGNORE`, and `INSERT OR IGNORE` are three different statements
    // that must all report a clash the same way -- as zero rows affected --
    // because the alternative is parsing a driver-specific constraint name
    // out of an error, which is exactly the code this store does not have.
    with_tokens!(|fixture| {
        let now = Utc::now();
        let later = now + TimeDelta::seconds(3600);
        assert_eq!(
            fixture
                .insert_raw([7u8; ID_BYTES], "user:42", "first", later, now)
                .await,
            1
        );
        assert_eq!(
            fixture
                .insert_raw([7u8; ID_BYTES], "user:7", "second", later, now)
                .await,
            0,
            "a taken id must be reported as zero rows, not raised as an error"
        );
        // And the clash left the first row exactly as it was.
        assert_eq!(
            names(&fixture.store().list_for("user:42").await.expect("list")),
            ["first"]
        );
        assert!(
            fixture
                .store()
                .list_for("user:7")
                .await
                .expect("list")
                .is_empty()
        );
    });
}

#[tokio::test]
async fn the_database_never_holds_a_working_token() {
    // A stolen backup, or a reporting account with SELECT on this table,
    // yields a digest. The assertion is against the raw bytes of the columns,
    // not against the store's own accessors -- and against the digest the
    // documentation names, so a change of algorithm cannot pass unnoticed.
    with_tokens!(|fixture| {
        let issued = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "laptop", MONTH))
            .await
            .expect("issue");
        let plaintext = issued.plaintext().expose().to_owned();

        let rows = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT id, secret_digest FROM arcature_api_tokens",
        )
        .fetch_all(fixture.pool())
        .await
        .expect("read the row");
        assert_eq!(rows.len(), 1);
        let (id_column, digest_column) = &rows[0];

        let (id, secret) = parse_plaintext(&plaintext).expect("a token this crate minted");
        assert_eq!(id_column.as_slice(), id.as_bytes(), "the id is the key");
        assert_eq!(digest_column.len(), 32);
        assert_ne!(digest_column.as_slice(), &secret[..]);
        assert_eq!(digest_column.as_slice(), &digest_of(&secret)[..]);
    });
}

#[tokio::test]
async fn migrating_twice_is_a_no_op() {
    // The fixture already migrated once. The second run has to find its own
    // history row and do nothing -- which is a different question per
    // dialect, because MySQL declares its indexes inside `CREATE TABLE` for
    // want of `CREATE INDEX IF NOT EXISTS` and PostgreSQL takes an advisory
    // lock it must also release.
    with_tokens!(|fixture| {
        fixture.store().migrate().await.expect("migrate again");
        let issued = fixture
            .store()
            .issue(&NewApiToken::expiring_in("user:42", "laptop", MONTH))
            .await
            .expect("issue");
        assert_eq!(fixture.rows().await, 1);
        assert!(
            fixture
                .store()
                .authenticate(issued.plaintext().expose())
                .await
                .expect("authenticate")
                .is_some()
        );
    });
}
