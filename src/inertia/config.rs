//! The Inertia adapter configuration and root document renderer.

use std::fmt;
use std::sync::Arc;

use super::error::InertiaError;
use super::head::Head;
use super::props::SharedProps;
use crate::http::security::CspNonce;

/// The current asset version, compared against `X-Inertia-Version`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetVersion(Arc<str>);

impl AssetVersion {
    /// Create a validated asset version (must be header-encodable).
    pub fn new(version: impl AsRef<str>) -> Result<Self, InertiaError> {
        let version = version.as_ref();
        axum::http::HeaderValue::from_str(version)
            .map_err(axum::http::Error::from)
            .map_err(InertiaError::Header)?;
        Ok(AssetVersion(Arc::from(version)))
    }

    /// The version as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for AssetVersion {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// The safely-embeddable Inertia page payload.
///
/// Contains the `<script data-page="..." type="application/json">ESCAPED</script>`
/// element followed by the `<div id="...">` mount point — already escaped for
/// safe embedding. Constructed only by the adapter; applications receive it
/// in their root document renderer and embed it via `Display`.
///
/// It also carries this request's [`CspNonce`], when there is one, and the
/// [`Head`] this page asked for. That is why the type is a struct rather than
/// a `String`: each of those could be added to it without touching
/// [`RootDocument::render`], and every application that passes a plain
/// `Fn(ScriptBody) -> String` closure kept compiling. Whatever a root document
/// needs next arrives the same way.
#[derive(Debug, Clone)]
pub struct ScriptBody {
    html: Arc<str>,
    nonce: Option<CspNonce>,
    head: Option<Head>,
}

impl ScriptBody {
    pub(crate) fn from_escaped(
        html: Arc<str>,
        nonce: Option<CspNonce>,
        head: Option<Head>,
    ) -> ScriptBody {
        ScriptBody { html, nonce, head }
    }

    /// The [`Head`] this page asked for: its title, meta description,
    /// canonical URL and Open Graph and Twitter card fields, every value
    /// already HTML-escaped.
    ///
    /// `None` when the handler set none, which is the case for every
    /// application written before this existed. A root document that wants
    /// server-rendered metadata reads it here and falls back to whatever it
    /// used before; the stock [`default_root_document`] and
    /// [`vite_root_document`] do exactly that with their own `title`.
    ///
    /// This is the entire server-side SEO story and it is deliberately small.
    /// Google runs JavaScript, so a client-rendered title reaches it
    /// eventually; Facebook, Zalo, Slack, Discord, LinkedIn, Telegram and X
    /// do not run any, so whatever is not in these bytes does not exist to
    /// them. Rendering the application itself on the server would mean a
    /// JavaScript runtime in the request path, which buys nothing a scraper
    /// reads.
    ///
    /// ```
    /// use arcature::axum::body::{Body, to_bytes};
    /// use arcature::axum::http::Request;
    /// use arcature::axum::routing::get;
    /// use arcature::axum::Router;
    /// use arcature::inertia::{Head, Inertia, InertiaConfig, InertiaLayer, ScriptBody};
    /// use tower::ServiceExt as _;
    ///
    /// # #[tokio::main]
    /// # async fn main() {
    /// let config = InertiaConfig::versionless(|body: ScriptBody| {
    ///     let title = body.head().and_then(Head::title).unwrap_or("Acme");
    ///     format!(
    ///         "<!doctype html><html><head><title>{title}</title></head>\
    ///          <body>{body}</body></html>"
    ///     )
    /// });
    ///
    /// let app = Router::new()
    ///     .route(
    ///         "/",
    ///         get(|inertia: Inertia| async move {
    ///             inertia
    ///                 .render("Home", arcature::serde_json::json!({}))
    ///                 .await
    ///                 .unwrap()
    ///         }),
    ///     )
    ///     .layer(InertiaLayer::new(config));
    ///
    /// let response = app
    ///     .oneshot(Request::get("/").body(Body::empty()).unwrap())
    ///     .await
    ///     .unwrap();
    /// let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    /// let html = String::from_utf8(bytes.to_vec()).unwrap();
    ///
    /// // Nothing set a head on this render, so the document's own title
    /// // stands -- exactly what it did before heads existed.
    /// assert!(html.contains("<title>Acme</title>"), "{html}");
    /// # }
    /// ```
    #[must_use]
    pub fn head(&self) -> Option<&Head> {
        self.head.as_ref()
    }

