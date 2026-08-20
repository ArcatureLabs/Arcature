//! The home controller: the welcome page.

use arcature::prelude::*;

/// The home controller.
pub struct HomeController;

#[controller]
impl HomeController {
    /// `GET /`
    pub async fn index() -> Result<Response> {
        Ok(text(StatusCode::OK, "Hello from __PROJECT_NAME__!"))
    }
}
