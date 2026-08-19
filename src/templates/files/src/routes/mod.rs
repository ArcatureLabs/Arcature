use arcature::prelude::*;

use crate::controllers;

pub fn routes() -> Routes {
    Routes::new([Route::get("/", controllers::index).name("home")])
}
