//! W3C Trace Context: parsing, generating, and propagating `traceparent`.
//!
//! A distributed trace only joins up if every hop agrees on the wire format.
//! The W3C recommendation is that agreement: one `traceparent` header
//! carrying the trace id, the caller's span id and the sampling flag, and an
//! optional `tracestate` carrying vendor-specific key/value pairs.
//!
//! Inbound headers are untrusted. A malformed `traceparent` is discarded and
//! a fresh root context is started rather than propagated, because half a
//! trace id is worse than none: it silently corrupts every trace it joins.
//! The `tracestate` header is length- and member-capped for the same reason
//! -- it is attacker-controlled text that would otherwise be copied onto
//! every outbound request the service makes.

use std::convert::Infallible;
use std::fmt;

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;
use tower::{Layer, Service};

/// The `traceparent` header name.
pub const TRACEPARENT: HeaderName = HeaderName::from_static("traceparent");
/// The `tracestate` header name.
pub const TRACESTATE: HeaderName = HeaderName::from_static("tracestate");

/// The sampled flag, bit 0 of the trace-flags octet.
pub const FLAG_SAMPLED: u8 = 0x01;

/// The specification's cap on `tracestate` members.
pub const MAX_TRACESTATE_MEMBERS: usize = 32;

/// A parsed `traceparent`.
///
/// The version field is kept so a future version can be recognised, but only
/// version `00` is emitted: forwarding a version this code does not
/// understand under its own name would be a lie about what the fields mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceParent {
    trace_id: [u8; 16],
    parent_id: [u8; 8],
    flags: u8,
}

impl TraceParent {
    /// A fresh root context, sampled.
    ///
    /// Randomness comes from `getrandom` when the `oauth` feature has pulled
    /// it in, and otherwise from a hash of the current instant and the
    /// thread id. Trace ids need to be unique, not unpredictable -- they
    /// carry no authority and grant no access -- so the fallback is
    /// adequate, and this module refuses to require a crypto dependency for
    /// a correlation identifier.
    #[must_use]
    pub fn root() -> Self {
        let mut bytes = [0_u8; 24];
        fill_unique(&mut bytes);
        let mut trace_id = [0_u8; 16];
        let mut parent_id = [0_u8; 8];
        trace_id.copy_from_slice(&bytes[..16]);
        parent_id.copy_from_slice(&bytes[16..]);
        // An all-zero id is invalid per the specification; the odds are
        // negligible but the check is one comparison.
        if trace_id == [0; 16] {
            trace_id[15] = 1;
        }
        if parent_id == [0; 8] {
            parent_id[7] = 1;
        }
        Self {
            trace_id,
            parent_id,
            flags: FLAG_SAMPLED,
        }
    }

    /// A child of this context: same trace, new span id, same flags.
    #[must_use]
    pub fn child(&self) -> Self {
        let mut bytes = [0_u8; 8];
        fill_unique(&mut bytes);
        if bytes == [0; 8] {
            bytes[7] = 1;
        }
        Self {
            trace_id: self.trace_id,
            parent_id: bytes,
            flags: self.flags,
        }
    }

    /// Parse a `traceparent` header value.
    ///
    /// Returns `None` for anything that is not a well-formed, non-zero
    /// version-`00` header. Callers start a root context instead.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        // 2 + 1 + 32 + 1 + 16 + 1 + 2
        if value.len() != 55 {
            return None;
        }
        let bytes = value.as_bytes();
        if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
            return None;
        }
        let version = hex_byte(&bytes[0..2])?;
        // `ff` is reserved and never valid.
        if version != 0 {
            return None;
        }
        let mut trace_id = [0_u8; 16];
        for (index, slot) in trace_id.iter_mut().enumerate() {
            *slot = hex_byte(&bytes[3 + index * 2..5 + index * 2])?;
        }
        let mut parent_id = [0_u8; 8];
        for (index, slot) in parent_id.iter_mut().enumerate() {
            *slot = hex_byte(&bytes[36 + index * 2..38 + index * 2])?;
        }
        if trace_id == [0; 16] || parent_id == [0; 8] {
            return None;
        }
        let flags = hex_byte(&bytes[53..55])?;
        Some(Self {
            trace_id,
            parent_id,
            flags,
        })
    }

    /// The trace id, lowercase hex.
    #[must_use]
    pub fn trace_id(&self) -> String {
        hex(&self.trace_id)
    }

    /// The parent span id, lowercase hex.
    #[must_use]
    pub fn parent_id(&self) -> String {
        hex(&self.parent_id)
    }

    /// Whether the sampled flag is set.
    #[must_use]
    pub fn sampled(&self) -> bool {
        self.flags & FLAG_SAMPLED != 0
    }

    /// The header value to send downstream.
    #[must_use]
    pub fn to_header_value(&self) -> HeaderValue {
        // Every byte is hex or a dash, so this cannot fail; the fallback
        // keeps the signature infallible without an unwrap.
        HeaderValue::from_str(&self.to_string()).unwrap_or(HeaderValue::from_static(""))
    }
}

