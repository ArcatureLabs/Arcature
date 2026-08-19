//! The Inertia adapter configuration and root document renderer.

use std::fmt;
use std::sync::Arc;

use super::error::InertiaError;
use super::props::SharedProps;

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
#[derive(Debug, Clone)]
pub struct ScriptBody {
    html: Arc<str>,
}

impl ScriptBody {
    pub(crate) fn from_escaped(html: Arc<str>) -> ScriptBody {
        ScriptBody { html }
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
    version: AssetVersion,
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
                version: AssetVersion::new(version)?,
                root_document: Arc::new(root_document),
                shared_props: SharedProps::new(),
                page_id: "app".to_string(),
            }),
        })
    }

    /// Register shared props.
    pub fn with_shared(mut self, shared: SharedProps) -> Self {
        Arc::make_mut(&mut self.inner).shared_props = shared;
        self
    }

    /// The current asset version.
    pub fn version(&self) -> &AssetVersion {
        &self.inner.version
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

/// A default root document renderer for generated apps. Produces a minimal
/// HTML document with the Inertia script body and the `#app` mount point,
/// plus a `<div id="app">` and Vite client/module scripts.
pub fn default_root_document(title: &str) -> impl RootDocument {
    let title = title.to_string();
    move |body: ScriptBody| {
        format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  \
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n  \
             <title>{title}</title>\n  <link rel=\"stylesheet\" href=\"/css/app.css\" />\n</head>\n\
             <body>\n  {body}\n  <script type=\"module\" src=\"/js/app.tsx\"></script>\n</body>\n</html>"
        )
    }
}
