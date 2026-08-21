//! The Arcature database handle: one pool with two data paths.

use super::config::DatabaseConfig;
use super::{Driver, Pool};

/// The database handle. One [`Pool`] shared by SeaORM and SQLx.
///
/// `Clone + Send + Sync + 'static` so it works as Axum state.
#[derive(Clone)]
pub struct Db {
    pool: Pool,
    orm: sea_orm::DatabaseConnection,
}

impl Db {
    /// Connect to the database using resolved configuration.
    ///
    /// Validates the configuration before any async work, builds one pool,
    /// and derives the SeaORM `DatabaseConnection` over the same pool.
    pub async fn connect(config: DatabaseConfig) -> Result<Db, crate::Error> {
        config.validate()?;
        let pool = build_pool(&config).await?;
        let orm = orm_over(pool.clone());
        Ok(Db { pool, orm })
    }

    /// Construct a `Db` from an existing pool (the escape hatch).
    pub fn from_pool(pool: Pool) -> Db {
        let orm = orm_over(pool.clone());
        Db { pool, orm }
    }

    /// The SeaORM connection (derived over the same pool).
    pub fn orm(&self) -> &sea_orm::DatabaseConnection {
        &self.orm
    }

    /// The raw SQLx pool (the escape hatch for hand-written SQL).
    pub fn sqlx(&self) -> &Pool {
        &self.pool
    }

    /// Health check: `SELECT 1`.
    pub async fn ping(&self) -> Result<(), crate::Error> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(())
    }

    /// Close the pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

/// Derive the SeaORM connection over the SQLx pool. SeaORM names one
/// constructor per driver rather than a generic one, so this is the single
/// place the driver name appears.
fn orm_over(pool: Pool) -> sea_orm::DatabaseConnection {
    #[cfg(feature = "db-postgres")]
    {
        sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(pool)
    }
    #[cfg(feature = "db-sqlite")]
    {
        sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool)
    }
    #[cfg(feature = "db-mysql")]
    {
        sea_orm::SqlxMySqlConnector::from_sqlx_mysql_pool(pool)
    }
}

async fn build_pool(config: &DatabaseConfig) -> Result<Pool, crate::Error> {
    let pool_config = config.pool_config();

    #[allow(unused_mut)]
    let mut options = sqlx::pool::PoolOptions::<Driver>::new()
        .max_connections(pool_config.get_max_connections())
        .min_connections(pool_config.get_min_connections())
        .acquire_timeout(pool_config.get_acquire_timeout())
        .idle_timeout(pool_config.get_idle_timeout())
        .max_lifetime(pool_config.get_max_lifetime());

    // `SET statement_timeout` and friends are PostgreSQL session parameters.
    // MySQL spells the equivalents differently and SQLite has no session
    // parameters at all, so this hook exists only on the PostgreSQL build
    // rather than being approximated elsewhere.
    #[cfg(feature = "db-postgres")]
    if let Some(set_stmt) = config.session_config().set_statement() {
        options = options.after_connect(move |conn, _meta| {
            let stmt = set_stmt.clone();
            Box::pin(async {
                sqlx::Executor::execute(conn, sqlx::AssertSqlSafe(stmt))
                    .await
                    .map(|_| ())
            })
        });
    }

    options
        .connect_with(config.connect_options())
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))
}