impl fmt::Display for TraceParent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "00-{}-{}-{:02x}",
            hex(&self.trace_id),
            hex(&self.parent_id),
            self.flags
        )
    }
}

/// A validated `tracestate` header.
///
/// Stored as the original text rather than a parsed member list because the
/// only operations this crate performs on it are "carry it forward" and
/// "prepend our own member", and re-serialising a parsed form risks
/// normalising away something a downstream vendor depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceState(String);

impl TraceState {
    /// Validate an inbound `tracestate`.
    ///
    /// Returns `None` when the header is empty, has more members than the
    /// specification allows, or contains a byte outside the printable ASCII
    /// range the grammar permits. A rejected `tracestate` is dropped; the
    /// trace itself still propagates.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if value.len() > 512 {
            return None;
        }
        let members = value.split(',').filter(|m| !m.trim().is_empty()).count();
        if members == 0 || members > MAX_TRACESTATE_MEMBERS {
            return None;
        }
        // The grammar allows printable ASCII other than comma and equals
        // inside a value; rather than re-implement the full member grammar,
        // reject anything outside printable ASCII, which is enough to stop
        // header injection and control characters.
        if !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return None;
        }
        Some(Self(value.to_string()))
    }

    /// The header text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TraceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The trace context for one request: the parent, and any vendor state.
///
/// Placed in request extensions by [`TraceContextLayer`]. A handler reads it
/// to correlate its own logs, and calls [`outbound_headers`] when it makes a
/// downstream call so the trace continues.
///
/// [`outbound_headers`]: TraceContext::outbound_headers
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceContext {
    parent: TraceParent,
    state: Option<TraceState>,
    /// Whether the parent came from the caller or was invented here. A
    /// service at the edge starts roots; one behind a gateway should not,
    /// and this makes the difference visible in a log line.
    continued: bool,
}

impl TraceContext {
    /// Extract the context from request headers, or start a new root.
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let parent = headers
            .get(&TRACEPARENT)
            .and_then(|value| value.to_str().ok())
            .and_then(TraceParent::parse);
        match parent {
            Some(parent) => Self {
                parent,
                state: headers
                    .get(&TRACESTATE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(TraceState::parse),
                continued: true,
            },
            // A `tracestate` without a valid `traceparent` is meaningless --
            // it describes a trace this request is not part of.
            None => Self {
                parent: TraceParent::root(),
                state: None,
                continued: false,
            },
        }
    }

    /// The `traceparent` of this request.
    #[must_use]
    pub fn parent(&self) -> &TraceParent {
        &self.parent
    }

    /// The vendor state, if the caller sent a usable one.
    #[must_use]
    pub fn state(&self) -> Option<&TraceState> {
        self.state.as_ref()
    }

    /// Whether an upstream trace was joined rather than started.
    #[must_use]
    pub fn continued(&self) -> bool {
        self.continued
    }

    /// The headers to put on a downstream request.
    ///
    /// The span id is a fresh child, which is what makes the downstream hop
    /// a child of this one rather than a sibling of the caller.
    #[must_use]
    pub fn outbound_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(TRACEPARENT, self.parent.child().to_header_value());
        if let Some(state) = &self.state
            && let Ok(value) = HeaderValue::from_str(state.as_str())
        {
            headers.insert(TRACESTATE, value);
        }
        headers
    }
}

// ---------------------------------------------------------------------------
// The extraction layer
// ---------------------------------------------------------------------------

/// A Tower layer that resolves the trace context and records it on the span.
///
/// It inserts a [`TraceContext`] into request extensions and opens a span
/// carrying `trace_id` and `parent_span_id`, so every log line emitted while
/// handling the request can be joined to the trace by id alone.
///
/// Nothing is written back onto the response: `traceparent` is a request
/// header, and echoing it would tell a client what the internal trace ids
/// are for no benefit.
#[derive(Debug, Clone, Copy, Default)]
pub struct TraceContextLayer;

impl<S> Layer<S> for TraceContextLayer {
    type Service = TraceContextService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TraceContextService { inner }
    }
}

/// The service produced by [`TraceContextLayer`].
#[derive(Debug, Clone)]
pub struct TraceContextService<S> {
    inner: S,
}

impl<S> Service<Request> for TraceContextService<S>
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

    fn call(&mut self, mut request: Request) -> Self::Future {
        let context = TraceContext::from_headers(request.headers());
        let trace_id = context.parent().trace_id();
        let parent_span_id = context.parent().parent_id();
        let continued = context.continued();
        request.extensions_mut().insert(context);

        // Swap in the clone and drive the original: only the original is
        // known ready, and `poll_ready` readiness does not survive cloning.
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            let span = tracing::info_span!(
                super::REQUEST,
                trace_id = %trace_id,
                parent_span_id = %parent_span_id,
                continued_trace = continued,
            );
            let _entered = span.enter();
            inner.call(request).await
        })
    }
}

// ---------------------------------------------------------------------------
// Hex helpers
// ---------------------------------------------------------------------------

