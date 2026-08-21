//! `Service` trait and `Inject<T>` extractor.
//!
//! A **service** is cheap composition from application resources. It is
//! constructed per request from application state `S` via [`Resolve<S>`],
//! NOT stored as a singleton, NOT resolved through a runtime container.
//!
//! The `#[service]` proc-macro generates:
//! - `impl DxComponent` -- the static name the module graph lists it under.
//! - `impl Service` -- the service marker + `DEPS` metadata.
//! - `impl Resolve<S>` -- per-state construction from field types.
//!
//! Handlers receive a service via the [`Inject<T>`] extractor, which is a
//! genuine Axum `FromRequestParts` implementation -- Axum remains the
//! handler runtime (no Arcature dispatcher).
//!
//! # Service dependency cycles
//!
//! Services compose by **value**, not by reference. A service `A` that
//! depends on service `B` stores `B` as a field. A cycle (`A` contains `B`
//! contains `A`) would require infinite size -- `rustc` rejects it at
//! compile time. No runtime cycle detection is needed; the type system
//! makes service cycles impossible.
//!
//! # Module service privacy
//!
//! A module's internal services are private by default. Privacy is
//! enforced by `rustc` visibility (a `pub(crate)` service cannot be named
//! outside its crate) and by `arc build` validation, which reports a module
//! that exports a name it does not declare as a service, a controller, or a
//! policy. An import naming a module that does not exist is rejected
//! earlier still, when the `ApplicationGraph` is constructed.

use axum::extract::FromRequestParts;

use super::resolve::Resolve;
use crate::DxComponent;

/// A service: cheap per-request composition from application resources.
///
/// The `#[service]` macro generates this impl. The trait extends
/// [`DxComponent`] (for the static `NAME`) and adds `DEPS` -- the
/// dependency type names, read off the struct's fields.
///
/// `DEPS` lists the simple type names of the service's fields. For
/// `LinkService { db: Db, cache: Cache }`, `DEPS = ["Db", "Cache"]`.
/// Resources (Db, Cache) are leaf nodes in the graph; service-to-service
/// edges are the only ones that could cycle, and cannot: a service is
/// composed by value from its fields, so a cycle would not compile.
pub trait Service: DxComponent + Send + Sync + 'static {
    /// The dependency type names, read off the struct's fields.
    /// Empty for services with no typed dependencies.
    const DEPS: &'static [&'static str] = &[];
}

/// Axum extractor: construct a `T: Resolve<S>` from application state.
///
/// A genuine `FromRequestParts` implementation -- Axum remains the
/// handler runtime. The extractor calls `T::resolve(state)` to construct
/// the value cheaply from the application's `Arc`/`Clone`-backed
/// resources.
///
/// # Example
///
/// ```ignore
/// async fn show(link: Bound<Link>, svc: Inject<LinkService>) -> Result<Json<Link>> {
///     let link = link.into_inner();
///     let report = svc.recent_for(link.id);
///     // ...
/// }
/// ```
///
/// `Inject<T>` works for any `T: Resolve<S>`, including built-in resources
/// (`Inject<Db>`) and services (`Inject<LinkService>`). The `Service`
/// trait is metadata, not a bound on the extractor.
pub struct Inject<T>(pub T);

impl<T> Inject<T> {
    /// Extract the resolved value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T, S> FromRequestParts<S> for Inject<T>
where
    T: Resolve<S>,
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        _parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Inject(T::resolve(state)))
    }
}
