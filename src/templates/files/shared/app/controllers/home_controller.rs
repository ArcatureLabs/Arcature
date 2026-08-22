//! The home controller: the one example route the scaffold ships with.

use arcature::prelude::*;

use crate::app::pages::HomePage;
use crate::app::views::WelcomeView;
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

    /// `GET /welcome` -- the one server-rendered page the scaffold ships
    /// with.
    ///
    /// The return type is `Response` rather than `Page<..>` because there is
    /// no client component behind it: the HTML is finished when it leaves
    /// the server, which is what makes a view the right answer for a screen
    /// that has to work with JavaScript turned off.
    ///
    /// A view that fails to render answers `500` with the framework's
    /// ordinary error body; the template text and its path go to the log and
    /// never to the browser.
    pub async fn welcome(State(state): State<AppState>) -> Result<Response> {
        Ok(view(WelcomeView {
            title: state.app_name.clone(),
            message: "This page was rendered on the server, from \n                      templates/welcome.html."
                .to_string(),
        })
        .into_response())
    }
}
