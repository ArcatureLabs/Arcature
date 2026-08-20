//! The application's authorization policies.
//!
//! Add one file per policy here: a `Policy<M>` impl that answers whether a
//! user may perform an action on a resource. The controller calls
//! `auth.authorize::<Policy>("action", &resource)`.
