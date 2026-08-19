//! The typed query builder over SeaORM entities.

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder,
    QuerySelect,
};

use super::connection::Db;

/// A typed query bound to an explicit `&Db`, over a SeaORM entity `E`.
pub struct Query<'db, E>
where
    E: EntityTrait,
{
    db: &'db Db,
    select: sea_orm::Select<E>,
}

impl<'db, E> Query<'db, E>
where
    E: EntityTrait,
{
    /// Construct a query over all rows of `E`, bound to `db`.
    #[must_use]
    pub fn new(db: &'db Db) -> Self {
        Self {
            db,
            select: E::find(),
        }
    }

    /// Add a filter condition (typically `Column::X.eq(v)`).
    #[must_use]
    pub fn filter<C>(mut self, condition: C) -> Self
    where
        C: sea_orm::sea_query::IntoCondition,
    {
        self.select = self.select.filter(condition);
        self
    }

    /// An ascending `ORDER BY` on a column.
    #[must_use]
    pub fn order_by_asc<C>(mut self, column: C) -> Self
    where
        C: sea_orm::IntoSimpleExpr,
    {
        self.select = self.select.order_by_asc(column);
        self
    }

    /// A descending `ORDER BY` on a column.
    #[must_use]
    pub fn order_by_desc<C>(mut self, column: C) -> Self
    where
        C: sea_orm::IntoSimpleExpr,
    {
        self.select = self.select.order_by_desc(column);
        self
    }

    /// Limit the number of rows.
    #[must_use]
    pub fn limit(mut self, n: u64) -> Self {
        self.select = self.select.limit(n);
        self
    }

    /// Skip the first `n` rows.
    #[must_use]
    pub fn offset(mut self, n: u64) -> Self {
        self.select = self.select.offset(n);
        self
    }

    /// Execute and return all matching rows.
    pub async fn all(self) -> Result<Vec<E::Model>, crate::Error> {
        self.select
            .all(self.db.orm())
            .await
            .map_err(crate::Error::from)
    }

    /// Execute and return the first matching row, if any.
    pub async fn one(self) -> Result<Option<E::Model>, crate::Error> {
        self.select
            .one(self.db.orm())
            .await
            .map_err(crate::Error::from)
    }
}

/// The golden-path entry point: every SeaORM `Entity` gains `Entity::query(&db)`.
pub trait QueryModel: EntityTrait {
    /// Start a typed query over this entity, bound to `db`.
    #[must_use]
    fn query(db: &Db) -> Query<'_, Self> {
        Query::new(db)
    }
}

impl<E: EntityTrait> QueryModel for E {}

// --- CRUD free functions ----------------------------------------------------

/// Insert a new row from an `ActiveModel`.
pub async fn insert<A>(
    db: &Db,
    active: A,
) -> Result<<A::Entity as EntityTrait>::Model, crate::Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
    active.insert(db.orm()).await.map_err(crate::Error::from)
}

/// Update an existing row from an `ActiveModel`.
pub async fn update<A>(
    db: &Db,
    active: A,
) -> Result<<A::Entity as EntityTrait>::Model, crate::Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
    active.update(db.orm()).await.map_err(crate::Error::from)
}

/// Delete an existing row from its `ActiveModel`.
pub async fn delete<A>(db: &Db, active: A) -> Result<sea_orm::DeleteResult, crate::Error>
where
    A: ActiveModelTrait + ActiveModelBehavior + Send,
    <A::Entity as EntityTrait>::Model: IntoActiveModel<A>,
{
    active.delete(db.orm()).await.map_err(crate::Error::from)
}

/// Find a single row by its primary key.
pub async fn find_by_pk<E, P>(db: &Db, pk: P) -> Result<Option<E::Model>, crate::Error>
where
    E: EntityTrait,
    P: Into<<E::PrimaryKey as sea_orm::PrimaryKeyTrait>::ValueType> + Send,
{
    E::find_by_id(pk)
        .one(db.orm())
        .await
        .map_err(crate::Error::from)
}
