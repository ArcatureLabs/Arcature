//! Turning a rendered view into an HTTP response.
//!
//! # The failure path is the interesting half
//!
//! Rendering succeeds almost always, because askama resolved the template at
//! build time. When it does not -- a value whose `Display` returned `Err` --
//! the response must say nothing about why.
//!
//! It is worth being precise about what "nothing" excludes, because the
//! obvious implementation leaks all three: the template's own text (which is
//! application source, and on an error page is often the half a developer was
//! mid-edit), the template's path (a map of the source tree, and of the
//! filesystem the process runs on), and the value that would not format
//! (whatever the failing `Display` had already written before it gave up --
//! plausibly a session token or a row from the database).
//!
//! So the response is a plain `500` with the framework's ordinary internal
//! error body, and the askama message goes to `tracing` where the operator
//! can read it. That is the same division `src/http/error_mapping.rs` makes
//! for a `text/plain` 5xx and the same one `src/error.rs` makes in
//! production; this module simply does not offer the development-mode
//! version, because there is no build in which a template's contents are a
//! reasonable thing to send to a browser.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::{Template, View};

impl<T> View<T> {
    /// Send the view under a status other than `200 OK`.
    ///
    /// ```
    /// use arcature::prelude::*;
    /// use arcature::view::{Template, view};
    ///
    /// #[derive(Template)]
    /// #[template(
    ///     source = "<h1>{{ path }} is not here</h1>",
    ///     ext = "html",
    ///     askama = arcature::askama
    /// )]
    /// struct NotFound {
    ///     path: String,
    /// }
    ///
    /// let response = view(NotFound { path: "/nope".into() })
    ///     .status(StatusCode::NOT_FOUND)
    ///     .into_response();
    ///
    /// assert_eq!(response.status(), StatusCode::NOT_FOUND);
    /// ```
    #[must_use]
    pub fn status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Send the view under a content type other than
    /// `text/html; charset=utf-8`.
    ///
    /// The default is HTML rather than something inferred from the template,
    /// because askama 0.16 does not carry the extension into the compiled
    /// type -- `Template` has no `MIME_TYPE` to read. A view rendering an
    /// `.xml` or `.txt` template therefore has to say so here.
    ///
    /// ```
    /// use arcature::axum::http::HeaderValue;
    /// use arcature::prelude::*;
    /// use arcature::view::{Template, view};
    ///
    /// #[derive(Template)]
    /// #[template(
    ///     source = "User-agent: *\nDisallow: {{ path }}\n",
    ///     ext = "txt",
    ///     askama = arcature::askama
    /// )]
    /// struct Robots {
    ///     path: String,
    /// }
    ///
    /// let response = view(Robots { path: "/admin".into() })
    ///     .content_type(HeaderValue::from_static("text/plain; charset=utf-8"))
    ///     .into_response();
    ///
    /// assert_eq!(
    ///     response.headers()["content-type"],
    ///     "text/plain; charset=utf-8"
    /// );
    /// ```
    #[must_use]
    pub fn content_type(mut self, content_type: HeaderValue) -> Self {
        self.content_type = content_type;
        self
    }

    /// Declare the language this view was rendered in, sending
    /// `Content-Language`.
    ///
    /// The framework does not infer this. A compiled template carries no
    /// language -- askama resolved it to `write!` calls -- and the locale
    /// [`LocaleLayer`](crate::i18n::LocaleLayer) negotiated is what the
    /// request *asked* for, which is not the same claim as what the bytes in
    /// this response are actually in. A handler that renders a French
    /// template says so here; one that renders a template it did not
    /// translate says nothing, which is better than an untrue header.
    ///
    /// Translation itself stays in the template, where askama already has
    /// it: give the template struct a [`Locale`](crate::i18n::Locale) field
    /// and call it. There is no filter and no `{{ t("key") }}` syntax here,
    /// because adding one would mean a lookup the compiler cannot check --
    /// the opposite of the reason this module exists.
    ///
    /// ```
    /// use arcature::i18n::{Catalog, Catalogs, LocaleId, LocaleNegotiator};
    /// use arcature::prelude::*;
    /// use arcature::view::{Template, view};
    ///
    /// #[derive(Template)]
    /// #[template(
    ///     source = "<h1>{{ locale.message(\"hi\").unwrap_or_default() }}</h1>",
    ///     ext = "html",
    ///     askama = arcature::askama
    /// )]
    /// struct Greeting {
    ///     locale: arcature::i18n::Locale,
    /// }
    ///
    /// let catalogs = Catalogs::new(
    ///     Catalog::parse(LocaleId::parse("fr").unwrap(), "hi = Bonjour").unwrap(),
    /// );
    /// let locale = LocaleNegotiator::new(catalogs).fallback();
    ///
    /// let response = view(Greeting { locale: locale.clone() })
    ///     .in_locale(&locale)
    ///     .into_response();
    ///
    /// assert_eq!(response.headers()["content-language"], "fr");
    /// ```
    #[cfg(feature = "i18n")]
    #[must_use]
    pub fn in_locale(mut self, locale: &crate::i18n::Locale) -> Self {
        // A `LocaleId` is a canonical BCP-47 tag -- ASCII alphanumerics and
        // dashes -- so this never fails. The fallible constructor is used
        // anyway rather than asserting that from a distance.
        self.content_language = HeaderValue::from_str(locale.id().as_str()).ok();
        self
    }
}

