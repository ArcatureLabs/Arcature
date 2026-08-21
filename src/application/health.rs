//! Health, liveness and readiness endpoints.
//!
//! # Why the framework owns these
//!
//! An orchestrator decides whether to send traffic to this process, and
//! whether to restart it, by asking it two different questions. Getting them
//! confused is the classic outage: a readiness probe wired to liveness
//! restarts a pod because its database is briefly unreachable, and the restart
//! does not bring the database back.
//!
//! * `GET /up/live` -- **is this process alive?** Answered from the
//!   [`Lifecycle`] alone. It never touches a database, a cache, or the
//!   network, so a failing dependency can never cause a restart loop.
//! * `GET /up/ready` -- **should this process receive traffic?** The lifecycle
//!   must be `Ready` *and* every started subsystem must answer its probe.
//!   During graceful shutdown the lifecycle moves to `Draining` before the
//!   listener stops, so this turns `503` while requests already in flight are
//!   still being served -- which is exactly the window a load balancer needs
//!   to take the instance out of rotation without dropping anything.
//! * `GET /up` -- the same information as JSON, for a human or a dashboard.
//!
//! # Two things these routes deliberately bypass
//!
//! **Maintenance mode.** A maintenance `503` is for browsers; if it also hit
//! the health routes the orchestrator would see the instance as broken and
//! start replacing instances that are working exactly as intended. See
//! [`crate::http::maintenance`].
//!
//! **Inertia.** Health responses are `application/json` no matter what the
//! caller sends, because the caller is a probe, not a browser. They also carry
//! `Cache-Control: no-store`: a cached readiness answer is a wrong answer.
//!
//! # Resources arrive after the router is built
//!
//! The router is composed in [`ApplicationBuilder::build`], but subsystems
//! only connect later, inside `run_with_state`. [`Health`] therefore holds a
//! `OnceLock<Resources>` that startup fills in. Before it is filled the
//! readiness report lists no subsystem checks and the lifecycle is still
//! `Starting`, so readiness is `false` -- which is the honest answer.
//!
//! [`ApplicationBuilder::build`]: super::ApplicationBuilder::build

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::application::lifecycle::{Lifecycle, LifecycleState};
use crate::application::resources::Resources;
use crate::routing::RouterState;

/// The default path prefix the three endpoints are mounted under.
pub const DEFAULT_PREFIX: &str = "/up";

/// The shared handle behind the health endpoints.
///
/// Cheap to clone (`Arc`-backed). The builder creates one, mounts its
/// [`router`](Self::router) on the application router, and hands the same
/// handle to startup so it can [`publish`](Self::publish) the connected
/// subsystems.
#[derive(Clone)]
pub struct Health(Arc<Inner>);

struct Inner {
    prefix: String,
    lifecycle: Lifecycle,
    resources: OnceLock<Resources>,
}

impl Health {
    /// A handle reporting on `lifecycle`, mounted under `prefix`.
    ///
    /// A trailing slash on `prefix` is trimmed, and a prefix that does not
    /// start with `/` gets one, so `"up"`, `"/up"` and `"/up/"` all mount the
    /// same three paths.
    #[must_use]
    pub fn new(prefix: impl Into<String>, lifecycle: Lifecycle) -> Self {
        let mut prefix = prefix.into();
        while prefix.ends_with('/') {
            prefix.pop();
        }
        if !prefix.starts_with('/') {
            prefix.insert(0, '/');
        }
        Health(Arc::new(Inner {
            prefix,
            lifecycle,
            resources: OnceLock::new(),
        }))
    }

