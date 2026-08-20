//! Reads the Vite IPC endpoint from the environment, once at startup.
//!
//! One responsibility: resolve `ARCATURE_VITE_IPC` to an [`IpcEndpoint`] at
//! pipeline-assembly time. This is the resolved-configuration seam for the
//! dev proxy — the env var is read **once** (when [`endpoint_from_env`] is
//! called), never per-request (configuration is explicit and resolved; do
//! not read environment variables inside request handling). The resulting
//! `Option<IpcEndpoint>` is stored in the [`crate::dev_proxy::DevProxyLayer`]
//! for the lifetime of the server.
//!
//! # The env var
//!
//! `arc dev` sets `ARCATURE_VITE_IPC` to the process-private IPC path it
//! created for Vite's `middlewareMode` server (Unix: a socket file under a
//! per-process temp dir; Windows: a `\\.\pipe\arcature-vite-<pid>` name).
//! When the var is unset (production, or `arc dev` not running), the dev
//! proxy is inactive — the layer is a zero-overhead pass-through.
//!
//! # Security
//!
//! The path is process-private and per-invocation; it is never
//! attacker-controlled. We do not validate the path's existence here
//! (connect-time `NotFound` is the honest signal if Vite is not up yet).
//! See the AP2.1-3 security review.

use std::path::PathBuf;

use crate::dev_proxy::endpoint::IpcEndpoint;

/// The environment variable consulted for the Vite IPC endpoint.
///
/// Set by `arc dev` to the IPC path Vite's `middlewareMode` server listens
/// on. Unset in production -> the dev proxy is inactive.
pub(crate) const IPC_ENV: &str = crate::config::VITE_IPC_ENV;

/// Resolve the Vite IPC endpoint from the environment.
///
/// Returns `Some(endpoint)` when `ARCATURE_VITE_IPC` is set to a non-empty
/// value, `None` otherwise. Called once at pipeline-assembly time; the
/// result is stored in the [`crate::dev_proxy::DevProxyLayer`].
///
/// Delegates to [`parse_endpoint`] for the pure string->endpoint mapping;
/// this function is the thin env-reading wrapper (the env is read once
/// here, never per-request).
#[must_use]
pub(crate) fn endpoint_from_env() -> Option<IpcEndpoint> {
    parse_endpoint(std::env::var(IPC_ENV).ok())
}

/// Parse a raw env value into an [`IpcEndpoint`].
///
/// Pure function: `None` or an empty string yields `None`; any non-empty
/// string yields `Some(endpoint)`. Extracted from [`endpoint_from_env`] so
/// the parsing logic is testable without mutating the process environment
/// (which is forbidden by `#![forbid(unsafe_code)]`).
#[must_use]
pub(crate) fn parse_endpoint(raw: Option<String>) -> Option<IpcEndpoint> {
    raw.filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .map(IpcEndpoint::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_none_yields_none() {
        assert!(parse_endpoint(None).is_none());
    }

    #[test]
    fn parse_empty_yields_none() {
        assert!(parse_endpoint(Some(String::new())).is_none());
    }

    #[test]
    fn parse_nonempty_yields_endpoint() {
        let endpoint = parse_endpoint(Some(String::from("/tmp/arcature-vite-test.sock")))
            .expect("non-empty value should yield an endpoint");
        assert_eq!(
            endpoint.path(),
            std::path::Path::new("/tmp/arcature-vite-test.sock")
        );
    }

    #[test]
    fn parse_windows_pipe_name() {
        let endpoint = parse_endpoint(Some(String::from(r"\\.\pipe\arcature-vite-42")))
            .expect("Windows pipe name should yield an endpoint");
        assert_eq!(
            endpoint.path(),
            std::path::Path::new(r"\\.\pipe\arcature-vite-42")
        );
    }
}
