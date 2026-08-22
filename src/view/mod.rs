//! Compiled HTML views: server-rendered templates that are Rust code by the
//! time the binary exists.
//!
//! # Why the templates are compiled
//!
//! Every runtime template engine -- minijinja, tera, handlebars -- ships a
//! parser and an expression evaluator, and runs both **inside the request
//! path**. That is the machinery server-side template injection drives: give
//! a template engine a value that reaches its parser and the evaluator will
//! do what the value says, which is the shortest route there is from a form
//! field to remote code execution.
//!
//! [`askama`] compiles a template into a Rust function at build time. At
//! runtime there is no parser, no evaluator, and no template text -- only the
//! `write!` calls the compiler emitted. SSTI is not mitigated here, it is
//! structurally absent, and that is the entire reason this module names
//! askama.
//!
//! The trade, stated plainly: **editing a template requires a rebuild.**
//!
//! # What this module owns
//!
//! * [`View<T>`] -- a template value on its way to becoming a response.
//! * [`view`] -- the constructor a handler calls.
//! * [`ViewError`] -- the one failure a compiled template still has, and its
//!   conversion into [`crate::Error`], which drops the detail into the log
//!   rather than into the body.
//! * A re-export of the certified [`askama`] crate, so an application targets
//!   the version Arcature pins instead of resolving its own.
//!
//! # What it does not own
//!
//! Template syntax, inheritance, filters and the escaper are askama's, and
//! this module deliberately puts nothing in front of them. It is a seam, not
//! a wrapper.
//!
//! # Escaping
//!
//! Askama picks an escaper from the template's extension: `html`, `htm`,
//! `svg`, `xml`, `j2`, `jinja` and `jinja2` get the HTML escaper, everything
//! else gets none. A `{{ value }}` in an `.html` template is therefore
//! escaped unless the template says `{{ value|safe }}`, and a value carrying
//! a `<script>` comes out as text. That is checked by a test in this module
//! rather than taken on trust.
//!
//! # Naming the crate from outside
//!
//! `#[derive(Template)]` writes code that says `askama::`. Inside a crate
//! that depends on askama directly, that resolves. An application depending
//! only on Arcature has to point the derive at the re-export:
//!
//! ```
//! use arcature::view::{Template, view};
//!
//! #[derive(Template)]
//! #[template(
//!     source = "<h1>{{ title }}</h1>",
//!     ext = "html",
//!     askama = arcature::askama
//! )]
//! struct Welcome {
//!     title: String,
//! }
//!
//! let html = view(Welcome { title: "Hello".into() }).render().unwrap();
//! assert_eq!(html, "<h1>Hello</h1>");
//! ```
//!
//! An application that would rather write plain `#[derive(Template)]` can add
//! `askama` to its own `Cargo.toml`; the price is a second version number to
//! keep in step with the framework's.

mod error;

pub use error::ViewError;

// The certified askama, re-exported so downstream code targets the version
// Arcature pins -- the same reason `lettre`, `sea_orm` and `validator` are
// re-exported. `Template` comes along by name because a `use` names every
// namespace at once: the trait and the derive macro share it.
pub use askama;
pub use askama::Template;

/// A template value on its way to becoming an HTTP response.
///
/// `View` is a newtype, not a wrapper: it renders through askama and adds
/// nothing to the template language.
///
/// ```
/// use arcature::view::{Template, View};
///
/// #[derive(Template)]
/// #[template(source = "{{ n }} bottles", ext = "txt", askama = arcature::askama)]
/// struct Song {
///     n: u32,
/// }
///
/// let view = View::new(Song { n: 99 });
/// assert_eq!(view.render().unwrap(), "99 bottles");
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct View<T> {
    template: T,
}

impl<T> View<T> {
    /// Wrap a template value.
    ///
    /// [`view`] is the shorter spelling and reads better in a handler; this
    /// exists because a type with a `new` is easier to name in generic code.
    #[must_use]
    pub fn new(template: T) -> Self {
        Self { template }
    }

    /// Borrow the wrapped template.
    #[must_use]
    pub fn template(&self) -> &T {
        &self.template
    }

    /// Unwrap and return the template value.
    #[must_use]
    pub fn into_template(self) -> T {
        self.template
    }
}

impl<T: Template> View<T> {
    /// Render the template to a `String`.
    ///
    /// # Errors
    ///
    /// [`ViewError::Render`] if a value's `Display` impl fails. There is no
    /// parse error and no unknown-variable error to return: askama resolved
    /// both at build time.
    pub fn render(&self) -> Result<String, ViewError> {
        self.template.render().map_err(ViewError::from)
    }
}

/// Begin a view from a template value.
///
/// ```
/// use arcature::view::{Template, view};
///
/// #[derive(Template)]
/// #[template(
///     source = "<p>Hello, {{ name }}.</p>",
///     ext = "html",
///     askama = arcature::askama
/// )]
/// struct Greeting {
///     name: String,
/// }
///
/// let html = view(Greeting { name: "Ada".into() }).render().unwrap();
/// assert_eq!(html, "<p>Hello, Ada.</p>");
/// ```
#[must_use]
pub fn view<T: Template>(template: T) -> View<T> {
    View::new(template)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fmt;

    use super::Template;

    /// A value whose `Display` impl always fails, so that a compiled
    /// template can be made to fail at render time.
    #[derive(Debug, Default, Clone, Copy)]
    pub(crate) struct Boom;

    impl fmt::Display for Boom {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    /// The template behind every "what happens when rendering fails" test.
    /// Its literal text is distinctive so a test can assert it never reaches
    /// a client.
    #[derive(Template, Debug, Default, Clone, Copy)]
    #[template(source = "<p>secret-template-text {{ boom }}</p>", ext = "html")]
    pub(crate) struct Unformattable {
        pub(crate) boom: Boom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Template)]
    #[template(source = "<p>{{ value }}</p>", ext = "html")]
    struct Html {
        value: &'static str,
    }

    #[derive(Template)]
    #[template(source = "{{ value }}", ext = "txt")]
    struct Text {
        value: &'static str,
    }

    #[test]
    fn a_view_renders_its_template() {
        let rendered = view(Html { value: "hello" }).render().unwrap();
        assert_eq!(rendered, "<p>hello</p>");
    }

    /// The property the whole module rests on: a value is escaped on the way
    /// into an HTML template, so a script tag in the data is text on the page
    /// and not markup.
    #[test]
    fn an_html_template_escapes_its_values() {
        let rendered = view(Html {
            value: "<script>alert(1)</script>",
        })
        .render()
        .unwrap();

        assert!(
            !rendered.contains("<script>"),
            "the script tag survived escaping: {rendered}"
        );
        assert!(
            rendered.contains("&#60;script&#62;") || rendered.contains("&lt;script&gt;"),
            "the script tag was not escaped to an entity: {rendered}"
        );
    }

    /// The other half of the same statement: escaping is chosen by extension,
    /// so a `.txt` template is not quietly HTML-escaping an email body.
    #[test]
    fn a_text_template_does_not_escape() {
        let rendered = view(Text { value: "a < b" }).render().unwrap();
        assert_eq!(rendered, "a < b");
    }

    #[test]
    fn a_failing_value_becomes_a_view_error() {
        let failure = view(test_support::Unformattable::default())
            .render()
            .unwrap_err();
        assert!(matches!(failure, ViewError::Render { .. }));
    }
}
