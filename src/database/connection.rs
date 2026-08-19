//! The Arcature database handle: one PostgreSQL pool with two data paths.

use sea_orm::SqlxPostgresConnector;
use sqlx::PgPool;

use super::config::DatabaseConfig;

/// The database handle. One `PgPool` shared by SeaORM and SQLx.
///
/// `Clone + Send + Sync + 'static` so it works as Axum state.
#[derive(Clone)]
pub struct Db {
    pool: PgPool,
    orm: sea_orm::DatabaseConnection,
}

impl Db {
    /// Connect to PostgreSQL using resolved configuration.
    ///
    /// Validates the configuration before any async work, builds one `PgPool`,
    /// and derives the SeaORM `DatabaseConnection` over the same pool.
    pub async fn connect(config: DatabaseConfig) -> Result<Db, crate::Error> {
        config.validate()?;
        let pool = build_pool(&config).await?;
        let orm = SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Ok(Db { pool, orm })
    }

    /// Construct a `Db` from an existing `PgPool` (the escape hatch).
    pub fn from_pool(pool: PgPool) -> Db {
        let orm = SqlxPostgresConnector::from_sqlx_postgres_pool(pool.clone());
        Db { pool, orm }
    }

    /// The SeaORM connection (derived over the same pool).
    pub fn orm(&self) -> &sea_orm::DatabaseConnection {
        &self.orm
    }

    /// The raw SQLx pool (the escape hatch for `sqlx::query!`).
    pub fn sqlx(&self) -> &PgPool {
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

async fn build_pool(config: &DatabaseConfig) -> Result<PgPool, crate::Error> {
    use sqlx::postgres::PgPoolOptions;

    let pool_config = config.pool_config();
    let session_config = config.session_config();

    let mut options = PgPoolOptions::new()
        .max_connections(pool_config.get_max_connections())
        .min_connections(pool_config.get_min_connections())
        .acquire_timeout(pool_config.get_acquire_timeout())
        .idle_timeout(pool_config.get_idle_timeout())
        .max_lifetime(pool_config.get_max_lifetime());

    if let Some(set_stmt) = session_config.set_statement() {
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
