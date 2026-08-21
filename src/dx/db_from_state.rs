//! `DbFromState<S>` -- how to obtain a [`Db`] from Axum state.
//!
//! This trait is the seam between the [`Bound<T>`](super::bound::Bound)
//! extractor / [`Resolve<S>`](super::resolve::Resolve) impl for `Db`
//! and the application's state type. It lives in its own file (not
//! inside `bound.rs`) so it is available behind `dx` + `database` alone --
//! `Bound<T>` additionally requires `api` (for Problem responses), but
//! `DbFromState` and the `Resolve<Db>` impl do not.
//!
//! This is a deliberate Arcature trait (not `axum::extract::FromRef`) so
//! the `arcature` crate can provide the `Db`-direct impl without running
//! afoul of Rust's orphan rules (`FromRef` and `Db` are both foreign
//! types). Applications implementing their own composite state provide
//! their own `DbFromState` impl -- one line of code.

use crate::database::Db;

/// How to obtain a [`Db`] from Axum state `S`.
///
/// The simplest case is `impl DbFromState<Db> for Db` (the state IS `Db`);
/// the common case is `impl DbFromState<AppState> for Db` (the state
/// wraps `Db` as a field).
///
/// # Example
///
/// ```
/// use arcature::{Db, DbFromState};
///
/// // The state IS `Db`: Arcature provides that impl, shown here for contrast.
/// // impl DbFromState<Db> for Db { fn db_from_state(state: &Db) -> Db { state.clone() } }
///
/// // The state wraps `Db`: the application writes this.
/// #[derive(Clone)]
/// struct AppState {
///     db: Db,
/// }
///
/// impl DbFromState<AppState> for Db {
///     fn db_from_state(state: &AppState) -> Db {
///         state.db.clone()
///     }
/// }
/// # fn main() {}
/// ```
pub trait DbFromState<S>: Send + Sync + 'static {
    /// Extract a `Db` handle from the state.
    fn db_from_state(state: &S) -> Db;
}

/// The simplest case: the state IS `Db`.
impl DbFromState<Db> for Db {
    fn db_from_state(state: &Db) -> Db {
        state.clone()
    }
}
