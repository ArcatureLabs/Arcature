//! Maintenance mode: one switch that takes the site offline without taking
//! the process down.
//!
//! # What it is for
//!
//! A migration that cannot run against live traffic, a data repair, a cutover.
//! The alternative -- stopping the process -- loses the ability to say
//! *anything* to a visitor, and loses the health endpoints an orchestrator
//! needs to know the instance is deliberately parked rather than crashed.
//!
//! # What gets through
//!
//! Two exemptions, both deliberate:
//!
//! * **The health endpoints.** If maintenance answered `503` on `/up/ready`
//!   too, the orchestrator would see a broken instance and start replacing
//!   instances that are doing exactly what they were told. The exempt prefix
//!   is supplied by [`Health::prefix`](crate::application::health::Health::prefix),
//!   so the two cannot drift apart.
//! * **Anything [`Maintenance::allow`] was given.** Typically the operator's
//!   own path back in.
//!
//! Everything else gets `503 Service Unavailable` with a `Retry-After` header
//! and an RFC 9457 [`Problem`](crate::api::Problem) body -- so a browser, a
//! `fetch`, and a CLI client all get an answer they can act on.
//!
//! # Where it sits in the pipeline
//!
//! Outside the session and CSRF layers, and outside Inertia. A maintenance
//! `503` must not depend on a session store that may be part of what is being
//! maintained, must not become a CSRF `419` because a form POST arrived during
//! the window, and must not be dressed up as an Inertia page response. See
//! [`crate::application::pipeline`].
//!
//! # The switch is a handle, not a global
//!
//! [`Maintenance`] is an `Arc`-backed handle you keep. Flip it from an admin
//! route, a signal handler, or a test. There is no registry to look it up in
//! -- if nothing holds the handle, nothing can engage it.

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use axum::http::{HeaderValue, Request, Response, header};
use tower::{Layer, Service};

/// The default `Retry-After` value, in seconds.
pub const DEFAULT_RETRY_AFTER: u32 = 60;

/// The maintenance switch.
///
/// Cheap to clone; every clone flips the same flag.
///
/// ```
/// use arcature::http::Maintenance;
///
/// let maintenance = Maintenance::new();
/// assert!(!maintenance.is_engaged());
/// maintenance.engage();
/// assert!(maintenance.is_engaged());
/// ```
#[derive(Clone, Debug)]
pub struct Maintenance {
    engaged: Arc<AtomicBool>,
    exempt: Arc<Vec<String>>,
    retry_after: u32,
}

impl Default for Maintenance {
    fn default() -> Self {
        Self::new()
    }
}

impl Maintenance {
    /// A disengaged switch.
    #[must_use]
    pub fn new() -> Self {
        Maintenance {
            engaged: Arc::new(AtomicBool::new(false)),
            exempt: Arc::new(Vec::new()),
            retry_after: DEFAULT_RETRY_AFTER,
        }
    }

    /// A switch that is already engaged.
    ///
    /// For an application that boots straight into maintenance -- a deploy
    /// that starts parked and is released once a migration finishes.
    #[must_use]
    pub fn engaged() -> Self {
        let maintenance = Self::new();
        maintenance.engage();
        maintenance
    }

    /// Let requests at `prefix` and anything under it through.
    ///
    /// Matches on a path-segment boundary, so exempting `/up` does **not**
    /// exempt `/upload`.
    #[must_use]
    pub fn allow(mut self, prefix: impl Into<String>) -> Self {
        let mut prefix = prefix.into();
        while prefix.len() > 1 && prefix.ends_with('/') {
            prefix.pop();
        }
        if !prefix.starts_with('/') {
            prefix.insert(0, '/');
        }
        Arc::make_mut(&mut self.exempt).push(prefix);
        self
    }

