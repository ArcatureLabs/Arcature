//! HTTP response vocabulary and helpers shared by all subsystems.

pub mod response;
pub mod security;

#[cfg(any(feature = "api", feature = "inertia"))]
pub use response::json;
pub use response::{RedirectResponse, no_content, redirect, text};
pub use security::SecurityHeaders;
