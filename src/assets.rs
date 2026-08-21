//! Static assets: the Vite manifest, and serving `public/`.
//!
//! # Why a manifest at all
//!
//! A production Vite build writes content-hashed filenames --
//! `assets/app-C7xk91Qa.js`, not `app.js` -- precisely so they can be cached
//! forever. Nothing can know those names ahead of time, which is why Vite also
//! writes `public/build/.vite/manifest.json`, mapping each source entry to
//! what it compiled to. [`Assets`] reads that file **once, at startup** and
//! answers the only question the root document has: what should the page
//! reference for entry `X`.
//!
//! In development there is no manifest and no hashing. Vite serves the source
//! files over the one-port dev proxy, so an entry resolves to its own source
//! path plus the HMR client, and CSS arrives through HMR rather than a
//! `<link>`.
//!
//! # Caching
//!
//! [`StaticFiles`] serves `public/` and picks `Cache-Control` per response: a
//! hashed file under the build prefix is `immutable` for a year, everything
//! else is `no-cache`. The distinction matters -- marking `public/robots.txt`
//! immutable makes it uneditable for a year, and marking a hashed bundle
//! `no-cache` throws away the reason the hash is in the name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The default directory served as the document root.
pub const DEFAULT_PUBLIC_DIR: &str = "public";
/// The default subdirectory of the public directory that Vite builds into,
/// which is also the URL prefix built assets are served under.
pub const DEFAULT_BUILD_DIR: &str = "build";

/// Where built assets live on disk, and under which URL prefix.
///
/// The defaults match what `arc new` scaffolds: Vite's `build.outDir` is
/// `public/build`, so `public/` is the document root and `/build/...` is the
/// URL prefix for hashed output.
#[derive(Debug, Clone)]
pub struct AssetsConfig {
    public_dir: PathBuf,
    build_dir: String,
}

impl AssetsConfig {
    /// The defaults: `public/` as the document root, `build` as Vite's output
    /// subdirectory.
    #[must_use]
    pub fn new() -> Self {
        AssetsConfig {
            public_dir: PathBuf::from(DEFAULT_PUBLIC_DIR),
            build_dir: DEFAULT_BUILD_DIR.to_string(),
        }
    }

    /// Set the document root (default `public`).
    #[must_use]
    pub fn public_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.public_dir = dir.into();
        self
    }

    /// Set Vite's output subdirectory, relative to the document root
    /// (default `build`). This is also the URL prefix.
    #[must_use]
    pub fn build_dir(mut self, dir: impl Into<String>) -> Self {
        self.build_dir = dir.into();
        self
    }

    /// The document root on disk.
    #[must_use]
    pub fn public_path(&self) -> &Path {
        &self.public_dir
    }

    /// Vite's output directory on disk.
    #[must_use]
    pub fn build_path(&self) -> PathBuf {
        self.public_dir.join(&self.build_dir)
    }

    /// The manifest Vite writes for a production build.
    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.build_path().join(".vite").join("manifest.json")
    }

    /// The URL prefix built assets are served under, with a leading slash and
    /// no trailing one (`/build`).
    #[must_use]
    pub fn url_prefix(&self) -> String {
        format!("/{}", self.build_dir.trim_matches('/'))
    }
}

impl Default for AssetsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// A failure to load the Vite manifest.
///
/// Every variant is a startup failure, not a request failure: an application
/// whose manifest is missing or malformed would serve a root document
/// referencing files that do not exist, which is worse than refusing to boot.
#[derive(Debug, thiserror::Error)]
pub enum AssetsError {
    /// The manifest file is not there. Usually means the frontend was never
    /// built.
    #[error(
        "no Vite manifest at {path}: run `arc build` (or `npx vite build`) before starting in production"
    )]
    ManifestMissing {
        /// The path that was looked for.
        path: PathBuf,
    },
    /// The manifest file exists but could not be read.
    #[error("could not read the Vite manifest at {path}: {source}")]
    ManifestUnreadable {
        /// The path that was read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The manifest file was read but is not the JSON shape Vite writes.
    #[error("the Vite manifest at {path} is not valid manifest JSON: {source}")]
    ManifestMalformed {
        /// The path that was parsed.
        path: PathBuf,
        /// The underlying parse error.
        #[source]
        source: serde_json::Error,
    },
}

