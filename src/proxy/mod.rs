//! Pre-routing proxy — application-owned global request policy (engine spec
//! §4/§5). One responsibility per file (AGENTS.md §1).
//!
//! The proxy runs *before* route selection. The engine wires the plumbing
//! (the Tower service that rewrites the request before the Axum router sees
//! it — see `service.rs`); the application owns only the policy, expressed as
//! a pure function from [`Request`] to [`Action`].

mod action;
mod request;
mod service;

pub use action::Action as ProxyAction;
pub use request::Request as ProxyRequest;
pub use service::ProxyLayer;
pub use service::ProxyService;

/// The application proxy function signature: a pure, synchronous decision
/// from a borrowed request view to an [`ProxyAction`]. `Arc`-shared so the
/// produced [`ProxyService`] is `Clone` (an `axum::serve` requirement).
pub type ProxyFn = std::sync::Arc<dyn Fn(ProxyRequest<'_>) -> ProxyAction + Send + Sync + 'static>;
