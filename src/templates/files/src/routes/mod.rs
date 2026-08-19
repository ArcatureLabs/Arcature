use arcature::prelude::*;
use crate::controllers::home;

pub fn routes() -> Routes {
    Routes::new([Route::get("/", home::index).name("home")])
}
