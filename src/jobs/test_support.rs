//! The live-database fixture the queue's own tests share.
//!
//! Every test below `crate::jobs` that needs a server goes through
//! [`queue`]. It is the single place that decides whether this run has a
//! database at all, and the single place that resets the table, so each test
//! reads as its own scenario rather than as setup.
//!
//! # Why these tests need `test-kit`
//!
//! "Skip, or fail because CI promised a database" is a decision that must
//! never be wrong in the lenient direction: get it wrong and a green run
//! means nothing was tested. `crate::test_kit::database` already owns that
//! decision, together with the check that refuses to write to any database
//! whose name is not prefixed `arcature_test_`. Re-spelling either rule here
//! to save a feature flag would leave two copies of something that must never
//! disagree, so these tests are gated on `test-kit` and call the original.
//!
//! The consequence is that `cargo test` on the default features does not
//! compile them. `just db-test` and CI's `Database` matrix both enable
//! `test-kit`, and `just drivers` type-checks the whole feature list against
//! each of the three drivers.
//!
//! # Why one test at a time
//!
//! There is one `arcature_jobs` table, and the claim is deliberately blind to
//! `kind`: a claimer takes the oldest available rows, whoever wrote them.
//! Two tests running concurrently would therefore claim each other's jobs and
//! both would be right to fail. Rather than partition the table -- which
//! would mean testing a claim the queue does not actually issue -- the
//! fixture hands out an exclusive lock for the duration of a test.

use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};
use uuid::Uuid;

use super::dialect::JobPool;
use super::{JobModel, JobRequest, Jobs};

/// Serialises the database tests against the one shared table. See the module
/// comment.
static TABLE: Mutex<()> = Mutex::const_new(());

/// The payload the fixture enqueues. The queue never looks inside it; the
/// field exists so the rows are distinguishable when a failure is being read
/// by hand.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Marker {
    pub(crate) n: i64,
}

/// The model the fixture enqueues under. `max_attempts` is 3 so a test can
/// exercise a retry without immediately exhausting the job.
pub(crate) const MARKER: JobModel<Marker> = JobModel::new("arcature.test.marker", 1, 3);

/// A migrated, empty `arcature_jobs`, held exclusively until dropped.
pub(crate) struct Queue {
    pool: JobPool,
    /// Dropped with the fixture, which is what releases the next test.
    _exclusive: MutexGuard<'static, ()>,
}

impl Queue {
    /// The pool. Its connection limit is [`CONNECTIONS`], which is above
    /// [`WORKERS`] on purpose: a concurrency test that silently queued on the
    /// pool would prove the pool serialises, not the database.
    pub(crate) fn pool(&self) -> &JobPool {
        &self.pool
    }
}

/// How many concurrent claimers the contention tests run.
pub(crate) const WORKERS: usize = 8;

/// How many jobs those tests enqueue. A multiple of [`WORKERS`] so an even
/// split is possible, and large enough that the claimers genuinely overlap.
pub(crate) const JOBS: usize = 40;

/// Pool size: one connection per worker, plus room for the test's own
/// bookkeeping queries.
const CONNECTIONS: u32 = WORKERS as u32 + 2;

/// A migrated, empty queue, or `None` when this machine has no test database.
///
/// # Panics
///
/// Panics when a database is configured but unusable -- an unsafe name, a
/// refused connection, a migration the server rejects -- and when none is
/// configured while `ARCATURE_REQUIRE_TEST_DB` says one was promised.
pub(crate) async fn queue() -> Option<Queue> {
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
        .acquire_timeout(Duration::from_secs(30))
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect to the test database: {error}"));

    super::migrate::apply(&pool)
        .await
        .expect("apply the jobs migrations");

    // Not `TRUNCATE`: PostgreSQL and MySQL have it, SQLite does not, and the
    // table is never big enough for the difference to matter.
    sqlx::query("DELETE FROM arcature_jobs")
        .execute(&pool)
        .await
        .expect("empty arcature_jobs");

    Some(Queue {
        pool,
        _exclusive: exclusive,
    })
}

/// Enqueue `count` immediately-claimable jobs and return their ids.
pub(crate) async fn enqueue(pool: &JobPool, count: usize) -> Vec<Uuid> {
    let jobs = Jobs::new(pool.clone());
    let mut ids = Vec::with_capacity(count);
    for n in 0..count {
        let request = JobRequest::new(&MARKER, &Marker { n: n as i64 }).expect("build the request");
        ids.push(jobs.enqueue(&request).await.expect("enqueue").id);
    }
    ids
}

