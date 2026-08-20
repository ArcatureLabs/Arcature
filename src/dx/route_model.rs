//! The `RouteModel` contract -- a deliberate model-binding boundary.
//!
//! A type implements `RouteModel` to declare that it can be loaded from the
//! database by a route parameter (typically a primary key). The `Bound<T>`
//! extractor uses this trait to load the model from the request's route
//! parameters and the `Db` handle.
//!
//! ## Binding does NOT imply authorization
//!
//! Loading a model from the database is not an authorization decision. The
//! `Bound<T>` extractor returns 404 when the model is absent, but it does
//! NOT check whether the authenticated user is allowed to see the model.
//! Authorization is a separate, explicit step (Policies). This invariant is
//! permanent.
//!
//! ## No ORM rewrite
//!
//! The `RouteModel` trait is a thin contract over SeaORM's existing query
//! API. The `#[route_model]` macro generates an `impl RouteModel` that
//! calls `Entity::find_by_id(key).one(db.orm())`. Arcature does not own,
//! reimplement, or rename SeaORM's query builder, relation engine, or
//! transaction system.

use std::str::FromStr;

use crate::database::Db;

/// A type that can be loaded from the database by a route parameter.
///
/// The associated `Key` type is the typed route key (e.g. `i64`, `Uuid`). The
/// `KEY_PARAM` constant names the route parameter (e.g. `"id"`). The
/// `load` method performs the actual database query and returns `Ok(None)`
/// when the model is not found (the `Bound<T>` extractor maps this to a
/// 404 response).
///
/// This trait is the seam the `#[route_model]` macro generates code
/// against. For custom keys (e.g. slug-based lookup), the developer writes
/// a hand-written `impl RouteModel` instead of using the macro.
pub trait RouteModel: Sized + Send + Sync + 'static {
    /// The typed route key (e.g. `i64`, `Uuid`, `String` for slugs).
    type Key: FromStr + Send + Sync + Clone + 'static;

    /// The error type from the database query (e.g. `sea_orm::DbErr`).
    type Error: std::error::Error + Send + Sync + 'static;

    /// The route parameter name (e.g. `"id"`, `"slug"`).
    const KEY_PARAM: &'static str;

    /// Load the model by its key from the database.
    ///
    /// Return `Ok(None)` when the model is not found (404). Return `Err`
    /// for database errors (500). Do NOT authorize here -- authorization
    /// is a separate, explicit step.
    fn load(
        key: Self::Key,
        db: &Db,
    ) -> impl std::future::Future<Output = Result<Option<Self>, Self::Error>> + Send;
}
