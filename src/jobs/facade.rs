//! The `Jobs` facade: enqueue over the application's existing pool.
//!
//! One pool, no second connection. `enqueue_tx` / `enqueue_with` take a caller
//! transaction/executor so `create order + enqueue job` share one transaction.

use super::dialect::{JobDb, JobPool};
use super::enqueue::{EnqueuedJob, JobRequest, insert_job};
use super::error::{EnqueueError, MigrateError};
use super::migrate;

/// The job queue facade over the application's existing pool.
#[derive(Clone)]
pub struct Jobs {
    pool: JobPool,
}

impl Jobs {
    /// Create a `Jobs` facade from an existing pool. No second pool.
    pub fn new(pool: JobPool) -> Self {
        Self { pool }
    }

    /// The underlying pool (the escape hatch).
    pub fn pool(&self) -> &JobPool {
        &self.pool
    }

    /// Apply the jobs schema migrations (idempotent, advisory-locked).
    pub async fn migrate(&self) -> Result<(), MigrateError> {
        migrate::apply(&self.pool).await
    }

    /// Apply migrations within a caller's transaction.
    pub async fn migrate_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, JobDb>,
    ) -> Result<(), MigrateError> {
        migrate::apply_tx(tx).await
    }

    /// Enqueue a job.
    pub async fn enqueue<J>(&self, request: &JobRequest<J>) -> Result<EnqueuedJob, EnqueueError>
    where
        J: serde::Serialize + serde::de::DeserializeOwned,
    {
        insert_job(&self.pool, request).await
    }

    /// Enqueue a job using a caller-supplied executor (e.g. a transaction).
    pub async fn enqueue_with<'c, E, J>(
        &self,
        executor: E,
        request: &JobRequest<J>,
    ) -> Result<EnqueuedJob, EnqueueError>
    where
        E: sqlx::Executor<'c, Database = JobDb>,
        J: serde::Serialize + serde::de::DeserializeOwned,
    {
        insert_job(executor, request).await
    }

    /// Enqueue a job within a caller's transaction.
    pub async fn enqueue_tx<J>(
        &self,
        tx: &mut sqlx::Transaction<'_, JobDb>,
        request: &JobRequest<J>,
    ) -> Result<EnqueuedJob, EnqueueError>
    where
        J: serde::Serialize + serde::de::DeserializeOwned,
    {
        insert_job(&mut **tx, request).await
    }
}