/// Every row's `(id, status, attempts)`, ordered by nothing in particular.
///
/// Read as three columns rather than counted with `COUNT(*)` on purpose: the
/// width of a `COUNT` differs by dialect (MySQL hands back an unsigned
/// bigint), and a fixture that fails to decode its own bookkeeping query
/// would look exactly like the queue misbehaving.
pub(crate) async fn rows(pool: &JobPool) -> Vec<(Uuid, String, i32)> {
    use sqlx::Row;

    sqlx::query("SELECT id, status, attempts FROM arcature_jobs")
        .fetch_all(pool)
        .await
        .expect("read arcature_jobs")
        .iter()
        .map(|row| {
            (
                row.try_get("id").expect("id"),
                row.try_get("status").expect("status"),
                row.try_get("attempts").expect("attempts"),
            )
        })
        .collect()
}

/// One row's `(status, attempts, claim_token)`.
pub(crate) async fn row(pool: &JobPool, id: Uuid) -> (String, i32, Option<Uuid>) {
    use sqlx::Row;

    let sql = format!(
        "SELECT status, attempts, claim_token FROM arcature_jobs WHERE id = {}",
        crate::database::dialect::placeholder(1)
    );
    // `AssertSqlSafe` because the statement is built rather than written out.
    // What it asserts holds: the only interpolation is the dialect's own
    // placeholder token, and the id is bound.
    let row = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read one job row");
    (
        row.try_get("status").expect("status"),
        row.try_get("attempts").expect("attempts"),
        row.try_get("claim_token").expect("claim_token"),
    )
}

/// Drain the queue with `WORKERS` concurrent claimers and return what each
/// one claimed, in claim order.
///
/// This is the shape all three dialects are tested with, because it is the
/// shape the property is stated in: whatever the strategy underneath, a job
/// belongs to one claimer. A worker stops at its first empty batch, which is
/// sound here because the fixture enqueues everything before any claimer
/// starts -- an empty batch means drained, not "not yet written".
pub(crate) async fn drain_concurrently(pool: &JobPool, batch: i64) -> Vec<Vec<Uuid>> {
    let mut claimers = Vec::with_capacity(WORKERS);
    for worker in 0..WORKERS {
        let pool = pool.clone();
        claimers.push(tokio::spawn(async move {
            let id = format!("worker-{worker}");
            let mut mine = Vec::new();
            loop {
                let claimed = super::claim::claim_jobs(&pool, &id, Duration::from_secs(60), batch)
                    .await
                    // Not `unwrap_or_default`: a claim that errors under
                    // contention -- a deadlock, a SQLITE_BUSY the busy
                    // timeout should have absorbed -- is the failure this
                    // test exists to catch, and swallowing it would read as
                    // "that worker claimed nothing".
                    .expect("claim a batch");
                if claimed.is_empty() {
                    break mine;
                }
                mine.extend(claimed.iter().map(|job| job.id));
            }
        }));
    }

    let mut claimed = Vec::with_capacity(WORKERS);
    for claimer in claimers {
        claimed.push(claimer.await.expect("a claimer panicked"));
    }
    claimed
}

/// Assert that `claimed` -- what each worker got -- accounts for `enqueued`
/// exactly once, and that the rows agree.
///
/// The two halves are both needed. The Rust-side check catches two workers
/// being *told* they own the same job, which is the bug that corrupts an
/// application; the row check catches a claim that incremented `attempts`
/// twice, which is the same race showing up in the data instead.
pub(crate) async fn assert_claimed_exactly_once(
    pool: &JobPool,
    enqueued: &[Uuid],
    claimed: &[Vec<Uuid>],
) {
    let mut seen: std::collections::HashMap<Uuid, Vec<usize>> = std::collections::HashMap::new();
    for (worker, batch) in claimed.iter().enumerate() {
        for &id in batch {
            seen.entry(id).or_default().push(worker);
        }
    }

    let contested: Vec<_> = seen.iter().filter(|(_, by)| by.len() > 1).collect();
    assert!(
        contested.is_empty(),
        "a job was handed to more than one worker: {contested:?}"
    );

    for id in enqueued {
        assert!(seen.contains_key(id), "job {id} was never claimed");
    }
    assert_eq!(
        seen.len(),
        enqueued.len(),
        "claimed {} distinct jobs, enqueued {}",
        seen.len(),
        enqueued.len()
    );

    for (id, status, attempts) in rows(pool).await {
        assert_eq!(status, "running", "job {id} is {status}, not running");
        assert_eq!(attempts, 1, "job {id} was claimed {attempts} times");
    }
}