/// One entry in Vite's `manifest.json`.
///
/// Only the fields that affect what the page references are modelled; the
/// rest of Vite's chunk metadata is ignored rather than rejected, so a newer
/// Vite writing extra fields does not break the parse.
#[derive(Debug, Clone, serde::Deserialize)]
struct Chunk {
    /// The emitted file, relative to the build directory.
    file: String,
    /// Stylesheets this chunk needs, relative to the build directory.
    #[serde(default)]
    css: Vec<String>,
    /// Keys of other chunks this one statically imports. Their CSS is needed
    /// too, which is why the collection below is transitive.
    #[serde(default)]
    imports: Vec<String>,
}

/// What an entry resolves to: one module to load, and the stylesheets it
/// needs loaded first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryAssets {
    /// The URL of the JavaScript module.
    pub js: String,
    /// The URLs of the stylesheets, in dependency order.
    pub css: Vec<String>,
}

/// How an entry name is turned into URLs.
#[derive(Debug, Clone)]
enum Resolver {
    /// Development: Vite serves the source path itself.
    Dev,
    /// Production: names come from the manifest.
    Manifest(BTreeMap<String, Chunk>),
}

#[derive(Debug)]
struct AssetsInner {
    resolver: Resolver,
    /// `/build`, the URL prefix hashed output is served under.
    url_prefix: String,
}

/// The resolved asset map. Cheap to clone; resolution is a lookup, never I/O.
#[derive(Debug, Clone)]
pub struct Assets {
    inner: Arc<AssetsInner>,
}

/// Say, once, that assets will not load.
///
/// Split out of [`Assets::detect`] for one reason: `tracing` is behind the
/// `observe` feature and `assets` is always-on kernel, so the log line cannot
/// simply be written inline. The fallback is `eprintln!` rather than silence
/// because of what this particular message is -- without it, a developer whose
/// frontend is not running sees a blank page with a 200 next to it and no
/// stated cause. Everything else the kernel might want to say can wait for a
/// subscriber; this cannot.
fn warn_no_manifest(path: &Path) {
    #[cfg(feature = "observe")]
    tracing::warn!(
        manifest = %path.display(),
        "no Vite manifest; serving asset URLs as source paths. This is a debug \
         build, so it starts anyway -- but nothing answers those paths until \
         `arc dev` is running or `npx vite build` has been run."
    );
    #[cfg(not(feature = "observe"))]
    eprintln!(
        "warning: no Vite manifest at {}; serving asset URLs as source paths. \
         This is a debug build, so it starts anyway -- but nothing answers \
         those paths until `arc dev` is running or `npx vite build` has been \
         run.",
        path.display()
    );
}

impl Assets {
    /// Development mode: no manifest, entries resolve to their own source
    /// paths for Vite to serve over the dev proxy.
    #[must_use]
    pub fn dev(config: &AssetsConfig) -> Self {
        Assets {
            inner: Arc::new(AssetsInner {
                resolver: Resolver::Dev,
                url_prefix: config.url_prefix(),
            }),
        }
    }

