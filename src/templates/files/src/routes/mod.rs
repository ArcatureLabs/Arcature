//! Route registration.

use arcature::prelude::*;
use crate::app::controllers::home_controller::HomeController;

pub fn routes() -> Routes {
    Routes::new([Route::get("/", HomeController::index).name("home")])
}