/// Attach the `Content-Language` a handler declared, if it declared one.
///
/// A free function rather than three lines inline, so the non-`i18n` build
/// does not need a `mut` binding it never mutates.
#[cfg(feature = "i18n")]
fn with_content_language(mut response: Response, language: Option<HeaderValue>) -> Response {
    if let Some(language) = language {
        response
            .headers_mut()
            .insert(header::CONTENT_LANGUAGE, language);
    }
    response
}

/// Render the view, or answer a failure with a `500` that says nothing.
///
/// ```
/// use arcature::prelude::*;
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
/// async fn greet() -> Result<Response> {
///     Ok(view(Greeting { name: "Ada".into() }).into_response())
/// }
/// ```
impl<T: Template> IntoResponse for View<T> {
    fn into_response(self) -> Response {
        #[cfg(feature = "i18n")]
        let content_language = self.content_language.clone();
        match self.template.render() {
            Ok(body) => {
                let response = (
                    self.status,
                    [(header::CONTENT_TYPE, self.content_type)],
                    Body::from(body),
                )
                    .into_response();
                #[cfg(feature = "i18n")]
                let response = with_content_language(response, content_language);
                response
            }
            // `From<ViewError> for Error` is where the detail is logged and
            // dropped; going through it keeps that decision in one place
            // instead of two that can drift apart.
            Err(error) => crate::Error::from(super::ViewError::from(error)).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::test_support::Unformattable;
    use crate::view::view;

    #[derive(Template)]
    #[template(source = "<p>{{ value }}</p>", ext = "html")]
    struct Page {
        value: &'static str,
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is readable");
        String::from_utf8(bytes.to_vec()).expect("the body is UTF-8")
    }

    #[tokio::test]
    async fn a_view_answers_200_with_html() {
        let response = view(Page { value: "hello" }).into_response();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(body_of(response).await, "<p>hello</p>");
    }

    #[tokio::test]
    async fn the_status_and_content_type_are_overridable() {
        let response = view(Page { value: "gone" })
            .status(StatusCode::GONE)
            .content_type(HeaderValue::from_static("application/xhtml+xml"))
            .into_response();

        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/xhtml+xml"
        );
    }

    /// The additive half: a view that says nothing about its language sends
    /// no `Content-Language`, in every build.
    #[tokio::test]
    async fn a_view_declares_no_language_by_default() {
        let response = view(Page { value: "hello" }).into_response();
        assert!(!response.headers().contains_key(header::CONTENT_LANGUAGE));
    }

    #[cfg(feature = "i18n")]
    #[tokio::test]
    async fn a_view_that_declares_a_locale_says_so_in_the_headers() {
        use crate::i18n::{Catalog, Catalogs, LocaleId, LocaleNegotiator};

        let catalogs =
            Catalogs::new(Catalog::parse(LocaleId::parse("pt-BR").unwrap(), "hi = Ola").unwrap());
        let locale = LocaleNegotiator::new(catalogs).fallback();

        let response = view(Page { value: "ola" })
            .in_locale(&locale)
            .into_response();

        assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "pt-BR");
        // And it did not disturb what the view already sent.
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    /// A render failure is a `500` that says nothing, and a declared language
    /// does not survive onto it: the body is the framework's error document,
    /// not the page that failed.
    #[cfg(feature = "i18n")]
    #[tokio::test]
    async fn a_failed_render_does_not_claim_a_language() {
        use crate::i18n::{Catalog, Catalogs, LocaleId, LocaleNegotiator};

        let catalogs =
            Catalogs::new(Catalog::parse(LocaleId::parse("fr").unwrap(), "hi = Salut").unwrap());
        let locale = LocaleNegotiator::new(catalogs).fallback();

        let response = view(Unformattable::default())
            .in_locale(&locale)
            .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!response.headers().contains_key(header::CONTENT_LANGUAGE));
    }

    /// The point of the whole module: a render failure tells the client
    /// nothing. Not the template's text, not its path, not the value that
    /// would not format, and not the name of the engine.
    #[tokio::test]
    async fn a_render_failure_leaks_nothing() {
        let response = view(Unformattable::default()).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = body_of(response).await;
        assert!(
            !body.contains("secret-template-text"),
            "the template's own text reached the client: {body}"
        );
        assert!(
            !body.to_ascii_lowercase().contains("template"),
            "the word `template` reached the client: {body}"
        );
        assert!(
            !body.to_ascii_lowercase().contains("askama"),
            "the engine named itself to the client: {body}"
        );
        assert!(
            !body.contains("view.rs") && !body.contains("src\\view") && !body.contains("src/view"),
            "a source path reached the client: {body}"
        );
    }
}
