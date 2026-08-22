//! The store itself: one table, four statements, and a sweep.

use std::collections::HashMap;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use sqlx::Row;
use sqlx::types::Json;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, ExpiredDeletion, SessionStore};

use crate::database::DatabaseConfig;

use super::dialect::{SessionDb, SessionPool, restored_time, sql, stored_time};
use super::error::SessionStoreError;
use super::migrate;

/// How many fresh ids [`SessionStore::create`] will try before giving up.
///
/// An id is 128 bits, so the first clash is already a once-in-the-heat-death
/// event and eight in a row is not chance -- it is a random source that is not
/// random. Looping forever would turn that into a hang; reporting it turns it
/// into a log line someone can act on.
const CREATE_ATTEMPTS: u32 = 8;

/// The ceiling on connections [`DbSessionStore::connect_lazy`] will open.
///
/// The store issues one short statement per request that touches a session, so
/// it needs a handful of connections and never a poolful. The cap matters
/// because `connect_lazy` opens a *second* pool alongside the application's
/// own, and a database's connection budget is shared: inheriting a
/// `max_connections` meant for the application's queries would quietly double
/// what the process asks the server for.
const CONNECTION_CEILING: u32 = 4;

/// Sessions stored in the application's own database.
///
/// # Why the row key is a digest
///
/// A session id is a bearer credential: it travels in a cookie, and whoever
/// holds it is the user until it expires. `arcature_sessions.id` therefore
/// holds the SHA-256 digest of the id, never the id. A lookup hashes whatever
/// the request presented and compares digests, which needs nothing else, and a
/// backup, a replica, or a read-only reporting account gets 32 bytes it cannot
/// walk back into a cookie.
///
/// # Why expiry is in the query
///
/// [`load`](SessionStore::load) selects `... AND expires_at > now()`, evaluated
/// by the database. An expired session is invisible from the instant it
/// expires, whether or not [`sweep_expired`](Self::sweep_expired) has run since
/// -- so a sweep that is late, misconfigured, or never wired up costs disk, not
/// security.
///
/// # Example
///
/// ```no_run
/// use arcature::auth::SessionConfig;
/// use arcature::auth::session_store::DbSessionStore;
/// use arcature::{Application, DatabaseConfig};
///
/// # async fn example(
/// #     database: DatabaseConfig,
/// #     session_config: SessionConfig,
/// # ) -> Result<(), Box<dyn std::error::Error>> {
/// let store = DbSessionStore::connect_lazy(&database);
/// store.migrate().await?;
///
/// let application: Application = Application::new()
///     .database(database)
///     .session(session_config, store)?
///     .build();
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DbSessionStore {
    pool: SessionPool,
}

impl DbSessionStore {
    /// Build a store over an existing pool.
    ///
    /// Prefer this when the application already holds a pool: one pool means
    /// one connection budget to reason about.
    #[must_use]
    pub fn new(pool: SessionPool) -> Self {
        Self { pool }
    }

    /// Build a store that will connect on first use.
    ///
    /// [`ApplicationBuilder::session`](crate::ApplicationBuilder::session)
    /// wants the store while the application is still being described, and the
    /// framework's own pool does not exist until `run` starts. A lazy pool
    /// resolves that ordering without making bootstrap async: nothing connects
    /// here, and the first statement opens the first connection.
    ///
    /// The cost is a second pool -- capped at [`CONNECTION_CEILING`]
    /// connections, starting at zero -- pointed at the same database as the
    /// application's. Call [`new`](Self::new) instead wherever the pool is
    /// already in hand.
    #[must_use]
    pub fn connect_lazy(config: &DatabaseConfig) -> Self {
        let pool_config = config.pool_config();
        let pool = sqlx::pool::PoolOptions::<SessionDb>::new()
            .max_connections(pool_config.get_max_connections().min(CONNECTION_CEILING))
            // An application that is not serving anybody holds no session
            // connections. The pool opens one when a request needs one.
            .min_connections(0)
            .acquire_timeout(pool_config.get_acquire_timeout())
            .idle_timeout(pool_config.get_idle_timeout())
            .max_lifetime(pool_config.get_max_lifetime())
            .connect_lazy_with(config.connect_options());
        Self { pool }
    }

    /// The pool the store runs over.
    #[must_use]
    pub fn pool(&self) -> &SessionPool {
        &self.pool
    }

