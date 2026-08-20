//! Route registration.
//!
//! Returns a [`Routes`] typed over [`AppState`] so handlers can extract
//! `State<AppState>` and reach the database, cache, storage, mail, and jobs.

use arcature::prelude::*;

use crate::app::controllers::home_controller::HomeController;
use crate::bootstrap::AppState;

/// The application routes.
pub fn routes() -> Routes<AppState> {
    Routes::new([Route::get("/", HomeController::index).name("home")])
}
