//! HTTP response vocabulary and helpers shared by all subsystems.

pub mod client_ip;
#[cfg(feature = "uploads")]
pub mod download;
pub mod error_mapping;
pub mod maintenance;
#[cfg(feature = "uploads")]
pub mod multipart;
pub mod response;
pub mod security;

pub use client_ip::{ClientIp, ProxyNet, ProxyNetError, TrustedProxies, X_FORWARDED_FOR};
#[cfg(feature = "uploads")]
pub use download::Attachment;
pub use error_mapping::{ErrorMapping, Mapper};
pub use maintenance::Maintenance;
#[cfg(feature = "uploads")]
pub use multipart::{BoundedField, BoundedMultipart, MultipartError, MultipartLimits};
#[cfg(any(feature = "api", feature = "inertia"))]
pub use response::json;
pub use response::{RedirectResponse, no_content, redirect, text};
pub use security::{CspNonce, CspNonceMissing, CspTemplateError, SecurityHeaders};