    /// Production mode: read `public/build/.vite/manifest.json`.
    ///
    /// Called once at startup. Every later resolution is a map lookup.
    ///
    /// # Errors
    ///
    /// [`AssetsError`] if the manifest is missing, unreadable, or not the
    /// shape Vite writes.
    pub fn from_manifest(config: &AssetsConfig) -> Result<Self, AssetsError> {
        let path = config.manifest_path();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(AssetsError::ManifestMissing { path });
            }
            Err(source) => return Err(AssetsError::ManifestUnreadable { path, source }),
        };
        let chunks: BTreeMap<String, Chunk> = serde_json::from_str(&raw)
            .map_err(|source| AssetsError::ManifestMalformed { path, source })?;
        Ok(Assets {
            inner: Arc::new(AssetsInner {
                resolver: Resolver::Manifest(chunks),
                url_prefix: config.url_prefix(),
            }),
        })
    }

    /// Pick the mode, in three steps, from strongest signal to weakest.
    ///
    /// 1. `arc dev` set [`VITE_IPC_ENV`](crate::config::VITE_IPC_ENV). Vite is
    ///    live behind the one port and is serving the source tree, so source
    ///    paths are not a guess -- they are the right answer, and a manifest
    ///    sitting next to them from an earlier `arc build` would be stale.
    /// 2. A manifest exists. Somebody ran a frontend build; use what it says,
    ///    whether or not this binary was compiled in debug. Running
    ///    `npx vite build && cargo run` is a normal thing to do when checking
    ///    that production asset resolution works, and it should work.
    /// 3. Neither. Now the build profile decides, on the same reasoning as
    ///    [`ErrorMapping`](crate::http::error_mapping::ErrorMapping) and the
    ///    UAG endpoint: `cfg!(debug_assertions)` is fixed when the binary is
    ///    compiled, so it cannot be flipped by whoever can reach the process
    ///    environment. A debug build falls back to development and logs why;
    ///    a release build refuses to start.
    ///
    /// Step 3 is what makes `arc new demo && cd demo && cargo run` serve a
    /// page. A fresh scaffold has no `node_modules` and no build output, and a
    /// framework whose first command after scaffolding is "now go run a
    /// frontend build" has put a wall in front of the first five minutes.
    /// The page it serves does reference source paths that nothing answers
    /// until Vite is up, so the fallback warns rather than passing silently.
    ///
    /// Deliberately **not** keyed off `APP_ENV`: that is an operator-settable
    /// string, and `AppConfig` documents why nothing with teeth may depend on
    /// one. Here the teeth are the release-mode refusal in step 3 -- serving
    /// source paths from a production binary would 404 every asset on the
    /// page, which is worse than not starting.
    ///
    /// An application that already knows its mode should call
    /// [`dev`](Self::dev) or [`from_manifest`](Self::from_manifest) directly.
    ///
    /// # Errors
    ///
    /// [`AssetsError`] when a release build has no usable manifest.
    pub fn detect(config: &AssetsConfig) -> Result<Self, AssetsError> {
        let ipc = std::env::var(crate::config::VITE_IPC_ENV).is_ok_and(|value| !value.is_empty());
        if ipc {
            return Ok(Self::dev(config));
        }
        match Self::from_manifest(config) {
            Ok(assets) => Ok(assets),
            Err(AssetsError::ManifestMissing { path }) if cfg!(debug_assertions) => {
                warn_no_manifest(&path);
                Ok(Self::dev(config))
            }
            // An unreadable or malformed manifest is a different thing from a
            // missing one: the file is there, so somebody built the frontend,
            // and quietly ignoring it would hide a broken build behind a page
            // that half works. That fails in debug too.
            Err(other) => Err(other),
        }
    }

    /// Whether entries resolve to source paths (development) rather than to
    /// built, hashed files.
    #[must_use]
    pub fn is_dev(&self) -> bool {
        matches!(self.inner.resolver, Resolver::Dev)
    }

    /// Resolve an entry -- the manifest key, which is the entry's path
    /// relative to the project root, e.g. `resources/js/app.tsx`.
    ///
    /// Returns `None` only in production, and only when the manifest has no
    /// such key: in development every path resolves, because Vite transforms
    /// whatever it is handed.
    #[must_use]
    pub fn resolve(&self, entry: &str) -> Option<EntryAssets> {
        match &self.inner.resolver {
            Resolver::Dev => Some(EntryAssets {
                js: format!("/{}", entry.trim_start_matches('/')),
                // Vite injects styles through HMR in development; a `<link>`
                // here would be a second, stale copy.
                css: Vec::new(),
            }),
            Resolver::Manifest(chunks) => {
                let chunk = chunks.get(entry)?;
                Some(EntryAssets {
                    js: self.url(&chunk.file),
                    css: self.collect_css(chunks, entry),
                })
            }
        }
    }

    /// Every stylesheet reachable from `entry`, in dependency order.
    ///
    /// Transitive, because Vite attaches a chunk's CSS to the chunk itself: an
    /// entry that imports a shared chunk needs that chunk's stylesheet too,
    /// and the manifest is the only place that relationship is recorded.
    fn collect_css(&self, chunks: &BTreeMap<String, Chunk>, entry: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut queue = vec![entry.to_string()];
        // A malformed manifest could name a cycle; `seen` bounds the walk.
        while let Some(key) = queue.pop() {
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(chunk) = chunks.get(&key) else {
                continue;
            };
            for css in &chunk.css {
                let url = self.url(css);
                if !out.contains(&url) {
                    out.push(url);
                }
            }
            queue.extend(chunk.imports.iter().cloned());
        }
        out
    }

    /// Prefix a manifest-relative path with the build URL prefix.
    fn url(&self, file: &str) -> String {
        format!("{}/{}", self.inner.url_prefix, file.trim_start_matches('/'))
    }

    /// The `<head>` markup for an entry: its stylesheets, or nothing in
    /// development.
    #[must_use]
    pub fn head_tags(&self, entry: &str) -> String {
        self.head_tags_with_nonce(entry, None)
    }

    /// [`head_tags`](Self::head_tags), with every `<link>` carrying a
    /// Content-Security-Policy nonce.
    ///
    /// A nonce on a `<link rel="stylesheet">` is what satisfies a `style-src
    /// 'nonce-...'` directive: the elements CSP looks for a nonce on are not
    /// only `<script>`s. Pass `None` and the markup is byte-for-byte what
    /// [`head_tags`](Self::head_tags) returns.
    #[must_use]
    pub fn head_tags_with_nonce(&self, entry: &str, nonce: Option<&str>) -> String {
        let Some(resolved) = self.resolve(entry) else {
            return String::new();
        };
        style_tags(&resolved.css, nonce)
    }

    /// The end-of-`<body>` markup for an entry: the module script, preceded in
    /// development by Vite's HMR client.
    ///
    /// Returns an empty string when the entry is not in the manifest -- an
    /// empty page is a clearer failure than a `<script>` pointing at a 404.
    #[must_use]
    pub fn body_tags(&self, entry: &str) -> String {
        self.body_tags_with_nonce(entry, None)
    }

    /// [`body_tags`](Self::body_tags), with every `<script>` carrying a
    /// Content-Security-Policy nonce.
    ///
    /// The nonce is per request -- it comes from
    /// [`CspNonce`](crate::http::security::CspNonce) -- so a root document
    /// that calls this has to call it per request too. That is cheaper than
    /// it sounds and cheaper still if the entry is resolved once at startup;
    /// [`vite_root_document`](crate::inertia::vite_root_document) does the
    /// latter. Pass `None` and the markup is byte-for-byte what
    /// [`body_tags`](Self::body_tags) returns.
    #[must_use]
    pub fn body_tags_with_nonce(&self, entry: &str, nonce: Option<&str>) -> String {
        let Some(resolved) = self.resolve(entry) else {
            return String::new();
        };
        script_tags(Some(&resolved.js), self.is_dev(), nonce)
    }
}

