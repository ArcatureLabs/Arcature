//! The application's authorization policies.
//!
//! Each policy is one file: a `Policy<M>` impl that answers whether a user may
//! perform an action on a resource. The controller calls
//! `auth.authorize(&policy, action, &resource)`.

pub mod user_policy;

pub use user_policy::UserPolicy;
