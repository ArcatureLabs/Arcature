//! The typed query builder over SeaORM entities.
//!
//! The golden path: `Entity::query(&db).where_eq(Column::X, v).latest().paginate(20).page(1)`.
//! Every method is a real SeaORM delegation, not a stub.

use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, Iterable,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
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

    /// Add a raw filter condition (typically `Column::X.eq(v)`).
    #[must_use]
    pub fn filter<C>(mut self, condition: C) -> Self
    where
        C: sea_orm::sea_query::IntoCondition,
    {
        self.select = self.select.filter(condition);
        self
    }

    /// Convenience: `where_eq(Column::X, value)`.
    #[must_use]
    pub fn where_eq<C, V>(self, column: C, value: V) -> Self
    where
        C: ColumnTrait,
        V: Into<sea_orm::sea_query::Value>,
    {
        self.filter(column.eq(value))
    }

    /// Convenience: `where_ne(Column::X, value)`.
    #[must_use]
    pub fn where_ne<C, V>(self, column: C, value: V) -> Self
    where
        C: ColumnTrait,
        V: Into<sea_orm::sea_query::Value>,
    {
        self.filter(column.ne(value))
    }

    /// Convenience: `where_gt(Column::X, value)`.
    #[must_use]
    pub fn where_gt<C, V>(self, column: C, value: V) -> Self
    where
        C: ColumnTrait,
        V: Into<sea_orm::sea_query::Value>,
    {
        self.filter(column.gt(value))
    }

    /// Convenience: `where_lt(Column::X, value)`.
    #[must_use]
    pub fn where_lt<C, V>(self, column: C, value: V) -> Self
    where
        C: ColumnTrait,
        V: Into<sea_orm::sea_query::Value>,
    {
        self.filter(column.lt(value))
    }

    /// Convenience: `where_in(Column::X, values)`.
    #[must_use]
    pub fn where_in<C, V, I>(self, column: C, values: I) -> Self
    where
        C: ColumnTrait,
        V: Into<sea_orm::sea_query::Value>,
        I: IntoIterator<Item = V>,
    {
        self.filter(column.is_in(values.into_iter().map(Into::into).collect::<Vec<_>>()))
    }

    /// Convenience: `where_null(Column::X)`.
    #[must_use]
    pub fn where_null<C>(self, column: C) -> Self
    where
        C: ColumnTrait,
    {
        self.filter(column.is_null())
    }

    /// Convenience: `where_not_null(Column::X)`.
    #[must_use]
    pub fn where_not_null<C>(self, column: C) -> Self
    where
        C: ColumnTrait,
    {
        self.filter(column.is_not_null())
    }

    /// Order by a column descending (the most recent first). This is the
    /// "latest" convenience: typically ordered by the primary key (auto-
    /// increment id) or a `created_at` column.
    #[must_use]
    pub fn latest(mut self) -> Self
    where
        E::Column: Iterable,
    {
        if let Some(col) = E::Column::iter().next() {
            self.select = self.select.order_by_desc(col);
        }
        self
    }

    /// Order by a specific column descending.
    #[must_use]
    pub fn latest_by<C>(mut self, column: C) -> Self
    where
        C: sea_orm::IntoSimpleExpr,
    {
        self.select = self.select.order_by_desc(column);
        self
    }

    /// Order by a column ascending (the oldest first).
    #[must_use]
    pub fn oldest(mut self) -> Self
    where
        E::Column: Iterable,
    {
        if let Some(col) = E::Column::iter().next() {
            self.select = self.select.order_by_asc(col);
        }
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

    /// Paginate: split into pages of `per_page` rows. Returns a
    /// [`Paginated`] builder. Pages are 1-indexed.
    #[must_use]
    pub fn paginate(self, per_page: u64) -> Paginated<'db, E> {
        Paginated {
            query: self,
            per_page,
        }
    }

    /// Count the matching rows (the grand total, ignoring any `limit`/`offset`
    /// set on the query). Equivalent to `SELECT COUNT(*)` over the query.
    pub async fn count(self) -> Result<u64, crate::Error>
    where
        E: Send + Sync,
        E::Model: Send + Sync,
    {
        self.select
            .paginate(self.db.orm(), 1)
            .num_items()
            .await
            .map_err(crate::Error::from)
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

/// A paginated query: holds the original query and the page size.
pub struct Paginated<'db, E>
where
    E: EntityTrait,
{
    query: Query<'db, E>,
    per_page: u64,
}

impl<'db, E> Paginated<'db, E>
where
    E: EntityTrait,
{
    /// The page size (rows per page).
    #[must_use]
    pub fn per_page(&self) -> u64 {
        self.per_page
    }

    /// Fetch page `n` (1-indexed). Returns the rows for that page.
    pub async fn page(self, n: u64) -> Result<Vec<E::Model>, crate::Error> {
        if self.per_page == 0 {
            return Ok(Vec::new());
        }
        let offset = n.saturating_sub(1) * self.per_page;
        self.query
            .offset(offset)
            .limit(self.per_page)
            .all()
            .await
    }

    /// Fetch page `n` (1-indexed) with total count. Returns the rows and the
    /// total number of matching rows (before pagination).
    pub async fn page_with_count(
        self,
        n: u64,
    ) -> Result<(Vec<E::Model>, u64), crate::Error>
    where
        E: Send + Sync,
        E::Model: Send + Sync,
    {
        // Clone the select (cheap: it holds only a `SelectStatement`) and the
        // db reference, so the count runs without moving `self.query.select`.
        // `num_items` resets limit/offset, reporting the grand total.
        let select_for_count = self.query.select.clone();
        let db = self.query.db;
        let total = select_for_count
            .paginate(db.orm(), 1)
            .num_items()
            .await
            .map_err(crate::Error::from)?;
        let rows = self.page(n).await?;
        Ok((rows, total))
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