/// The `nonce="..."` attribute with its leading space, or nothing.
fn nonce_attribute(nonce: Option<&str>) -> String {
    // Base64 out of the framework's own RNG: no character in that alphabet
    // needs escaping inside a double-quoted attribute.
    nonce.map(|n| format!(" nonce=\"{n}\"")).unwrap_or_default()
}

/// One `<link rel="stylesheet">` per href, joined the way a `<head>` wants.
///
/// Split out of [`Assets::head_tags_with_nonce`] so a root document can
/// resolve the entry once at startup and still stamp a per-request nonce onto
/// the result.
pub(crate) fn style_tags(css: &[String], nonce: Option<&str>) -> String {
    let attribute = nonce_attribute(nonce);
    css.iter()
        .map(|href| format!("<link{attribute} rel=\"stylesheet\" href=\"{href}\" />"))
        .collect::<Vec<_>>()
        .join("\n  ")
}

/// The end-of-`<body>` scripts for an already-resolved entry.
///
/// `js` is `None` for an entry the manifest does not know, which yields no
/// markup at all -- an empty page is a clearer failure than a `<script>`
/// pointing at a 404, and in that state there is no HMR client worth loading
/// either.
pub(crate) fn script_tags(js: Option<&str>, dev: bool, nonce: Option<&str>) -> String {
    let Some(js) = js else {
        return String::new();
    };
    let attribute = nonce_attribute(nonce);
    let script = format!("<script{attribute} type=\"module\" src=\"{js}\"></script>");
    if dev {
        // `/@vite/client` is what the dev proxy recognises and forwards;
        // without it there is no HMR socket and no style injection. It is a
        // script like any other, so it needs the nonce too.
        format!("<script{attribute} type=\"module\" src=\"/@vite/client\"></script>\n  {script}")
    } else {
        script
    }
}

