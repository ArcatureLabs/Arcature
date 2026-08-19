//! HTTP response vocabulary and helpers shared by all subsystems.

pub mod response;

pub use response::{no_content, redirect, text, RedirectResponse};
#[cfg(any(feature = "api", feature = "inertia"))]
pub use response::json;
