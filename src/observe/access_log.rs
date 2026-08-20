//! HTTP access logging middleware.
//!
//! A Tower layer that emits one access log line per request via `tracing`.
//! Records the method, path, status code, duration, and request id (when
//! the [`RequestIdLayer`](super::RequestIdLayer) ran first and inserted the
//! id into request extensions).
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

use super::{REQUEST, RequestId};

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

        let started = Instant::now();

        let inner = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, inner);
        Box::pin(async move {
            let _span = tracing::info_span!(
                REQUEST,
                method = %method,
                path = %uri.path(),
                request_id = %request_id,
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
