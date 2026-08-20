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

    /// Pick the mode from the environment: development when `arc dev` set
    /// [`VITE_IPC_ENV`](crate::config::VITE_IPC_ENV), production otherwise.
    ///
    /// This reads the environment once, at startup, like every other resolved
    /// configuration in the framework. An application that already knows which
    /// mode it is in should call [`dev`](Self::dev) or
    /// [`from_manifest`](Self::from_manifest) directly.
    ///
    /// # Errors
    ///
    /// [`AssetsError`] when production mode cannot load the manifest. There is
    /// deliberately no fallback to development mode: silently serving source
    /// paths in production would 404 every asset on the page.
    pub fn detect(config: &AssetsConfig) -> Result<Self, AssetsError> {
        let dev = std::env::var(crate::config::VITE_IPC_ENV).is_ok_and(|value| !value.is_empty());
        if dev {
            Ok(Self::dev(config))
        } else {
            Self::from_manifest(config)
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
        let Some(resolved) = self.resolve(entry) else {
            return String::new();
        };
        resolved
            .css
            .iter()
            .map(|href| format!("<link rel=\"stylesheet\" href=\"{href}\" />"))
            .collect::<Vec<_>>()
            .join("\n  ")
    }

    /// The end-of-`<body>` markup for an entry: the module script, preceded in
    /// development by Vite's HMR client.
    ///
    /// Returns an empty string when the entry is not in the manifest -- an
    /// empty page is a clearer failure than a `<script>` pointing at a 404.
    #[must_use]
    pub fn body_tags(&self, entry: &str) -> String {
        let Some(resolved) = self.resolve(entry) else {
            return String::new();
        };
        let script = format!("<script type=\"module\" src=\"{}\"></script>", resolved.js);
        if self.is_dev() {
            // `/@vite/client` is what the dev proxy recognises and forwards;
            // without it there is no HMR socket and no style injection.
            format!("<script type=\"module\" src=\"/@vite/client\"></script>\n  {script}")
        } else {
            script
        }
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
    fn a_missing_manifest_is_a_startup_error_not_a_silent_dev_fallback() {
        let config = AssetsConfig::new().public_dir("this-directory-does-not-exist");
        let error = Assets::from_manifest(&config).expect_err("no manifest");
        assert!(matches!(error, AssetsError::ManifestMissing { .. }));
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
