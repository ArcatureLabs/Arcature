//! The one-TCP-port development proxy (AP2.1-3).
//!
//! In development, Vite runs in `middlewareMode` over an IPC endpoint (Unix
//! socket / Windows named pipe) — it owns **no TCP port**. The Rust
//! application owns the single TCP listener and installs this dev proxy as
//! the outermost pre-routing layer: requests that look like Vite's
//! (`/@vite/`, `/src/...`, HMR WebSocket upgrade) are forwarded to Vite over
//! IPC; everything else reaches the application pipeline unchanged. The
//! browser sees one origin — assets, HMR, and the app all come from the
//! same host:port.
//!
//! # Module layout (one file = one responsibility)
//!
//! - [`endpoint`] — the IPC path and `connect()` (transport boundary).
//! - [`vite`] — pure `is_vite_request` detection (routing decision).
//! - [`service`] — the Tower layer/service that forwards or delegates.
//! - [`config`] — reads `ARCATURE_VITE_IPC` once at startup (resolved
//!   configuration).
//!
//! # Dev-only, feature-gated
//!
//! The whole module is behind the `dev-proxy` Cargo feature. The layer is
//! inactive unless `ARCATURE_VITE_IPC` is set (production builds that enable
//! the feature pay only the cost of one `Option` check per request). The
//! feature pulls `hyper` (client) and `hyper-util` — both already in the
//! compiled graph via axum, so no new supply-chain surface (see the AP2.1-3
//! dependency review).
//!
//! # Security
//!
//! See [`service`] and the AP2.1-3 security review. The IPC path is
//! process-private and per-invocation; forwarding is gated by a pure
//! function of the request path/headers; mid-request Vite failures return a
//! redacted 502; connect-time failures fall back to the application 404.

pub(crate) mod config;
pub(crate) mod endpoint;
pub(crate) mod service;
pub(crate) mod vite;

// The dev proxy is engine plumbing, not an application API. Only the layer
// is re-exported (the pipeline assembler constructs it). `IpcEndpoint` and
// `DevProxyService` are `pub(crate)`-reachable via their module paths; no
// crate-root re-export is needed.
pub use service::DevProxyLayer;
