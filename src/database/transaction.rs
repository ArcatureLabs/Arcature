//! Typed transactions over the SeaORM and SQLx paths.

use std::future::Future;

use super::connection::Db;
use sea_orm::TransactionTrait;

/// Typed transactions over the explicit `&Db`. A namespace, not a value.
pub struct Transaction;

impl Transaction {
    /// Run a closure inside a SeaORM transaction over `db.orm()`. SeaORM
    /// commits on `Ok`, rolls back on `Err`.
    pub async fn orm<F, T>(db: &Db, f: F) -> Result<T, crate::Error>
    where
        F: for<'c> FnOnce(
                &'c sea_orm::DatabaseTransaction,
            ) -> std::pin::Pin<
                Box<dyn Future<Output = Result<T, sea_orm::DbErr>> + Send + 'c>,
            > + Send,
        T: Send,
    {
        db.orm().transaction(f).await.map_err(|e| match e {
            sea_orm::TransactionError::Connection(err)
            | sea_orm::TransactionError::Transaction(err) => crate::Error::from(err),
        })
    }

    /// Run a closure inside a raw SQLx transaction over `db.sqlx()`. Commits
    /// on `Ok`, rolls back on `Err`.
    pub async fn sqlx<F, T>(db: &Db, f: F) -> Result<T, crate::Error>
    where
        F: for<'c> FnOnce(
            &'c mut sqlx::Transaction<'_, super::Driver>,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<T, sqlx::Error>> + Send + 'c>,
        >,
        T: Send,
    {
        let mut txn = db.sqlx().begin().await.map_err(crate::Error::from)?;
        match f(&mut txn).await {
            Ok(value) => {
                txn.commit().await.map_err(crate::Error::from)?;
                Ok(value)
            }
            Err(error) => {
                drop(txn);
                Err(crate::Error::from(error))
            }
        }
    }
}