/// Lowercase hex, which is the only case the specification accepts on the
/// wire.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

/// Decode exactly two hex digits. Uppercase is rejected: the specification
/// says lowercase, and accepting both would make two spellings of one id.
fn hex_byte(pair: &[u8]) -> Option<u8> {
    fn nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        }
    }
    Some((nibble(*pair.first()?)? << 4) | nibble(*pair.get(1)?)?)
}

/// Fill `bytes` with values unlikely to repeat within a deployment.
///
/// Uses `getrandom` where the build has it. The fallback mixes the
/// monotonic clock, the wall clock, a per-process counter and the address of
/// a stack local through a SplitMix64 step; that is a correlation id, not a
/// secret, and the doc comment on [`TraceParent::root`] says so.
fn fill_unique(bytes: &mut [u8]) {
    #[cfg(feature = "oauth")]
    if getrandom::fill(bytes).is_ok() {
        return;
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let local = 0_u8;
    let mut seed = COUNTER.fetch_add(1, Ordering::Relaxed)
        ^ (std::ptr::from_ref(&local) as u64)
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
    for chunk in bytes.chunks_mut(8) {
        seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = seed;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^= mixed >> 31;
        let source = mixed.to_le_bytes();
        chunk.copy_from_slice(&source[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn a_well_formed_traceparent_round_trips() {
        let parsed = TraceParent::parse(SAMPLE).expect("valid header");
        assert_eq!(parsed.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(parsed.parent_id(), "00f067aa0ba902b7");
        assert!(parsed.sampled());
        assert_eq!(parsed.to_string(), SAMPLE);
    }

    #[test]
    fn an_unsampled_flag_is_preserved() {
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
        let parsed = TraceParent::parse(header).expect("valid header");
        assert!(!parsed.sampled());
        assert_eq!(parsed.to_string(), header);
    }

    #[test]
    fn malformed_traceparents_are_all_rejected() {
        for header in [
            "",
            "garbage",
            // Wrong length.
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0",
            // Zero trace id.
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            // Zero parent id.
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            // Unknown version.
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            // Reserved version.
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            // Uppercase hex.
            "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01",
            // Wrong separators.
            "00:4bf92f3577b34da6a3ce929d0e0e4736:00f067aa0ba902b7:01",
        ] {
            assert!(TraceParent::parse(header).is_none(), "accepted {header:?}");
        }
    }

    #[test]
    fn a_child_keeps_the_trace_and_changes_the_span() {
        let parent = TraceParent::parse(SAMPLE).expect("valid header");
        let child = parent.child();
        assert_eq!(child.trace_id(), parent.trace_id());
        assert_ne!(child.parent_id(), parent.parent_id());
        assert_eq!(child.sampled(), parent.sampled());
    }

    #[test]
    fn two_roots_do_not_collide() {
        let one = TraceParent::root();
        let two = TraceParent::root();
        assert_ne!(one.trace_id(), two.trace_id());
        assert_ne!(one.trace_id(), "0".repeat(32));
    }

    #[test]
    fn a_missing_traceparent_starts_a_root() {
        let context = TraceContext::from_headers(&HeaderMap::new());
        assert!(!context.continued());
        assert!(context.state().is_none());
    }

    #[test]
    fn a_tracestate_without_a_traceparent_is_discarded() {
        let mut headers = HeaderMap::new();
        headers.insert(TRACESTATE, HeaderValue::from_static("vendor=value"));
        let context = TraceContext::from_headers(&headers);
        assert!(!context.continued());
        assert!(context.state().is_none());
    }

    #[test]
    fn a_valid_pair_is_joined_and_carried_forward() {
        let mut headers = HeaderMap::new();
        headers.insert(TRACEPARENT, HeaderValue::from_static(SAMPLE));
        headers.insert(TRACESTATE, HeaderValue::from_static("vendor=value"));
        let context = TraceContext::from_headers(&headers);
        assert!(context.continued());
        assert_eq!(
            context.state().map(TraceState::as_str),
            Some("vendor=value")
        );

        let outbound = context.outbound_headers();
        let forwarded = outbound
            .get(&TRACEPARENT)
            .and_then(|v| v.to_str().ok())
            .and_then(TraceParent::parse)
            .expect("a valid outbound header");
        assert_eq!(forwarded.trace_id(), context.parent().trace_id());
        assert_ne!(forwarded.parent_id(), context.parent().parent_id());
        assert_eq!(outbound.get(&TRACESTATE).unwrap(), "vendor=value");
    }

    #[test]
    fn an_oversized_or_hostile_tracestate_is_refused() {
        let too_many = (0..40)
            .map(|i| format!("v{i}=x"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(TraceState::parse(&too_many).is_none());
        assert!(TraceState::parse(&"a=b,".repeat(400)).is_none());
        assert!(TraceState::parse("vendor=value\r\ninjected: yes").is_none());
        assert!(TraceState::parse("   ").is_none());
        assert!(TraceState::parse("vendor=value").is_some());
    }
}
