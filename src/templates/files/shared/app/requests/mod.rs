//! The validated request payloads.
//!
//! Add one file per request with `arc make:request`. `#[request]` adds
//! `Validate`; add `Deserialize` by hand so the extractor deserializes and
//! validates in one step. Example:
//!
//! ```ignore
//! #[request]
//! #[derive(Debug, Clone, Deserialize)]
//! pub struct CreateUserRequest {
//!     #[validate(length(min = 1, max = 255))]
//!     pub name: String,
//!     #[validate(email)]
//!     pub email: String,
//! }
//! ```
