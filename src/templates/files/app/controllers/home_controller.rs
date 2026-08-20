//! The home controller: the welcome page.

use arcature::prelude::*;

#[controller]
impl HomeController {
    /// `GET /`
    pub async fn index() -> Result<Response> {
        Ok(text(StatusCode::OK, "Hello from __PROJECT_NAME__!"))
    }
}