    /// This request's Content-Security-Policy nonce, if the application
    /// installed [`SecurityHeaders::with_csp_nonce`].
    ///
    /// The payload script this body contains already carries it. This is for
    /// the *other* elements a hand-written root document writes — its own
    /// `<script>` and `<style>` tags, an analytics snippet — which the
    /// framework cannot stamp because it never sees them.
    ///
    /// [`SecurityHeaders::with_csp_nonce`]: crate::http::security::SecurityHeaders::with_csp_nonce
    #[must_use]
    pub fn nonce(&self) -> Option<&CspNonce> {
        self.nonce.as_ref()
    }

    /// The nonce as an HTML attribute with a leading space, or the empty
    /// string when there is none.
    ///
    /// Written to be interpolated straight into a tag, which is what makes a
    /// nonce-aware root document readable:
    ///
    /// ```
    /// use arcature::inertia::{RootDocument, ScriptBody};
    ///
    /// // A root document is any `Fn(ScriptBody) -> String`, so this function
    /// // is one: the framework builds the body and hands it over, nonce and
    /// // all, and the document decides what surrounds it.
    /// fn document(body: ScriptBody) -> String {
    ///     let nonce = body.nonce_attribute();
    ///     format!("<body>{body}<script{nonce} src=\"/js/app.js\"></script></body>")
    /// }
    ///
    /// # fn takes_root_document(_: impl RootDocument) {}
    /// # takes_root_document(document as fn(ScriptBody) -> String);
    /// ```
    #[must_use]
    pub fn nonce_attribute(&self) -> String {
        self.nonce
            .as_ref()
            .map(CspNonce::attribute)
            .unwrap_or_default()
    }
}

impl fmt::Display for ScriptBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.html)
    }
}

/// The application-supplied root document renderer.
///
/// Implement this for your own type, or pass any `Fn(ScriptBody) -> String`
/// closure/function (a blanket impl covers closures).
pub trait RootDocument: Send + Sync {
    /// Produce the full HTML document, embedding `body`.
    fn render(&self, body: ScriptBody) -> String;
}

impl<T> RootDocument for T
where
    T: Fn(ScriptBody) -> String + Send + Sync,
{
    fn render(&self, body: ScriptBody) -> String {
        self(body)
    }
}

/// The configuration for an Inertia adapter.
///
/// Built once at startup and shared (cheaply cloned) across requests.
#[derive(Clone)]
pub struct InertiaConfig {
    inner: Arc<ConfigInner>,
}

impl fmt::Debug for InertiaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InertiaConfig")
            .field("version", &self.inner.version)
            .field("shared_props", &self.inner.shared_props.is_empty())
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
struct ConfigInner {
    version: Option<AssetVersion>,
    root_document: Arc<dyn RootDocument>,
    shared_props: SharedProps,
    page_id: String,
}

impl InertiaConfig {
    /// Create a configuration with an asset version and a root document renderer.
    pub fn new(
        version: impl AsRef<str>,
        root_document: impl RootDocument + 'static,
    ) -> Result<InertiaConfig, InertiaError> {
        Ok(InertiaConfig {
            inner: Arc::new(ConfigInner {
                version: Some(AssetVersion::new(version)?),
                root_document: Arc::new(root_document),
                shared_props: SharedProps::new(),
                page_id: "app".to_string(),
            }),
        })
    }

    /// Create a configuration with no asset version.
    ///
    /// The client types `version` as `string | null` and sends back whatever
    /// it was given, so an application with no build step has a coherent
    /// answer: `null` in the page object, no `X-Inertia-Version` to compare,
    /// and therefore no version mismatch to force a hard reload. There is
    /// nothing to invalidate when nothing is hashed.
    ///
    /// Every application that does build assets wants
    /// [`new`](Self::new) instead -- without a version the client keeps
    /// running last deploy's JavaScript against this deploy's props until
    /// something else makes it reload.
    pub fn versionless(root_document: impl RootDocument + 'static) -> InertiaConfig {
        InertiaConfig {
            inner: Arc::new(ConfigInner {
                version: None,
                root_document: Arc::new(root_document),
                shared_props: SharedProps::new(),
                page_id: "app".to_string(),
            }),
        }
    }