/// `Cache-Control` for a content-hashed file: the name changes when the bytes
/// change, so the old name is safe to keep forever.
pub const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
/// `Cache-Control` for everything else under the document root. `no-cache`
/// permits caching but forces revalidation, so an edited `robots.txt` is
/// picked up without giving up conditional requests.
pub const REVALIDATE_CACHE_CONTROL: &str = "no-cache";

/// Choose the `Cache-Control` value for a served path.
///
/// `immutable_prefix` is the build URL prefix *with* a trailing slash.
fn cache_control_for(path: &str, immutable_prefix: &str) -> &'static str {
    if path.starts_with(immutable_prefix) && looks_hashed(path) {
        IMMUTABLE_CACHE_CONTROL
    } else {
        REVALIDATE_CACHE_CONTROL
    }
}

/// Whether a path's file name carries a content hash, as Vite emits them:
/// `<name>-<hash>.<ext>`.
///
/// Deliberately conservative. A hash containing `-` makes the trailing segment
/// look too short and the file is treated as mutable -- a cache miss, not a
/// stale asset. Being wrong the other way would pin a changing file for a year.
fn looks_hashed(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or_default();
    let Some((stem, _extension)) = file.rsplit_once('.') else {
        return false;
    };
    let Some((_name, hash)) = stem.rsplit_once('-') else {
        return false;
    };
    hash.len() >= 8 && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The document-root file server: `ServeDir` over `public/`, with
/// `Cache-Control` chosen per path.
///
/// Installed as the router's fallback by
/// [`ApplicationBuilder::static_files`](crate::application::ApplicationBuilder::static_files),
/// so it only ever sees requests no route matched.
#[derive(Clone, Debug)]
pub struct StaticFiles {
    inner: tower_http::services::ServeDir,
    immutable_prefix: Arc<str>,
}

impl StaticFiles {
    /// Serve the configured document root.
    #[must_use]
    pub fn new(config: &AssetsConfig) -> Self {
        StaticFiles {
            inner: tower_http::services::ServeDir::new(config.public_path())
                // An Inertia app renders its own root document from Rust;
                // handing out `public/index.html` for a directory would
                // shadow that with a stale copy.
                .append_index_html_on_directories(false),
            immutable_prefix: Arc::from(format!("{}/", config.url_prefix())),
        }
    }
}

impl tower::Service<axum::extract::Request> for StaticFiles {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        <tower_http::services::ServeDir as tower::Service<axum::extract::Request>>::poll_ready(
            &mut self.inner,
            cx,
        )
    }

    fn call(&mut self, request: axum::extract::Request) -> Self::Future {
        use axum::response::IntoResponse as _;

        // Decided from the request path, which is why this is a service and
        // not a `SetResponseHeaderLayer`: that only ever sees the response.
        let cache_control = cache_control_for(request.uri().path(), &self.immutable_prefix);
        let future = tower::Service::call(&mut self.inner, request);
        Box::pin(async move {
            let mut response = match future.await {
                Ok(response) => response.into_response(),
                Err(never) => match never {},
            };
            // Only on a hit. Caching a 404 for a year would make a mistyped
            // asset name permanent.
            if response.status().is_success() {
                response.headers_mut().insert(
                    axum::http::header::CACHE_CONTROL,
                    axum::http::HeaderValue::from_static(cache_control),
                );
            }
            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: &str) -> Assets {
        Assets {
            inner: Arc::new(AssetsInner {
                resolver: Resolver::Manifest(serde_json::from_str(json).expect("manifest")),
                url_prefix: "/build".to_string(),
            }),
        }
    }

    #[test]
    fn the_default_config_matches_the_scaffold_layout() {
        let config = AssetsConfig::new();
        assert_eq!(config.public_path(), Path::new("public"));
        assert_eq!(config.build_path(), Path::new("public").join("build"));
        assert_eq!(config.url_prefix(), "/build");
        assert!(
            config
                .manifest_path()
                .ends_with(Path::new("build").join(".vite").join("manifest.json"))
        );
    }

    #[test]
    fn an_entry_resolves_to_its_hashed_file() {
        let assets = manifest(
            r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js","isEntry":true}}"#,
        );
        let resolved = assets.resolve("resources/js/app.tsx").expect("entry");
        assert_eq!(resolved.js, "/build/assets/app-C7xk91Qa.js");
        assert!(resolved.css.is_empty());
    }

    #[test]
    fn css_is_collected_through_imports() {
        // The entry has no CSS of its own; the shared chunk it imports does.
        // Missing that is how a production build loses half its stylesheet.
        let assets = manifest(
            r#"{
              "resources/js/app.tsx": {
                "file": "assets/app-A1b2C3d4.js",
                "imports": ["_shared-E5f6G7h8.js"],
                "css": ["assets/app-I9j0K1l2.css"]
              },
              "_shared-E5f6G7h8.js": {
                "file": "assets/shared-E5f6G7h8.js",
                "css": ["assets/shared-M3n4O5p6.css"]
              }
            }"#,
        );
        let resolved = assets.resolve("resources/js/app.tsx").expect("entry");
        assert_eq!(
            resolved.css,
            [
                "/build/assets/app-I9j0K1l2.css",
                "/build/assets/shared-M3n4O5p6.css"
            ]
        );
    }

    #[test]
    fn a_cyclic_manifest_does_not_hang() {
        let assets = manifest(
            r#"{
              "a.js": {"file": "assets/a-A1b2C3d4.js", "imports": ["b.js"], "css": ["assets/a-Q7r8S9t0.css"]},
              "b.js": {"file": "assets/b-U1v2W3x4.js", "imports": ["a.js"]}
            }"#,
        );
        let resolved = assets.resolve("a.js").expect("entry");
        assert_eq!(resolved.css, ["/build/assets/a-Q7r8S9t0.css"]);
    }

    #[test]
    fn an_unknown_entry_does_not_resolve_in_production() {
        let assets = manifest(r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js"}}"#);
        assert!(assets.resolve("resources/js/missing.tsx").is_none());
        assert_eq!(assets.body_tags("resources/js/missing.tsx"), "");
    }

    #[test]
    fn dev_resolves_the_source_path_and_adds_the_hmr_client() {
        let assets = Assets::dev(&AssetsConfig::new());
        let resolved = assets.resolve("resources/js/app.tsx").expect("entry");
        assert_eq!(resolved.js, "/resources/js/app.tsx");
        assert!(resolved.css.is_empty(), "Vite injects styles over HMR");
        assert!(assets.head_tags("resources/js/app.tsx").is_empty());
        assert!(
            assets
                .body_tags("resources/js/app.tsx")
                .contains("/@vite/client")
        );
    }

    #[test]
    fn production_tags_link_the_stylesheet_and_load_the_module() {
        let assets = manifest(
            r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js","css":["assets/app-Z9y8X7w6.css"]}}"#,
        );
        assert_eq!(
            assets.head_tags("resources/js/app.tsx"),
            "<link rel=\"stylesheet\" href=\"/build/assets/app-Z9y8X7w6.css\" />"
        );
        let body = assets.body_tags("resources/js/app.tsx");
        assert_eq!(
            body,
            "<script type=\"module\" src=\"/build/assets/app-C7xk91Qa.js\"></script>"
        );
        assert!(!body.contains("@vite/client"), "not a dev build");
    }

    #[test]
    fn the_nonce_lands_on_every_tag_a_manifest_entry_produces() {
        // A `script-src 'nonce-X'` policy blocks a bundle whose tag does not
        // carry X, which is a blank page rather than a hardened one.
        let assets = manifest(
            r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js","css":["assets/app-Z9y8X7w6.css"]}}"#,
        );
        assert_eq!(
            assets.head_tags_with_nonce("resources/js/app.tsx", Some("r4nd0m")),
            "<link nonce=\"r4nd0m\" rel=\"stylesheet\" href=\"/build/assets/app-Z9y8X7w6.css\" />"
        );
        assert_eq!(
            assets.body_tags_with_nonce("resources/js/app.tsx", Some("r4nd0m")),
            "<script nonce=\"r4nd0m\" type=\"module\" src=\"/build/assets/app-C7xk91Qa.js\"></script>"
        );
    }

    #[test]
    fn the_hmr_client_carries_the_nonce_too() {
        // It is a script like any other; missing it costs the dev server its
        // HMR socket under a policy that is supposed to match production.
        let assets = Assets::dev(&AssetsConfig::new());
        let body = assets.body_tags_with_nonce("resources/js/app.tsx", Some("r4nd0m"));
        assert_eq!(body.matches("nonce=\"r4nd0m\"").count(), 2, "{body}");
    }

    #[test]
    fn tags_without_a_nonce_are_byte_for_byte_what_they_always_were() {
        let assets = manifest(
            r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js","css":["assets/app-Z9y8X7w6.css"]}}"#,
        );
        assert_eq!(
            assets.head_tags("resources/js/app.tsx"),
            assets.head_tags_with_nonce("resources/js/app.tsx", None)
        );
        assert!(!assets.body_tags("resources/js/app.tsx").contains("nonce"));
    }

    #[test]
    fn a_missing_manifest_is_a_startup_error_not_a_silent_dev_fallback() {
        let config = AssetsConfig::new().public_dir("this-directory-does-not-exist");
        let error = Assets::from_manifest(&config).expect_err("no manifest");
        assert!(matches!(error, AssetsError::ManifestMissing { .. }));
    }

    // The three `detect` tests below deliberately do not touch
    // `ARCATURE_VITE_IPC`. The crate is `unsafe_code = "forbid"` and
    // `std::env::set_var` is unsafe in edition 2024, so the first branch of
    // `detect` is not reachable from a unit test at all -- and even if it
    // were, mutating process environment from one of several test threads is
    // a race. What is testable is the part that decides the outcome when
    // nobody set the variable, which is also the part that regressed: the
    // scaffolded application refused to boot.

    #[test]
    fn detect_falls_back_to_dev_when_a_debug_build_has_no_manifest() {
        let config = AssetsConfig::new().public_dir("this-directory-does-not-exist");
        let assets = Assets::detect(&config).expect("a debug build starts without a manifest");
        assert!(assets.is_dev());
    }

    #[test]
    fn detect_prefers_a_manifest_that_exists_over_the_debug_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vite = dir.path().join("build").join(".vite");
        std::fs::create_dir_all(&vite).expect("mkdir");
        std::fs::write(
            vite.join("manifest.json"),
            r#"{"resources/js/app.tsx":{"file":"assets/app-C7xk91Qa.js"}}"#,
        )
        .expect("write manifest");

        let config = AssetsConfig::new().public_dir(dir.path());
        let assets = Assets::detect(&config).expect("the manifest is readable");
        assert!(!assets.is_dev(), "a present manifest wins over the profile");
        assert_eq!(
            assets.resolve("resources/js/app.tsx").expect("entry").js,
            "/build/assets/app-C7xk91Qa.js"
        );
    }

    #[test]
    fn detect_refuses_a_manifest_it_cannot_parse_even_in_a_debug_build() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vite = dir.path().join("build").join(".vite");
        std::fs::create_dir_all(&vite).expect("mkdir");
        std::fs::write(vite.join("manifest.json"), "{ not json").expect("write manifest");

        let config = AssetsConfig::new().public_dir(dir.path());
        let error = Assets::detect(&config).expect_err("a broken build is not a dev build");
        assert!(matches!(error, AssetsError::ManifestMalformed { .. }));
    }

    #[test]
    fn hashed_files_under_the_build_prefix_are_immutable() {
        assert_eq!(
            cache_control_for("/build/assets/app-C7xk91Qa.js", "/build/"),
            IMMUTABLE_CACHE_CONTROL
        );
    }

    #[test]
    fn unhashed_files_revalidate_even_under_the_build_prefix() {
        // `manifest.json` lives under `/build/` and is rewritten every build.
        assert_eq!(
            cache_control_for("/build/.vite/manifest.json", "/build/"),
            REVALIDATE_CACHE_CONTROL
        );
    }

    #[test]
    fn files_outside_the_build_prefix_revalidate_however_they_are_named() {
        // A hash-looking name the app author chose is not a Vite guarantee.
        assert_eq!(
            cache_control_for("/robots.txt", "/build/"),
            REVALIDATE_CACHE_CONTROL
        );
        assert_eq!(
            cache_control_for("/images/logo-C7xk91Qa.png", "/build/"),
            REVALIDATE_CACHE_CONTROL
        );
    }

    #[test]
    fn short_suffixes_are_not_mistaken_for_hashes() {
        assert!(!looks_hashed("/build/assets/app-v2.js"));
        assert!(!looks_hashed("/build/assets/app.js"));
        assert!(!looks_hashed("/build/assets/appC7xk91Qa"));
        assert!(looks_hashed("/build/assets/app-C7xk91Qa.js"));
    }
}
