//! `Provider` trait -- startup-constructed application resources.
//!
//! A **provider** is a long-lived application resource constructed during
//! startup -- a Stripe client, a search client, a signer, an external API
//! SDK. Providers are NOT constructed per request (that is the service
//! lifetime). A provider initialization failure is a typed startup failure;
//! expensive network clients are never initialized from a request
//! extractor.
//!
//! The `#[provider]` macro generates `impl DxComponent` (for the static
//! name used by `arc services`). The developer writes `impl Provider` by
//! hand -- the `Error` type and `DEPS` are specific to the provider and
//! cannot be inferred from the struct definition alone.
//!
//! # Lifetime model
//!
//! | Lifetime   | Type      | When constructed   | Where it lives     |
//! |-----------|-----------|--------------------|--------------------|
//! | Resource  | Provider  | Application startup | Application state  |
//! | Service   | Service   | Per request        | Handler (via Inject)|
//! | Request   | T         | Per request        | Handler parameter   |
//!
//! Providers are placed into the application state `S` by the startup
//! closure. Services or handlers that need a provider obtain it via
//! `Resolve<S>` (the same mechanism as services) -- the application
//! provides a one-line `impl Resolve<S> for MyProvider` that clones the
//! provider from state.
//!
//! # Provider init
//!
//! The developer writes a regular `async fn` constructor (not a trait
//! method) -- the signature is provider-specific and may take `&Resources`,
//! `&Db`, configuration values, or any other startup input:
//!
//! ```ignore
//! impl StripeClient {
//!     pub async fn init(db: &Db, config: &StripeConfig) -> Result<Self, ProviderError> {
//!         // ...
//!     }
//! }
//! ```
//!
//! The `Provider` trait carries `Error` (the typed init failure) and
//! `DEPS` (for `arc check` graph validation). It does NOT carry the init
//! method -- init is business behavior, not mechanical plumbing, and the
//! macro must not hide business behavior.

/// A startup-constructed application resource.
///
/// Implemented by types that represent long-lived, expensive resources
/// constructed during application startup. The `#[provider]` macro
/// generates `impl DxComponent` for the name; the developer writes
/// `impl Provider` by hand with the `Error` type and `DEPS`.
///
/// Providers are NOT singletons in a container -- they are plain values
/// stored in the application state. The application decides how to
/// construct, store, and share them.
pub trait Provider: crate::DxComponent + Send + Sync + 'static {
    /// The typed initialization error. A provider init failure becomes a
    /// typed startup failure -- never a silent panic.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The dependency type names, for `arc check` graph validation.
    /// Empty for providers with no typed dependencies.
    const DEPS: &'static [&'static str] = &[];
}
