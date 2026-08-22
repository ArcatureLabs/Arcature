//! Route registration.
//!
//! One `routes!` block generates three items from the table below:
//!
//! - `app_routes() -> Routes<AppState>`, which `bootstrap/app.rs` mounts;
//! - `APP_ROUTES`, the descriptor slice `arc routes` and the module graph
//!   read;
//! - `app_route`, typed URL helpers, so `app_route::home()` is a compile
//!   error when the route is renamed rather than a 404 at runtime.
//!
//! Two routes ship with the scaffold: an Inertia page and a
//! server-rendered view. Add yours here; a `group` takes a path
//! prefix and shared middleware, and `resource` expands to the seven REST
//! actions of a controller.

use arcature::prelude::*;

use crate::app::controllers::HomeController;
use crate::bootstrap::AppState;

routes! {
    pub app {
        state: AppState;

        get "/" => HomeController::index { name: home }
        get "/welcome" => HomeController::welcome { name: welcome }
    }
}
