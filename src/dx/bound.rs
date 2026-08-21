//! `Bound<T>` -- a genuine Axum extractor that loads a model from the
//! database by a route parameter.
//!
//! `Bound<T>` implements `FromRequestParts<S>` where `T: RouteModel` and
//! `Db: DbFromState<S>`. It:
//!
//! 1. Extracts the route key from `Path` params using `RouteModel::KEY_PARAM`.
//! 2. Parses it as `RouteModel::Key` via `FromStr` (400 Problem on parse failure).
//! 3. Calls `RouteModel::load(key, &db)` (500 Problem on DB error).
//! 4. If `None` -> 404 `Problem` response (`ProblemKind::NotFound`).
//! 5. If `Some(model)` -> `Ok(Bound(model))`.
//!
//! ## Binding does NOT imply authorization
//!
//! `Bound<T>` loads the model and verifies its existence. It does NOT check
//! whether the authenticated user may access the model. Authorization is a
//! separate, explicit step (Policies). This invariant is permanent.
//!
//! ## Example
//!
//! ```
//! use arcature::{Bound, Json, Result};
//!
//! # #[allow(dead_code)]
//! struct Link {
//!     id: i64,
//!     owner_id: i64,
//! }
//!
//! #[derive(serde::Serialize)]
//! struct LinkResource {
//!     id: i64,
//! }
//!
//! async fn show(link: Bound<Link>) -> Result<Json<LinkResource>> {
//!     let link = link.into_inner();
//!     // ... authorize access to `link` here -- binding did not ...
//!     Ok(Json(LinkResource { id: link.id }))
//! }
//! # fn main() {}
//! ```
//!
//! ## State extraction
//!
//! `Bound<T>` obtains the `Db` handle from the Axum state via the
//! [`DbFromState`](super::db_from_state::DbFromState) trait (not
//! `axum::extract::FromRef`, to avoid orphan-rule conflicts). Applications
//! implement `DbFromState` for their state type -- one line of code. The
//! simplest case (`impl DbFromState<Db> for Db`) is provided in the
//! [`db_from_state`](super::db_from_state) module.

use std::collections::HashMap;
use std::str::FromStr;

use axum::extract::FromRequestParts;
use axum::response::{IntoResponse, Response};

use crate::api::{ProblemBuilder, ProblemKind};
use crate::axum::extract::Path;
use crate::database::Db;
use crate::dx::db_from_state::DbFromState;
use crate::dx::route_model::RouteModel;

/// A model loaded from the database by a route parameter.
///
/// Wraps the loaded model. Use `into_inner()` to extract the model.
pub struct Bound<T>(pub T);

impl<T> Bound<T> {
    /// Extract the loaded model.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> FromRequestParts<S> for Bound<T>
where
    T: RouteModel,
    S: Send + Sync,
    Db: DbFromState<S>,
{
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let db = Db::db_from_state(state);

        // Extract the route key from Path params.
        let path_params = Path::<HashMap<String, String>>::from_request_parts(parts, state)
            .await
            .map_err(|_| {
                ProblemBuilder::new(ProblemKind::BadRequest)
                    .detail("missing route parameters")
                    .build()
                    .into_response()
            })?;

        let key_str = path_params.get(T::KEY_PARAM).ok_or_else(|| {
            ProblemBuilder::new(ProblemKind::BadRequest)
                .detail(format!("missing route parameter `{}`", T::KEY_PARAM))
                .build()
                .into_response()
        })?;

        // Parse the typed key.
        let key = T::Key::from_str(key_str).map_err(|_| {
            ProblemBuilder::new(ProblemKind::BadRequest)
                .detail(format!(
                    "invalid route parameter `{}`: expected {}",
                    T::KEY_PARAM,
                    std::any::type_name::<T::Key>()
                ))
                .build()
                .into_response()
        })?;

        // Load the model from the database.
        let model = T::load(key, &db).await.map_err(|err| {
            ProblemBuilder::new(ProblemKind::Internal)
                .detail(format!("database error: {err}"))
                .build()
                .into_response()
        })?;

        // 404 if not found.
        let model = model.ok_or_else(|| {
            ProblemBuilder::new(ProblemKind::NotFound)
                .build()
                .into_response()
        })?;

        Ok(Bound(model))
    }
}
