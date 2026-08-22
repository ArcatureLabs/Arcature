//! HTTP access logging middleware.
//!
//! A Tower layer that emits one access log line per request via `tracing`.
//! Records the method, path, status code, duration, the request id (when
//! the [`RequestIdLayer`](super::RequestIdLayer) ran first and inserted the
//! id into request extensions) and the client address (when the serve path
//! resolved one into [`ClientIp`](crate::http::ClientIp)).
//!
//! # The client address is a field, never the message
//!
//! An IP address is personal data in most of the places this will run, and
//! an access log is the single most-copied artefact a service produces.
//! [`redact`](super::redact) is what decides whether a value may travel,
//! and it decides per *field name* -- so an address interpolated into the
//! human-readable message would be past the only checkpoint there is. It is
//! emitted as the structured `client_ip` field alone, and passed through
//! [`redact::apply`](super::redact::apply) on the way, so that adding an
//! address term to the deny-list is all it would take to withhold it
//! everywhere rather than everywhere-except-the-message.
//!
//! One file, one responsibility: the access-log layer and service. The
//! [`RequestIdLayer`] and span-name constants live in [`super`].
//!
//! [`RequestIdLayer`]: super::RequestIdLayer

use std::convert::Infallible;
use std::time::Instant;

use axum::extract::Request;
use axum::response::Response;
use tower::{Layer, Service};

use super::{REQUEST, RequestId, redact};

/// The field name the client address is logged under, and the name
/// [`redact`] is asked about. The two must be the same string or the
/// deny-list would be consulted about a field that is not the one written.
const CLIENT_IP: &str = "client_ip";

/// A Tower layer that logs each request as an access line.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccessLogLayer;

impl<S> Layer<S> for AccessLogLayer {
    type Service = AccessLogService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AccessLogService { inner }
    }
}

/// The service produced by [`AccessLogLayer`].
#[derive(Debug, Clone)]
pub struct AccessLogService<S> {
    inner: S,
}

impl<S> Service<Request> for AccessLogService<S>
where
    S: Service<Request, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request) -> Self::Future {
        let method = req.method().clone();
        let uri = req.uri().clone();
        let request_id = req
            .extensions()
            .get::<RequestId>()
            .map(|id| id.to_string())
            .unwrap_or_default();
        // Empty rather than absent when nothing resolved an address, as with
        // the request id above: a reader can tell "not known here" from "the
        // field was never part of this log".
        let client_ip = req
            .extensions()
            .get::<crate::http::ClientIp>()
            .map(|client| client.addr().to_string())
            .unwrap_or_default();

        let started = Instant::now();

        let inner = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, inner);
        Box::pin(async move {
            let _span = tracing::info_span!(
                REQUEST,
                method = %method,
                path = %uri.path(),
                request_id = %request_id,
                client_ip = %redact::apply(CLIENT_IP, &client_ip),
            );
            let response = inner.call(req).await?;

            let status = response.status();
            let duration = started.elapsed();
            tracing::info!(
                method = %method,
                path = %uri.path(),
                status = status.as_u16(),
                duration_ms = duration.as_millis() as u64,
                request_id = %request_id,
                client_ip = %redact::apply(CLIENT_IP, &client_ip),
                // The address is deliberately absent from the message.
                "{} {} {} {}ms",
                method,
                uri.path(),
                status.as_u16(),
                duration.as_millis(),
            );
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt as _;

    /// An inner service that answers `200` and nothing else. Hand-written
    /// rather than `tower::service_fn`, which needs a tower feature this
    /// crate does not turn on.
    #[derive(Clone)]
    struct Ok200;

    impl Service<Request> for Ok200 {
        type Response = Response;
        type Error = Infallible;
        type Future = std::future::Ready<Result<Response, Infallible>>;

        fn poll_ready(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request) -> Self::Future {
            std::future::ready(Ok(Response::new(axum::body::Body::empty())))
        }
    }

    /// Log one `GET /` through the layer and return what the JSON sink saw.
    fn log_one(client: Option<crate::http::ClientIp>) -> String {
        let sink = super::super::CaptureSink::new();
        let subscriber =
            tracing_subscriber::registry().with(super::super::JsonLog::new(sink.clone()));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(async {
                let mut service = AccessLogLayer.layer(Ok200);
                let mut request = Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .expect("request builds");
                if let Some(client) = client {
                    request.extensions_mut().insert(client);
                }
                let _ = service.call(request).await.expect("the inner service");
            });
        });
        sink.lines().join("\n")
    }

    #[test]
    fn the_client_address_is_logged_as_its_own_field() {
        let client = crate::http::ClientIp::resolve(
            "203.0.113.9".parse().expect("a literal address"),
            &axum::http::HeaderMap::new(),
            &crate::http::TrustedProxies::none(),
        );
        let line = log_one(Some(client));
        assert!(
            line.contains("\"client_ip\":\"203.0.113.9\""),
            "no client_ip field in {line}"
        );
        // Exactly once: in the field, and not also inside the rendered
        // message, where the deny-list cannot reach it.
        assert_eq!(
            line.matches("203.0.113.9").count(),
            1,
            "the address leaked into the message: {line}"
        );
    }

    #[test]
    fn an_unresolved_address_logs_an_empty_field_rather_than_a_wrong_one() {
        let line = log_one(None);
        assert!(
            line.contains("\"client_ip\":\"\""),
            "no empty client_ip field in {line}"
        );
    }

    #[test]
    fn the_field_name_is_the_one_redaction_is_asked_about() {
        // If `client_ip` is ever added to the deny-list, this line is what
        // makes the log honour it; a mismatch here would silently opt the
        // address out of redaction.
        assert_eq!(redact::apply(CLIENT_IP, "203.0.113.9"), "203.0.113.9");
        assert!(!redact::is_sensitive(CLIENT_IP));
    }
}