    /// Set the `Retry-After` value, in seconds (default
    /// [`DEFAULT_RETRY_AFTER`]).
    ///
    /// An honest estimate is worth sending: crawlers and well-behaved clients
    /// back off by it instead of retrying in a tight loop.
    #[must_use]
    pub fn retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = seconds;
        self
    }

    /// Engage the switch: everything but the exempt paths gets `503`.
    pub fn engage(&self) {
        self.engaged.store(true, Ordering::Release);
    }

    /// Disengage the switch: traffic flows again.
    pub fn disengage(&self) {
        self.engaged.store(false, Ordering::Release);
    }

    /// Whether the switch is engaged.
    #[must_use]
    pub fn is_engaged(&self) -> bool {
        self.engaged.load(Ordering::Acquire)
    }

    /// Whether `path` is exempt from the `503`.
    #[must_use]
    pub fn is_exempt(&self, path: &str) -> bool {
        self.exempt.iter().any(|prefix| covers(prefix, path))
    }

    /// Whether a request for `path` should be refused right now.
    #[must_use]
    pub fn blocks(&self, path: &str) -> bool {
        self.is_engaged() && !self.is_exempt(path)
    }
}

/// Whether `prefix` covers `path`, on a path-segment boundary.
fn covers(prefix: &str, path: &str) -> bool {
    if prefix == "/" {
        return true;
    }
    path == prefix
        || (path.len() > prefix.len()
            && path.starts_with(prefix)
            && path.as_bytes()[prefix.len()] == b'/')
}

impl<S> Layer<S> for Maintenance {
    type Service = MaintenanceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MaintenanceService {
            inner,
            maintenance: self.clone(),
        }
    }
}

/// The service [`Maintenance`] wraps around.
#[derive(Clone, Debug)]
pub struct MaintenanceService<S> {
    inner: S,
    maintenance: Maintenance,
}

impl<S> Service<Request<axum::body::Body>> for MaintenanceService<S>
where
    S: Service<
            Request<axum::body::Body>,
            Response = Response<axum::body::Body>,
            Error = Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<axum::body::Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<axum::body::Body>) -> Self::Future {
        if self.maintenance.blocks(request.uri().path()) {
            let response = unavailable(self.maintenance.retry_after);
            return Box::pin(async move { Ok(response) });
        }

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move { inner.call(request).await })
    }
}

/// The `503` body: an RFC 9457 problem, plus `Retry-After`.
fn unavailable(retry_after: u32) -> Response<axum::body::Body> {
    use axum::response::IntoResponse as _;

    let mut response = crate::api::Problem::of(crate::api::ProblemKind::Unavailable)
        .with_detail("The application is down for maintenance. Please try again shortly.")
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn a_new_switch_is_off_and_blocks_nothing() {
        let maintenance = Maintenance::new();
        assert!(!maintenance.is_engaged());
        assert!(!maintenance.blocks("/"));
    }

    #[test]
    fn engaging_blocks_everything_not_exempt() {
        let maintenance = Maintenance::new().allow("/up");
        maintenance.engage();
        assert!(maintenance.blocks("/"));
        assert!(maintenance.blocks("/users/1"));
        assert!(!maintenance.blocks("/up"));
        assert!(!maintenance.blocks("/up/ready"));
    }

    #[test]
    fn an_exempt_prefix_matches_on_a_segment_boundary() {
        // Exempting `/up` must not accidentally exempt an application route
        // that merely starts with the same letters.
        let maintenance = Maintenance::engaged().allow("/up");
        assert!(maintenance.blocks("/upload"));
        assert!(maintenance.blocks("/updates"));
    }

    #[test]
    fn a_prefix_is_normalised_before_it_is_matched() {
        let maintenance = Maintenance::engaged().allow("up/");
        assert!(!maintenance.blocks("/up/ready"));
    }

    #[test]
    fn every_clone_shares_one_switch() {
        let maintenance = Maintenance::new();
        let clone = maintenance.clone();
        maintenance.engage();
        assert!(clone.is_engaged());
        clone.disengage();
        assert!(!maintenance.is_engaged());
    }

    #[test]
    fn the_response_is_a_problem_with_retry_after() {
        let response = unavailable(30);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get(header::RETRY_AFTER).unwrap(),
            &HeaderValue::from_static("30")
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            &HeaderValue::from_static("application/problem+json")
        );
    }
}
