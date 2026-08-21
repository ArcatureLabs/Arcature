# Database

One PostgreSQL pool, two first-class paths. SeaORM and SQLx share the same
`PgPool` through `SqlxPostgresConnector::from_sqlx_postgres_pool`. There is no
second pool, no global registry, and no thread-local.

The `Db` handle is `Clone + Send + Sync + 'static`, so it lives in Axum state
like any other value.

## Connecting

```rust,ignore
use arcature::database::{DatabaseConfig, Db, PoolConfig};

let config = DatabaseConfig::new(&std::env::var("DATABASE_URL")?)?
    .pool(PoolConfig::new().max_connections(20))
    .application_name("acme");

let db = Db::connect(config).await?;
```

`Application::database(config)` does this for you at startup and hands the
handle to the state closure as `resources.db()`.

`SessionConfig` sets per-connection PostgreSQL timeouts —
`statement_timeout`, `lock_timeout`,
`idle_in_transaction_session_timeout` — applied when a connection is
established. `SessionConfig::none()` opts out.

`db.orm()` borrows the SeaORM `DatabaseConnection`; `db.sqlx()` borrows the
`PgPool`. `db.ping()` checks liveness; `db.close()` shuts the pool down.

## Reaching the handle from a handler

`Db` is not an Axum extractor. It comes out of state:

```rust,ignore
use arcature::axum::extract::State;

pub async fn index(State(state): State<AppState>) -> Result<Response> {
    let db = state.db.as_ref().ok_or_else(|| not_found("no database"))?;
    let users = user::Entity::query(db).all().await?;
    Ok(json(&users))
}
```

The generated application's `AppState` holds `db: Option<Db>`, because a
subsystem that was never configured contributes `None` rather than a panic.

## Models

A model is an ordinary SeaORM entity. SeaORM is re-exported as
`arcature::database::sea_orm`, so there is no second version to keep in step:

```rust,ignore
pub mod user {
    use arcature::database::sea_orm::entity::prelude::*;
    use arcature::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "users")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i64,
        pub email: String,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}
```

The struct must be named `Model` and must live in its own module: that is
SeaORM's requirement, not Arcature's. `DeriveEntityModel` generates `Entity`,
`Column` and `PrimaryKey` beside it.

### The short path: `#[model(table = "...")]`

`#[model]` writes the module above for you. It expands to a private module
holding a struct named `Model` -- which is the name SeaORM's
`DeriveEntityModel` requires -- and re-exports the family under predictable
names in the parent scope:

```rust,ignore
#[model(table = "users")]
pub struct User {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub email: String,
}
```

yields `User`, `UserEntity`, `UserActiveModel`, `UserColumn`, `UserPrimaryKey`
and `UserRelation`. `arc make:model` generates exactly this.

The generated `Relation` enum is empty and there is no syntax to fill it: the
enum lives inside the generated module, which an application cannot write
into. A model that needs relations is written as a plain SeaORM entity module
instead. `#[model]` is the short path, not the only one, and nothing else in
the database layer depends on it -- the query facade is a blanket impl over
every SeaORM entity, so a hand-written entity gets it for free.

A model is a database row. It is not a `#[resource]` and not browser-safe by
virtue of deriving `Serialize`; converting it for the browser is an explicit
`impl From<User> for UserResource`. See [Inertia](inertia.md).

## Querying

Every SeaORM entity gains `Entity::query(&db)` through the blanket
`QueryModel` impl. Bring the trait into scope with
`use arcature::database::QueryModel;`:

```rust,ignore
let recent = user::Entity::query(&db)
    .where_eq(user::Column::Active, true)
    .where_not_null(user::Column::VerifiedAt)
    .latest()
    .limit(20)
    .all()
    .await?;

let one = user::Entity::query(&db)
    .where_eq(user::Column::Email, "ada@example.com")
    .one()
    .await?;
```