    /// Register shared props.
    pub fn with_shared(mut self, shared: SharedProps) -> Self {
        Arc::make_mut(&mut self.inner).shared_props = shared;
        self
    }

    /// Use a different id for the page element and its mount point.
    ///
    /// `app` is the default on both sides. Change it only alongside the
    /// client's `createInertiaApp({ id })` -- the two names are one
    /// agreement, and a server that renames alone renders a page the client
    /// cannot find.
    pub fn with_page_id(mut self, id: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).page_id = id.into();
        self
    }

    /// The current asset version, if the application has one.
    pub fn version(&self) -> Option<&AssetVersion> {
        self.inner.version.as_ref()
    }

    /// The asset version as the protocol compares it: the empty string when
    /// there is none, which is exactly what a client with no version sends.
    pub(crate) fn version_str(&self) -> &str {
        self.inner.version.as_ref().map_or("", AssetVersion::as_str)
    }

    pub(crate) fn root_document(&self) -> &Arc<dyn RootDocument> {
        &self.inner.root_document
    }

    pub(crate) fn shared_props(&self) -> &SharedProps {
        &self.inner.shared_props
    }

    pub(crate) fn page_id(&self) -> &str {
        &self.inner.page_id
    }
}

/// A minimal root document that references fixed asset paths.
///
/// Kept for applications with no build step, where `public/css/app.css` and
/// `public/js/app.js` are files that genuinely exist. A Vite application
/// wants [`vite_root_document`] instead: a production build emits hashed
/// names that nothing can spell in advance.
// `use<>`: the returned document owns a copy of everything it needs, so it
// must not capture the argument lifetime. Without the bound, Rust 2024
// captures `'_` and a caller cannot build a config that outlives its title.
pub fn default_root_document(title: &str) -> impl RootDocument + use<> {
    let title = title.to_string();
    move |body: ScriptBody| {
        // Both the stylesheet link and the module script carry the request's
        // nonce when there is one, and nothing when there is not. A
        // `script-src 'nonce-X'` policy that this document did not satisfy
        // would render the page blank rather than merely unstyled.
        let nonce = body.nonce_attribute();
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  \
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  \
             <title>{title}</title>\n  <link{nonce} rel=\"stylesheet\" href=\"/css/app.css\" />\n</head>\n\
             <body>\n  {body}\n  <script{nonce} type=\"module\" src=\"/js/app.js\"></script>\n</body>\n</html>"
        )
    }
}

/// A root document that gets its asset URLs from [`Assets`].
///
/// `entry` is the manifest key -- the entry's path relative to the project
/// root, `resources/js/app.tsx` in a scaffolded app. In development that
/// resolves to the source path plus Vite's HMR client; in production it
/// resolves through `manifest.json` to the hashed build output, which is the
/// only way the reference can still be correct after a rebuild.
///
/// The entry is resolved **once**, here, not per request: [`Assets`] is
/// already the loaded manifest, and the answer cannot change while the
/// process runs. Only the tags are re-formatted per request, and only because
/// they carry that request's Content-Security-Policy nonce -- the URLs inside
/// them were settled at startup.
///
/// ```no_run
/// use arcature::assets::{Assets, AssetsConfig};
/// use arcature::inertia::{InertiaConfig, vite_root_document};
///
/// let assets = Assets::detect(&AssetsConfig::new())?;
/// let config = InertiaConfig::new(
///     env!("CARGO_PKG_VERSION"),
///     vite_root_document("Acme", &assets, "resources/js/app.tsx"),
/// )?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// [`Assets`]: crate::assets::Assets
pub fn vite_root_document(
    title: &str,
    assets: &crate::assets::Assets,
    entry: &str,
) -> impl RootDocument + use<> {
    let title = title.to_string();
    let resolved = assets.resolve(entry);
    let dev = assets.is_dev();
    move |body: ScriptBody| {
        let nonce = body.nonce().map(CspNonce::as_str);
        let head = crate::assets::style_tags(
            resolved.as_ref().map(|r| r.css.as_slice()).unwrap_or(&[]),
            nonce,
        );
        let scripts =
            crate::assets::script_tags(resolved.as_ref().map(|r| r.js.as_str()), dev, nonce);
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  \
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  \
             <title>{title}</title>\n  {head}\n</head>\n\
             <body>\n  {body}\n  {scripts}\n</body>\n</html>"
        )
    }
}
