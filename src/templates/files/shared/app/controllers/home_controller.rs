//! The home controller: the one example route the scaffold ships with.

use arcature::prelude::*;

use crate::app::pages::HomePage;
use crate::bootstrap::AppState;

/// The home controller.
pub struct HomeController;

#[controller]
impl HomeController {
    /// `GET /` -- the welcome page.
    ///
    /// Returning `Page<HomePage>` rather than a `Response` is what puts this
    /// handler's page identity into the controller metadata: the return type
    /// names the component, so a page the client does not have fails at
    /// compile time rather than on the first visit.
    pub async fn index(State(state): State<AppState>) -> Result<Page<HomePage>> {
        Ok(page(HomePage {
            message: "Welcome to __PROJECT_NAME__".to_string(),
            app_name: state.app_name.clone(),
            // Substituted when the project is generated, so the page always
            // reports the framework version the scaffold was cut from.
            arcature_version: "__ARCATURE_VERSION__".to_string(),
        }))
    }
}
