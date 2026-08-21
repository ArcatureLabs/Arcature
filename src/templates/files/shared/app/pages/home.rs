//! The welcome page.

use arcature::prelude::*;

/// Props for the `home` component.
///
/// Every field here crosses the wire and is visible in the browser's page
/// data, so this struct is the exposure boundary for what `/` sends.
#[page("home")]
pub struct HomePage {
    /// The greeting shown in the hero.
    pub message: String,
    /// The configured application name, rendered in the navigation bar.
    pub app_name: String,
    /// The Arcature version this project was generated against.
    pub arcature_version: String,
}
