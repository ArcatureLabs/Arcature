//! Server-rendered views: the templates under `templates/`, as Rust types.
//!
//! A view is a struct whose fields are the values its template names, and
//! `#[derive(Template)]` writes the rendering code at build time from the
//! file named in `path`. Paths resolve against `templates/` in the project
//! root.
//!
//! # Why this exists next to `app/pages/`
//!
//! Most of this application's screens are Inertia pages: the server sends
//! props, the client renders. A view is the other answer -- HTML assembled
//! on the server -- and it is the right one for the pages a browser has to
//! be able to show without running any JavaScript: an unsubscribe
//! confirmation, an emailed receipt, an RSS feed, a fallback error page.
//!
//! # Why the templates are compiled
//!
//! A runtime template engine parses and evaluates inside the request path,
//! which is where server-side template injection lives. Askama emits Rust at
//! build time, so the running process has no template parser for hostile
//! input to reach. The price is that editing a template needs a rebuild;
//! `arc dev` already rebuilds on save.
//!
//! One view ships with the scaffold. Add yours here, one struct per
//! template.

// `Template` is both the trait and the `#[derive(Template)]` macro; one
// `use` names both. It is also in `arcature::prelude`, alongside `view`,
// which is what the controller that renders this imports.
use arcature::view::Template;

/// `templates/welcome.html`.
///
/// `askama = arcature::askama` points the derive at the askama the framework
/// pins, so this application does not have to depend on askama itself and
/// cannot drift to a different version of it.
#[derive(Template)]
#[template(path = "welcome.html", askama = arcature::askama)]
pub struct WelcomeView {
    /// The page title, used in `<title>` and in the heading.
    pub title: String,
    /// The body copy.
    pub message: String,
}