    /// Create `arcature_sessions` and its index if they are not there.
    ///
    /// Idempotent, and safe to run from every replica at once: the migration
    /// is applied under the dialect's advisory lock with a history table.
    /// Call it at startup. A store whose table is missing fails on the first
    /// request instead, which is the same outage discovered by a user.
    ///
    /// # Errors
    ///
    /// Returns [`SessionStoreError::Database`] if the database is unreachable
    /// or rejects a statement.
    pub async fn migrate(&self) -> Result<(), SessionStoreError> {
        migrate::apply(&self.pool).await
    }

    /// Delete every session whose expiry has passed, and report how many.
    ///
    /// This reclaims disk. It is not what makes expiry correct -- every read
    /// already carries the expiry predicate -- so a deployment that never
    /// calls it is secure, merely wasteful. `tower_sessions` calls it through
    /// [`ExpiredDeletion`].
    ///
    /// # Errors
    ///
    /// Returns [`SessionStoreError::Database`] if the database is unreachable
    /// or rejects the statement.
    pub async fn sweep_expired(&self) -> Result<u64, SessionStoreError> {
        let result = sqlx::query(sql::DELETE_EXPIRED).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }

    /// Run one of the two write statements, reporting whether it wrote a row.
    ///
    /// `create` and `save` differ in exactly one thing -- what happens when the
    /// id is taken -- and that difference is entirely in the statement text, so
    /// they share the binding.
    async fn write(
        &self,
        statement: &'static str,
        record: &Record,
    ) -> Result<bool, SessionStoreError> {
        let expires_at = stored_time(record.expiry_date)?;
        let result = sqlx::query(statement)
            .bind(digest_of(&record.id))
            .bind(Json(&record.data))
            .bind(expires_at)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Read a live session back, or nothing.
    async fn read(&self, session_id: &Id) -> Result<Option<Record>, SessionStoreError> {
        let Some(row) = sqlx::query(sql::LOAD)
            .bind(digest_of(session_id))
            .fetch_optional(&self.pool)
            .await?
        else {
            return Ok(None);
        };

        // Columns by index, not by name: the three dialects agree on the
        // order the statement asks for and nothing else has to be true.
        let data: Json<HashMap<String, serde_json::Value>> = row
            .try_get(0)
            .map_err(|error| SessionStoreError::Decode(error.to_string()))?;
        let expires_at = row.try_get(1)?;

        Ok(Some(Record {
            id: *session_id,
            data: data.0,
            expiry_date: restored_time(expires_at)?,
        }))
    }
}

/// The 32 bytes a session id is stored under.
///
/// The id's little-endian bytes are what `tower_sessions` itself base64s into
/// the cookie, so hashing them keeps one canonical encoding rather than
/// inventing a second.
fn digest_of(id: &Id) -> Vec<u8> {
    Sha256::digest(id.0.to_le_bytes()).to_vec()
}

#[async_trait]
impl SessionStore for DbSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        for _ in 0..CREATE_ATTEMPTS {
            if self.write(sql::INSERT_NEW, record).await? {
                return Ok(());
            }
            // The insert did nothing, so the id is taken. Take another; the
            // caller's `&mut` exists for exactly this.
            record.id = Id::default();
        }
        Err(SessionStoreError::IdCollision {
            attempts: CREATE_ATTEMPTS,
        }
        .into())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.write(sql::UPSERT, record).await?;
        Ok(())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        Ok(self.read(session_id).await?)
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        sqlx::query(sql::DELETE)
            .bind(digest_of(session_id))
            .execute(&self.pool)
            .await
            .map_err(SessionStoreError::from)?;
        Ok(())
    }
}

#[async_trait]
impl ExpiredDeletion for DbSessionStore {
    async fn delete_expired(&self) -> session_store::Result<()> {
        self.sweep_expired().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_key_is_not_the_session_id() {
        // The property the whole design rests on: nothing derived from the id
        // that reaches the database is the id.
        let id = Id(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let digest = digest_of(&id);
        assert_eq!(digest.len(), 32);
        assert_ne!(digest.as_slice(), &id.0.to_le_bytes()[..]);
    }

    #[test]
    fn the_same_id_always_hashes_to_the_same_key() {
        // A lookup hashes the id the request presented, so an unstable digest
        // would log every user out on every request.
        let id = Id(-42);
        assert_eq!(digest_of(&id), digest_of(&id));
    }

    #[test]
    fn different_ids_hash_to_different_keys() {
        assert_ne!(digest_of(&Id(1)), digest_of(&Id(2)));
    }
}