    /// The path prefix the endpoints are mounted under, without a trailing
    /// slash.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.0.prefix
    }

    /// Whether `path` is one of this handle's endpoints.
    ///
    /// Used by the maintenance layer to let probes through a `503`. Matches
    /// the prefix itself and anything below it, and nothing else: `/upload`
    /// is not covered by the `/up` prefix.
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        let prefix = &self.0.prefix;
        path == prefix
            || (path.len() > prefix.len()
                && path.starts_with(prefix.as_str())
                && path.as_bytes()[prefix.len()] == b'/')
    }

    /// Hand the connected subsystems to the readiness probe.
    ///
    /// Called once by startup. A second call is ignored rather than
    /// panicking: the endpoints keep reporting on the first bundle, which is
    /// the one the running application is actually using.
    pub fn publish(&self, resources: Resources) {
        let _ = self.0.resources.set(resources);
    }

    /// The lifecycle this handle reports on.
    #[must_use]
    pub fn lifecycle(&self) -> &Lifecycle {
        &self.0.lifecycle
    }

    /// The routes: `{prefix}`, `{prefix}/live` and `{prefix}/ready`.
    pub fn router<S: RouterState>(&self) -> Router<S> {
        use axum::routing::get;

        let summary = self.clone();
        let live = self.clone();
        let ready = self.clone();

        Router::new()
            .route(
                &self.0.prefix,
                get(move || {
                    let health = summary.clone();
                    async move { health.report().await.into_response() }
                }),
            )
            .route(
                &format!("{}/live", self.0.prefix),
                get(move || {
                    let health = live.clone();
                    async move { health.live().into_response() }
                }),
            )
            .route(
                &format!("{}/ready", self.0.prefix),
                get(move || {
                    let health = ready.clone();
                    async move { health.report().await.into_response() }
                }),
            )
    }

    /// The liveness answer: the lifecycle state, and nothing else.
    #[must_use]
    pub fn live(&self) -> HealthReport {
        HealthReport {
            state: self.0.lifecycle.state(),
            ready: false,
            checks: BTreeMap::new(),
            live_only: true,
        }
    }

    /// The full report: lifecycle state plus one probe per started subsystem.
    ///
    /// Probes run in sequence rather than concurrently. There are at most four
    /// of them, each is a single round trip, and a readiness endpoint that
    /// fans out is a readiness endpoint that can be used to amplify load
    /// against the very dependencies it is checking.
    pub async fn report(&self) -> HealthReport {
        let state = self.0.lifecycle.state();
        // Read only by the feature-gated probes below; with every subsystem
        // feature off there is nothing to probe and the report is the
        // lifecycle state alone.
        let _resources = self.0.resources.get();
        // Every insertion below is behind a subsystem feature, so a build
        // with none of them enabled fills this in exactly nowhere. Declared
        // `mut` regardless rather than duplicating the binding per cfg.
        #[cfg_attr(
            not(any(
                feature = "database",
                feature = "cache",
                feature = "storage-fs",
                feature = "jobs"
            )),
            expect(unused_mut, reason = "every insertion is behind a subsystem feature")
        )]
        let mut checks: BTreeMap<String, Check> = BTreeMap::new();

        #[cfg(feature = "database")]
        if let Some(db) = _resources.and_then(Resources::db) {
            checks.insert("database".to_owned(), Check::from_probe(db.ping().await));
        }

        #[cfg(feature = "cache")]
        if let Some(cache) = _resources.and_then(Resources::cache) {
            checks.insert("cache".to_owned(), Check::from_probe(cache.ping().await));
        }

        #[cfg(feature = "storage-fs")]
        if let Some(storage) = _resources.and_then(Resources::storage) {
            // Every registered disk, not just the default one: an application
            // with `local` healthy and `s3` unreachable is not ready, and a
            // report that only probed the default would say it was.
            //
            // `check` is OpenDAL's own reachability probe -- it does the
            // cheapest operation the backend supports rather than inventing a
            // sentinel object this process would then have to clean up.
            for name in storage.disk_names() {
                let Some(disk) = storage.try_disk(name) else {
                    continue;
                };
                checks.insert(
                    format!("storage:{name}"),
                    Check::from_probe(disk.operator().check().await),
                );
            }
        }

        #[cfg(feature = "jobs")]
        if _resources.and_then(Resources::jobs).is_some() {
            // The queue rides on the database pool, which is already probed
            // above. Reporting it as a second check would double-count one
            // dependency and make a single database blip look like two
            // failures.
            checks.insert("jobs".to_owned(), Check::up());
        }

        let ready = state == LifecycleState::Ready && checks.values().all(Check::is_up);

        HealthReport {
            state,
            ready,
            checks,
            live_only: false,
        }
    }
}

impl std::fmt::Debug for Health {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Health")
            .field("prefix", &self.0.prefix)
            .field("state", &self.0.lifecycle.state())
            .finish_non_exhaustive()
    }
}

