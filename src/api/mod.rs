//! API subsystem: RFC 9457 problem details and typed API responses.
//!
//! [`problem`] is always available (it needs only always-on deps and the
//! validation subsystem depends on it). Additional API conveniences are
//! gated on the `api` feature.

mod kind;
pub mod problem;

pub use kind::ProblemKind;
pub use problem::{PROBLEM_JSON, Problem, ProblemBuilder};