The predicates are `where_eq`, `where_ne`, `where_gt`, `where_lt`,
`where_in`, `where_null`, `where_not_null`, and `filter` for a raw SeaORM
condition. Ordering is `latest`, `latest_by`, `oldest`, `order_by_asc`,
`order_by_desc`. `limit` and `offset` bound the window. The terminators are
`all`, `one` and `count`.

`paginate(per_page)` returns a `Paginated`; `page(n)` fetches a 1-indexed
page, and `page_with_count(n)` fetches the page and the grand total in two
statements.

## Writing

The CRUD free functions take the `ActiveModel`:

```rust,ignore
use arcature::database::{delete, find_by_pk, insert, update};

let created = insert(&db, user::ActiveModel { .. }).await?;
let changed = update(&db, active).await?;
delete(&db, active).await?;
let found = find_by_pk::<user::Entity>(&db, 1).await?;
```

## Transactions

`Transaction` is a namespace, not a value. Both paths commit on `Ok` and roll
back on `Err`:

```rust,ignore
use arcature::database::Transaction;

Transaction::orm(&db, |txn| Box::pin(async move {
    // SeaORM calls against `txn`
    Ok(())
})).await?;

Transaction::sqlx(&db, |txn| Box::pin(async move {
    sqlx::query("update accounts set balance = balance - $1")
        .bind(100)
        .execute(&mut **txn)
        .await?;
    Ok(())
})).await?;
```

The closure returns a pinned boxed future because the borrow of the
transaction has to outlive the call; that is the price of not owning the
transaction type.

## Route model binding

`Bound<T>` loads a model from a route parameter before the handler runs:

```rust,ignore
#[arcature::route_model(entity = link::Entity, key = "id", key_type = i64)]
pub struct Link(pub link::Model);

pub async fn show(link: Bound<Link>) -> Result<Response> {
    let link = link.into_inner();
    Ok(json(&link.0.id))
}
```

The extractor reads `KEY_PARAM` from the path, parses it (400 on failure),
calls `RouteModel::load` (500 on a database error), and returns a 404 problem
when the row is absent.

Binding does not imply authorization. `Bound<T>` proves the row exists; it
does not check that this user may see it. That check is a `Policy`, and it is
a separate, explicit step. The invariant is permanent and is restated in the
source of both `bound.rs` and `route_model.rs`.

`Bound<T>` obtains its `Db` through `DbFromState<S>`, an Arcature trait
rather than `axum::extract::FromRef` — `FromRef` and `Db` are both foreign
types, so the blanket impl would collide with the orphan rule. An application
writes one line:

```rust,ignore
impl DbFromState<AppState> for Db {
    fn db_from_state(state: &AppState) -> Db {
        state.db.clone().expect("database configured")
    }
}
```

For a lookup that is not a primary-key `find_by_id` — a slug scoped to a
tenant, say — write the `impl RouteModel` by hand. The macro covers the
common case and stops there.

## Migrations

Migrations are SeaORM migrations. The module wraps the four operations
against a `MigratorTrait` schema:

```rust,ignore
use arcature::database::migration;

migration::up::<Schema>(&db).await?;
migration::down::<Schema>(&db, 1).await?;
migration::fresh::<Schema>(&db).await?;
let status = migration::status::<Schema>(&db).await?;
```

From the CLI: `arc migrate`, `arc db:fresh`, `arc db:reset`, `arc db:seed`,
and `arc make:migration` to scaffold one.

## Drivers

The database feature splits three ways — `db-postgres`, `db-sqlite`,
`db-mysql` — so a SQLite application does not compile the PostgreSQL wire
protocol. The connection type above is PostgreSQL-specific, and the job queue
requires PostgreSQL because it depends on `FOR UPDATE SKIP LOCKED`. See
[Jobs](jobs.md).

## No ORM rewrite

Arcature does not own, reimplement, or rename SeaORM's query builder,
relation engine, or transaction system. `Query` is a thin facade over
`Select`; when it runs out, `db.orm()` and `db.sqlx()` are right there. The
cost is that two query vocabularies exist in one codebase, which is a
smaller cost than a half-built ORM.
