//! The validated request payloads.
//!
//! Each request is one file. `#[request]` adds `Validate`; `Deserialize` is
//! added by hand so the extractor (`ValidatedJson<T>`) deserializes and
//! validates in one step.

pub mod create_user_request;
pub mod update_user_request;

pub use create_user_request::CreateUserRequest;
pub use update_user_request::UpdateUserRequest;
