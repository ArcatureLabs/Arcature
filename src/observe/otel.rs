//! The OpenTelemetry pipeline: OTLP export over gRPC, wired to `tracing`.
//!
//! Everything here is a handle the application holds. `opentelemetry` offers
//! a global tracer provider and this module does not use it: a global would
//! make two applications in one process share a pipeline, make a test unable
//! to isolate its spans, and make shutdown a question of who calls it last.
//! The caller builds a [`Telemetry`], keeps it alive for as long as the
//! application runs, and calls [`Telemetry::shutdown`] before exiting so the
//! batch processor gets a chance to flush.
//!
//! Spans exported here carry the same field names the JSON log layer writes,
//! and the same deny-list applies: a field the log layer redacts is a field
//! that must not be recorded on a span either, because an OTLP collector is
//! just another log sink with a different wire format.
//!
//! The exporter needs a Tokio runtime to be running when it is built, since
//! the batch processor spawns a background task. Build the pipeline inside
//! the async entry point, not in a `static` initialiser.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};

use super::ObserveError;

/// The default OTLP gRPC endpoint, which is what a local collector listens
/// on out of the box.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4317";

/// A configured tracing pipeline.
///
/// Holding this value is what keeps the exporter alive. Dropping it without
/// calling [`shutdown`](Self::shutdown) loses whatever is still buffered.
#[derive(Debug)]
pub struct Telemetry {
    provider: SdkTracerProvider,
    service_name: String,
}

impl Telemetry {
    /// Start configuring a pipeline for `service_name`.
    ///
    /// The service name is the one attribute every backend groups by, so it
    /// is required rather than defaulted -- a deployment reporting as
    /// `unknown_service` is a deployment nobody can find.
    #[must_use]
    pub fn builder(service_name: impl Into<String>) -> TelemetryBuilder {
        TelemetryBuilder {
            service_name: service_name.into(),
            endpoint: None,
            timeout: None,
        }
    }

    /// A tracer for this application's instrumentation scope.
    #[must_use]
    pub fn tracer(&self) -> SdkTracer {
        self.provider.tracer(self.service_name.clone())
    }

    /// The `tracing` layer that turns spans into OpenTelemetry spans.
    ///
    /// Compose it onto a `Registry` alongside
    /// [`JsonLog`](super::json_log::JsonLog): one subscriber, two
    /// destinations, one set of spans.
    #[must_use]
    pub fn tracing_layer<S>(&self) -> tracing_opentelemetry::OpenTelemetryLayer<S, SdkTracer>
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        tracing_opentelemetry::layer().with_tracer(self.tracer())
    }

    /// Flush and stop the exporter.
    ///
    /// Call this on the way out of `main`. It is fallible because a
    /// collector that is already gone cannot accept the final batch, and
    /// that is worth reporting rather than swallowing.
    pub fn shutdown(self) -> Result<(), ObserveError> {
        self.provider
            .shutdown()
            .map_err(|_| ObserveError::Telemetry {
                reason: "the tracer provider did not shut down cleanly",
            })
    }
}

/// Configuration for a [`Telemetry`] pipeline.
#[derive(Debug, Clone)]
pub struct TelemetryBuilder {
    service_name: String,
    endpoint: Option<String>,
    timeout: Option<std::time::Duration>,
}

impl TelemetryBuilder {
    /// The OTLP gRPC endpoint. Defaults to [`DEFAULT_ENDPOINT`], or to
    /// whatever `OTEL_EXPORTER_OTLP_ENDPOINT` says when this is not set.
    #[must_use]
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// How long one export attempt may take.
    #[must_use]
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Build the pipeline.
    ///
    /// Must be called with a Tokio runtime already running: the batch
    /// processor spawns onto it.
    pub fn build(self) -> Result<Telemetry, ObserveError> {
        let mut exporter = SpanExporter::builder().with_tonic();
        if let Some(endpoint) = &self.endpoint {
            exporter = exporter.with_endpoint(endpoint.clone());
        }
        if let Some(timeout) = self.timeout {
            exporter = exporter.with_timeout(timeout);
        }
        let exporter = exporter.build().map_err(|_| ObserveError::Telemetry {
            reason: "the OTLP span exporter could not be built",
        })?;

        let resource = Resource::builder()
            .with_service_name(self.service_name.clone())
            .build();
        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource)
            .build();

        Ok(Telemetry {
            provider,
            service_name: self.service_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_builder_keeps_the_service_name_and_endpoint() {
        let builder = Telemetry::builder("checkout").endpoint("http://collector:4317");
        assert_eq!(builder.service_name, "checkout");
        assert_eq!(builder.endpoint.as_deref(), Some("http://collector:4317"));
    }

    // Building the pipeline is not tested here: `with_batch_exporter`
    // spawns onto a Tokio runtime, and this crate's test profile has no
    // runtime under the feature set that enables `otel`. An exporter test
    // would need a live collector, which is an integration concern.
}