/// One subsystem's answer.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// `"up"` or `"down"`.
    pub status: &'static str,
    /// Why it is down. Absent when it is up.
    ///
    /// This is an operator-facing string on an endpoint whose exposure the
    /// operator controls; it is the one place a subsystem error message is
    /// allowed out, because "readiness is false" with no reason is a page
    /// nobody can act on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Check {
    /// A passing check.
    #[must_use]
    pub fn up() -> Self {
        Check {
            status: "up",
            reason: None,
        }
    }

    /// A failing check, with the reason.
    #[must_use]
    pub fn down(reason: impl std::fmt::Display) -> Self {
        Check {
            status: "down",
            reason: Some(reason.to_string()),
        }
    }

    /// Whether this check passed.
    #[must_use]
    pub fn is_up(&self) -> bool {
        self.status == "up"
    }

    /// Turn a probe's `Result` into a check.
    #[cfg_attr(
        not(any(feature = "database", feature = "cache", feature = "storage-fs")),
        expect(dead_code, reason = "every caller is behind a subsystem feature")
    )]
    fn from_probe<T, E: std::fmt::Display>(result: Result<T, E>) -> Self {
        match result {
            Ok(_) => Check::up(),
            Err(error) => Check::down(error),
        }
    }
}

/// The JSON body of every health endpoint.
///
/// The status code carries the verdict -- `200` or `503` -- and the body
/// explains it. A probe that only reads the status code is correct; a
/// dashboard that reads the body gets the detail.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    /// The lifecycle state: `starting`, `ready`, `draining` or `stopped`.
    #[serde(serialize_with = "serialize_state")]
    pub state: LifecycleState,
    /// Whether this instance should receive traffic.
    pub ready: bool,
    /// One entry per started subsystem.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub checks: BTreeMap<String, Check>,
    /// Set on a `/live` report, whose verdict ignores `ready` and `checks`.
    #[serde(skip)]
    live_only: bool,
}

impl HealthReport {
    /// The status code this report answers with.
    ///
    /// A liveness report is `200` for as long as the process has not stopped,
    /// including while it drains: a draining process is still serving the
    /// requests it accepted, and killing it there is what turns a graceful
    /// shutdown into dropped connections.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        let ok = if self.live_only {
            self.state.is_live()
        } else {
            self.ready
        };
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

impl IntoResponse for HealthReport {
    fn into_response(self) -> Response {
        let status = self.status();
        let mut response = (status, axum::Json(self)).into_response();
        // A cached readiness answer is a wrong answer.
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
        response
    }
}

fn serialize_state<S: serde::Serializer>(
    state: &LifecycleState,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(state.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health() -> Health {
        Health::new(DEFAULT_PREFIX, Lifecycle::new())
    }

    #[test]
    fn a_prefix_is_normalised_to_one_leading_and_no_trailing_slash() {
        for given in ["up", "/up", "/up/", "up//"] {
            assert_eq!(Health::new(given, Lifecycle::new()).prefix(), "/up");
        }
    }

    #[test]
    fn covers_matches_the_prefix_and_what_is_under_it() {
        let health = health();
        assert!(health.covers("/up"));
        assert!(health.covers("/up/live"));
        assert!(health.covers("/up/ready"));
    }

    #[test]
    fn covers_does_not_match_a_path_that_merely_starts_with_the_same_letters() {
        // `/upload` is an application route, not a probe, and must still get
        // the maintenance `503`.
        let health = health();
        assert!(!health.covers("/upload"));
        assert!(!health.covers("/"));
        assert!(!health.covers("/updates"));
    }

    #[tokio::test]
    async fn a_starting_application_is_live_but_not_ready() {
        let health = health();
        assert_eq!(health.live().status(), StatusCode::OK);
        assert_eq!(
            health.report().await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn readiness_follows_the_lifecycle() {
        let lifecycle = Lifecycle::new();
        let health = Health::new(DEFAULT_PREFIX, lifecycle.clone());

        lifecycle.mark_ready();
        assert!(health.report().await.ready);
        assert_eq!(health.report().await.status(), StatusCode::OK);

        lifecycle.begin_drain();
        assert!(!health.report().await.ready);
        assert_eq!(
            health.report().await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        // ...but the process is still alive and still finishing in-flight
        // requests, so liveness must not fail and trigger a kill.
        assert_eq!(health.live().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_stopped_application_is_not_live() {
        let lifecycle = Lifecycle::new();
        let health = Health::new(DEFAULT_PREFIX, lifecycle.clone());
        lifecycle.mark_ready();
        lifecycle.begin_drain();
        lifecycle.mark_stopped();
        assert_eq!(health.live().status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn a_down_check_carries_its_reason_and_an_up_one_does_not() {
        assert_eq!(Check::up().reason, None);
        let down = Check::down("connection refused");
        assert!(!down.is_up());
        assert_eq!(down.reason.as_deref(), Some("connection refused"));
    }

    #[test]
    fn publishing_twice_keeps_the_first_bundle() {
        let health = health();
        health.publish(Resources::empty());
        health.publish(Resources::empty());
        // No panic; the endpoints keep reporting on the live bundle.
    }
}
