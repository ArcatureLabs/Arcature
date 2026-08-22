//! Deciding which registered locale a request is in.
//!
//! # The whole design in one sentence
//!
//! A request proposes locales; the application's [`Catalogs`] decides which
//! of them exist; anything unproposed, unparseable or unregistered becomes
//! the default.
//!
//! That sentence is the security property, and it is worth being explicit
//! about the alternative it rules out. The natural implementation of "let the
//! user pick a language" reads a tag out of a header or a `?lang=` and uses
//! it -- to build `locales/{tag}.ftl`, to index a `HashMap` that was keyed by
//! whatever a config file happened to contain, to compose a redirect. Every
//! one of those is a path from an attacker-controlled string to something
//! that is not a string, and the first is directory traversal with a
//! `../../etc/passwd` payload that has been in every web-application
//! checklist for twenty-five years.
//!
//! Here, a proposed tag has exactly two things done to it:
//!
//! 1. [`LocaleId::parse`], which refuses anything that is not a canonical
//!    BCP-47 language identifier of at most 35 bytes -- no `/`, no `\`, no
//!    `.`, no NUL, no newline, no control character, nothing non-ASCII;
//! 2. a lookup in [`Catalogs`], which is an in-memory map whose keys came
//!    from the application's own source at startup.
//!
//! A tag that fails either is discarded and the next candidate is tried.
//! Nothing else in this module consumes a request-supplied string, and
//! nothing anywhere in `i18n` opens a file.
//!
//! # Precedence
//!
//! [`LocaleSource`], most specific first:
//!
//! * **`?lang=fr`** ([`LocaleSource::Url`]) -- an explicit act, taken this
//!   instant, and the one thing a user can do when the other two are wrong.
//!   Off unless the application names the parameter.
//! * **the session** ([`LocaleSource::Session`]) -- an explicit act taken
//!   earlier. Off unless the application names the key. This module reads the
//!   session and never writes it: persisting a choice is a decision with a
//!   cookie and a lifetime attached, and it belongs to the handler that
//!   offers the language switcher.
//! * **`Accept-Language`** ([`LocaleSource::Header`]) -- what the browser was
//!   configured with, which is a good guess and never a statement.
//! * **the default** ([`LocaleSource::Default`]).
//!
//! # Caching
//!
//! [`LocaleLayer`] adds `Vary: Accept-Language` to every response it passes
//! and sets `Content-Language` to the locale it chose. The `Vary` is not
//! decoration: without it a shared cache stores one representation per URL
//! and serves the French page to the next English reader, which is a
//! correctness bug at best and a privacy one as soon as a page contains
//! anything about the person who requested it.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::{ACCEPT_LANGUAGE, CONTENT_LANGUAGE, VARY};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Request};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use super::args::TranslationArgs;
use super::catalog::{Catalog, Catalogs};
use super::error::I18nError;
use super::locale::LocaleId;

/// The longest `Accept-Language` this module will read.
///
/// A header is attacker-controlled in both content and length. The parse is
/// linear, so a long one is not a complexity attack, but there is no reason
/// to walk a megabyte of `en;q=0.9,` to reach an answer that the first
/// handful of entries already decided. A real browser sends well under 100
/// bytes.
const MAX_ACCEPT_LANGUAGE_LEN: usize = 512;

/// The most candidates read out of one `Accept-Language`.
///
/// Firefox sends at most a dozen. Anything past this is padding.
const MAX_CANDIDATES: usize = 16;

/// Where the active locale came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocaleSource {
    /// A URL parameter named by
    /// [`LocaleNegotiator::query_parameter`].
    Url,
    /// A session entry named by [`LocaleNegotiator::session_key`].
    Session,
    /// The request's `Accept-Language`.
    Header,
    /// Nothing the request offered was registered.
    Default,
}

/// The locale this request is being served in, and the catalogs to serve it
/// from.
///
/// Put into the request's extensions by [`LocaleLayer`] and taken out again
/// by the [`FromRequestParts`] impl, so a handler asks for one by naming it:
///
/// ```
/// use arcature::i18n::Locale;
/// use arcature::prelude::*;
///
/// async fn greet(locale: Locale) -> Result<Response> {
///     Ok(text(StatusCode::OK, locale.message("greeting")?))
/// }
/// ```
///
/// Cloning is cheap: the tag is an `Arc<str>` and the catalogs are behind an
/// `Arc`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Locale {
    id: LocaleId,
    source: LocaleSource,
    catalogs: Catalogs,
}

impl Locale {
    /// The active locale's tag.
    #[must_use]
    pub fn id(&self) -> &LocaleId {
        &self.id
    }

    /// Where the active locale came from.
    ///
    /// Worth reading in a language switcher, which wants to show the current
    /// choice as chosen rather than as inferred.
    #[must_use]
    pub fn source(&self) -> LocaleSource {
        self.source
    }

    /// Whether the locale was actually asked for, rather than fallen back to.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.source == LocaleSource::Default
    }

    /// Every catalog the application registered.
    #[must_use]
    pub fn catalogs(&self) -> &Catalogs {
        &self.catalogs
    }

    /// This locale's catalog.
    ///
    /// Always present: negotiation only ever selects a registered locale.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        self.catalogs
            .catalog(&self.id)
            .unwrap_or_else(|| self.catalogs.default_catalog())
    }

    /// Format a no-argument message in this locale.
    ///
    /// # Errors
    ///
    /// As [`Catalogs::translate`].
    pub fn message(&self, key: &str) -> Result<String, I18nError> {
        self.catalogs.message(&self.id, key)
    }

    /// Format a message in this locale.
    ///
    /// # Errors
    ///
    /// As [`Catalogs::translate`].
    pub fn translate(&self, key: &str, args: &TranslationArgs) -> Result<String, I18nError> {
        self.catalogs.translate(&self.id, key, args)
    }
}

impl<S> FromRequestParts<S> for Locale
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Deliberately not "negotiate one here from the headers". Doing that
        // would need a `Catalogs`, which only the layer has, and it would
        // make a route that forgot the layer answer in a language nobody
        // negotiated instead of saying the wiring is missing.
        parts
            .extensions
            .get::<Locale>()
            .cloned()
            .ok_or_else(|| crate::Error::from(I18nError::NotNegotiated).into_response())
    }
}

/// How a request's proposals are turned into an active locale.
///
/// Built once at startup and handed to [`LocaleLayer`].
///
/// ```
/// use arcature::i18n::{Catalog, Catalogs, LocaleId, LocaleNegotiator, LocaleSource};
///
/// let catalogs = Catalogs::new(
///     Catalog::parse(LocaleId::parse("en").unwrap(), "hi = Hello").unwrap(),
/// )
/// .with(Catalog::parse(LocaleId::parse("fr").unwrap(), "hi = Bonjour").unwrap());
///
/// let negotiator = LocaleNegotiator::new(catalogs)
///     .query_parameter("lang")
///     .session_key("locale");
///
/// // A browser configured for French, with no explicit choice on record.
/// let locale = negotiator.resolve(None, None, Some("fr-CA,fr;q=0.9,en;q=0.4"));
/// assert_eq!(locale.id().as_str(), "fr");
/// assert_eq!(locale.source(), LocaleSource::Header);
///
/// // An explicit `?lang=` beats both the session and the header.
/// let locale = negotiator.resolve(Some("en"), Some("fr"), Some("fr"));
/// assert_eq!(locale.id().as_str(), "en");
/// assert_eq!(locale.source(), LocaleSource::Url);
///
/// // And a hostile one is simply not a candidate.
/// let locale = negotiator.resolve(Some("../../etc/passwd"), None, None);
/// assert_eq!(locale.id(), catalogs_default(&negotiator));
/// assert_eq!(locale.source(), LocaleSource::Default);
/// # fn catalogs_default(n: &LocaleNegotiator) -> &LocaleId { n.catalogs().default_locale() }
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LocaleNegotiator {
    catalogs: Catalogs,
    query_parameter: Option<Arc<str>>,
    session_key: Option<Arc<str>>,
}

impl LocaleNegotiator {
    /// Negotiate against `catalogs`, from `Accept-Language` only.
    ///
    /// Both overrides start off. A URL parameter is a public switch that puts
    /// the locale into every link a page emits and into every log line, and a
    /// session key is a write to storage the application owns; neither should
    /// appear because a framework assumed a name for it.
    #[must_use]
    pub fn new(catalogs: Catalogs) -> Self {
        Self {
            catalogs,
            query_parameter: None,
            session_key: None,
        }
    }

    /// Read an override from this query parameter, e.g. `"lang"` for
    /// `?lang=fr`.
    ///
    /// The value is taken verbatim, without percent-decoding: a well-formed
    /// locale tag is `[A-Za-z0-9-]` and needs none, so a percent-encoded one
    /// fails validation and falls through to the next source. That is a
    /// deliberate trade -- one decoder fewer between a request and a lookup.
    #[must_use]
    pub fn query_parameter(mut self, name: impl Into<Arc<str>>) -> Self {
        self.query_parameter = Some(name.into());
        self
    }

    /// Read an override from this session key.
    ///
    /// Requires the `auth` feature, which is what brings `tower-sessions`;
    /// without it the key is accepted and never consulted, so enabling `auth`
    /// later does not change a call site.
    #[must_use]
    pub fn session_key(mut self, key: impl Into<Arc<str>>) -> Self {
        self.session_key = Some(key.into());
        self
    }

    /// The catalogs this negotiator matches against.
    #[must_use]
    pub fn catalogs(&self) -> &Catalogs {
        &self.catalogs
    }

    /// The locale used when a request proposes nothing usable.
    #[must_use]
    pub fn fallback(&self) -> Locale {
        Locale {
            id: self.catalogs.default_locale().clone(),
            source: LocaleSource::Default,
            catalogs: self.catalogs.clone(),
        }
    }

    /// Choose a locale from what a request proposed.
    ///
    /// Every argument is untrusted. `url` and `session` are single tags;
    /// `accept_language` is a raw `Accept-Language` field value. Any of them
    /// may be absent, malformed, hostile, or name a locale this application
    /// does not have, and each of those cases falls through to the next
    /// source.
    #[must_use]
    pub fn resolve(
        &self,
        url: Option<&str>,
        session: Option<&str>,
        accept_language: Option<&str>,
    ) -> Locale {
        if self.query_parameter.is_some()
            && let Some(id) = url.and_then(|tag| self.registered(tag))
        {
            return self.at(id, LocaleSource::Url);
        }
        if self.session_key.is_some()
            && let Some(id) = session.and_then(|tag| self.registered(tag))
        {
            return self.at(id, LocaleSource::Session);
        }
        if let Some(id) = accept_language.and_then(|header| self.match_accept_language(header)) {
            return self.at(id, LocaleSource::Header);
        }
        self.fallback()
    }

    /// Validate one proposed tag and match it against the whitelist.
    ///
    /// The only function in this module that a request's bytes reach, and it
    /// returns a `LocaleId` that is known to be in [`Catalogs`] or nothing at
    /// all. There is no third outcome, and in particular no outcome in which
    /// the caller's string is returned.
    fn registered(&self, tag: &str) -> Option<LocaleId> {
        let id = LocaleId::parse(tag).ok()?;
        if self.catalogs.contains(&id) {
            return Some(id);
        }
        // `fr-CA` when only `fr` is registered, or `fr` when only `fr-CA` is.
        // Matched on the language subtag of an already-validated identifier,
        // never on a prefix of the raw string -- a prefix match on raw bytes
        // would make `en/../../etc` match `en`.
        self.catalogs
            .locales()
            .find(|registered| registered.language() == id.language())
            .cloned()
    }

    fn at(&self, id: LocaleId, source: LocaleSource) -> Locale {
        Locale {
            id,
            source,
            catalogs: self.catalogs.clone(),
        }
    }

    /// The best registered locale named by an `Accept-Language` field value.
    fn match_accept_language(&self, header: &str) -> Option<LocaleId> {
        for (tag, _) in accept_language_candidates(header) {
            if let Some(id) = self.registered(tag) {
                return Some(id);
            }
        }
        None
    }
}

/// Parse an `Accept-Language` field value into candidate tags, best first.
///
/// Returns the range strings unvalidated and unresolved -- deciding whether
/// one is a locale at all is [`LocaleId::parse`]'s job, and this function
/// deliberately does not duplicate it. `*` is dropped: it means "anything",
/// which is what falling through to the default already does.
///
/// The sort is stable, so equal q-values keep the order the client sent, and
/// RFC 9110's "no q means 1" is honoured. A malformed q is treated as absent
/// rather than as zero: a client that garbles a weight still wanted the
/// language it named.
fn accept_language_candidates(header: &str) -> Vec<(&str, u16)> {
    let header = &header[..header.len().min(MAX_ACCEPT_LANGUAGE_LEN)];

    let mut candidates: Vec<(&str, u16)> = Vec::new();
    for entry in header.split(',').take(MAX_CANDIDATES) {
        let mut parts = entry.split(';');
        let Some(range) = parts.next().map(str::trim) else {
            continue;
        };
        if range.is_empty() || range == "*" {
            continue;
        }

        // Weights are thousandths, so they compare as integers: `q=0.8` is
        // 800. Comparing `f32`s here would mean sorting on a partial order.
        let quality = parts
            .filter_map(|parameter| {
                let parameter = parameter.trim();
                let value = parameter.strip_prefix("q=").or_else(|| {
                    parameter
                        .strip_prefix("Q=")
                        .or_else(|| parameter.strip_prefix("q ="))
                })?;
                let weight: f32 = value.trim().parse().ok()?;
                if (0.0..=1.0).contains(&weight) {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "the range check above bounds the product to 0..=1000"
                    )]
                    Some((weight * 1000.0) as u16)
                } else {
                    None
                }
            })
            .next()
            .unwrap_or(1000);

        if quality == 0 {
            // `q=0` is a client saying "not this one", and honouring it is
            // the difference between a preference list and a wish list.
            continue;
        }
        candidates.push((range, quality));
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.1));
    candidates
}

/// A Tower layer that negotiates a locale for every request and puts it in
/// the request's extensions.
///
/// ```no_run
/// use arcature::axum::Router;
/// use arcature::axum::routing::get;
/// use arcature::i18n::{Catalog, Catalogs, Locale, LocaleId, LocaleLayer, LocaleNegotiator};
///
/// let catalogs = Catalogs::new(
///     Catalog::parse(LocaleId::parse("en").unwrap(), "hi = Hello").unwrap(),
/// );
///
/// let app: Router = Router::new()
///     .route("/", get(|locale: Locale| async move { locale.message("hi").unwrap() }))
///     .layer(LocaleLayer::new(
///         LocaleNegotiator::new(catalogs).query_parameter("lang"),
///     ));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LocaleLayer {
    negotiator: Arc<LocaleNegotiator>,
}

impl LocaleLayer {
    /// Install `negotiator` on a router.
    #[must_use]
    pub fn new(negotiator: LocaleNegotiator) -> Self {
        Self {
            negotiator: Arc::new(negotiator),
        }
    }
}

impl<S> Layer<S> for LocaleLayer {
    type Service = LocaleMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LocaleMiddleware {
            inner,
            negotiator: Arc::clone(&self.negotiator),
        }
    }
}

/// The service produced by [`LocaleLayer`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LocaleMiddleware<S> {
    inner: S,
    negotiator: Arc<LocaleNegotiator>,
}

impl<S, ReqBody> Service<Request<ReqBody>> for LocaleMiddleware<S>
where
    S: Service<Request<ReqBody>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        let negotiator = Arc::clone(&self.negotiator);
        let (mut parts, body) = request.into_parts();

        // Cloned out of the parts rather than borrowed across the await: the
        // session read below is async, and `parts` has to be reassembled into
        // a request afterwards.
        let url = negotiator
            .query_parameter
            .as_deref()
            .and_then(|name| query_value(parts.uri.query(), name))
            .map(str::to_owned);
        let accept_language = parts
            .headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let session = session_value(&parts, negotiator.session_key.as_deref()).await;

            let locale = negotiator.resolve(
                url.as_deref(),
                session.as_deref(),
                accept_language.as_deref(),
            );
            let tag = locale.id().clone();
            parts.extensions.insert(locale);

            let mut response = inner.call(Request::from_parts(parts, body)).await?;
            annotate(response.headers_mut(), &tag);
            Ok(response)
        })
    }
}

/// Read one query parameter's raw value.
///
/// No percent-decoding, by the argument on
/// [`LocaleNegotiator::query_parameter`]. The last occurrence wins, matching
/// what `serde_urlencoded` does for a repeated key, so a parameter smuggled
/// in ahead of the real one does not take precedence over it.
fn query_value<'q>(query: Option<&'q str>, name: &str) -> Option<&'q str> {
    query?
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then_some(value)
        })
        .next_back()
}

/// Read the locale the application stored in the session, if it stores one.
///
/// Cross-gated on `auth`, which is what puts `tower-sessions` in the graph.
/// Without it the whole thing is a `None` the optimiser deletes.
#[cfg(feature = "auth")]
async fn session_value(parts: &Parts, key: Option<&str>) -> Option<String> {
    let key = key?;
    let session = parts.extensions.get::<tower_sessions::Session>()?;
    // A session value is application-written, but it is still not trusted
    // here: it goes through `LocaleId::parse` with everything else. A session
    // is a store the application shares with code that may have written
    // something else into it, and "we wrote it, so it is fine" is how a
    // validated field stops being validated.
    session.get::<String>(key).await.ok().flatten()
}

#[cfg(not(feature = "auth"))]
#[expect(
    clippy::unused_async,
    reason = "matches the `auth` signature so the call site needs no cfg"
)]
async fn session_value(_parts: &Parts, _key: Option<&str>) -> Option<String> {
    None
}

/// Say what language the body is in, and that the answer depends on the
/// request's own `Accept-Language`.
fn annotate(headers: &mut HeaderMap, locale: &LocaleId) {
    // The tag is canonical BCP-47 -- ASCII alphanumerics and dashes -- so it
    // is always a valid header value. The fallible constructor is used rather
    // than asserting that.
    if !headers.contains_key(CONTENT_LANGUAGE)
        && let Ok(value) = HeaderValue::from_str(locale.as_str())
    {
        headers.insert(CONTENT_LANGUAGE, value);
    }
    ensure_vary_accept_language(headers);
}

/// Add `Accept-Language` to `Vary`, keeping whatever was already there.
fn ensure_vary_accept_language(headers: &mut HeaderMap) {
    const ACCEPT_LANGUAGE_TOKEN: &str = "Accept-Language";

    let mut tokens: Vec<String> = Vec::new();
    for value in headers.get_all(VARY) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            // `Vary: *` already says "do not reuse this for anyone else",
            // which is stronger than anything added here.
            if token == "*" {
                return;
            }
            if token.eq_ignore_ascii_case(ACCEPT_LANGUAGE_TOKEN) {
                return;
            }
            tokens.push(token.to_owned());
        }
    }

    tokens.push(ACCEPT_LANGUAGE_TOKEN.to_owned());
    if let Ok(value) = HeaderValue::from_str(&tokens.join(", ")) {
        headers.remove(VARY);
        headers.insert(VARY, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Catalog;

    fn catalogs() -> Catalogs {
        Catalogs::new(Catalog::parse(id("en"), "hi = Hello").unwrap())
            .with(Catalog::parse(id("fr"), "hi = Bonjour").unwrap())
            .with(Catalog::parse(id("pt-BR"), "hi = Ola").unwrap())
    }

    fn id(tag: &str) -> LocaleId {
        LocaleId::parse(tag).unwrap()
    }

    fn negotiator() -> LocaleNegotiator {
        LocaleNegotiator::new(catalogs())
            .query_parameter("lang")
            .session_key("locale")
    }

    // --- The security property ---------------------------------------------

    /// The test the whole module is written for.
    ///
    /// Every string here is something a request can carry. None of them may
    /// select a locale, and none of them may appear in the outcome: what
    /// comes back is the application's default, which is a value that was
    /// never derived from the input at all.
    ///
    /// "Never reaches the filesystem" is structural rather than observed.
    /// `i18n` performs no filesystem access: there is no `std::fs` in the
    /// module, catalogs are values the application constructs, and the
    /// registry is a `BTreeMap` in memory. A traversal payload has nothing to
    /// traverse. What this test pins is the layer in front of that -- that a
    /// hostile tag is rejected before it is used for anything, so the
    /// property survives a future change that does introduce a path.
    #[test]
    fn a_hostile_locale_never_selects_anything() {
        let negotiator = negotiator();
        let hostile = [
            "../../etc/passwd",
            "../../../../../../etc/shadow",
            "..\\..\\..\\windows\\win.ini",
            "/etc/passwd",
            "C:\\Windows\\System32",
            "en/../../etc/passwd",
            "fr/../../../root/.ssh/id_rsa",
            "%2e%2e%2f%2e%2e%2fetc%2fpasswd",
            "....//....//etc/passwd",
            "en\0",
            "en\0.ftl",
            "\0/etc/passwd",
            "fr\nSet-Cookie: session=stolen",
            "fr\r\nX-Injected: 1",
            "en; rm -rf /",
            "$(cat /etc/passwd)",
            "`id`",
            "{{7*7}}",
            "<script>alert(1)</script>",
            "en\u{202e}",
            "\u{feff}en",
        ];

        for tag in hostile {
            let mut outcomes = vec![
                negotiator.resolve(Some(tag), None, None),
                negotiator.resolve(None, Some(tag), None),
            ];
            // `;` and `,` are `Accept-Language`'s own delimiters, so in that
            // position they do not make one hostile tag -- they make a list.
            // See `a_header_parameter_is_not_part_of_the_language_range`.
            if !tag.contains([';', ',']) {
                outcomes.push(negotiator.resolve(None, None, Some(tag)));
            }

            for locale in outcomes {
                assert_eq!(locale.id(), &id("en"), "{tag:?} selected {}", locale.id());
                assert_eq!(locale.source(), LocaleSource::Default, "{tag:?}");
            }
            // And it never becomes a `LocaleId` in the first place, which is
            // what stops it reaching anything that takes one.
            assert!(LocaleId::parse(tag).is_err(), "{tag:?} parsed");
        }
    }

    /// `Accept-Language: en; rm -rf /` is not a hostile tag being accepted.
    /// `;` starts a parameter list, so the language range is `en` and the
    /// rest is a parameter that is not a `q` and is discarded -- which is
    /// what RFC 9110 says the field means. What matters is that `en` still
    /// goes through `LocaleId::parse` and the whitelist like any other
    /// candidate, and that nothing after the `;` survives into the result.
    #[test]
    fn a_header_parameter_is_not_part_of_the_language_range() {
        let locale = negotiator().resolve(None, None, Some("en; rm -rf /"));
        assert_eq!(locale.id(), &id("en"));
        assert_eq!(locale.source(), LocaleSource::Header);

        // The same bytes proposed as a whole tag are still refused.
        assert!(LocaleId::parse("en; rm -rf /").is_err());
        assert_eq!(
            negotiator()
                .resolve(Some("en; rm -rf /"), None, None)
                .source(),
            LocaleSource::Default
        );

        // And a parameter cannot smuggle in a language the range did not
        // name: the range is `de`, which is not registered.
        assert_eq!(
            negotiator().resolve(None, None, Some("de; fr")).source(),
            LocaleSource::Default
        );
    }

    #[test]
    fn an_overlong_locale_never_selects_anything() {
        let negotiator = negotiator();
        for length in [36, 1024, 64 * 1024] {
            let tag = "e".repeat(length);
            let locale = negotiator.resolve(Some(&tag), None, None);
            assert_eq!(locale.id(), &id("en"));
            assert_eq!(locale.source(), LocaleSource::Default);
        }
    }

    /// A hostile string inside an otherwise ordinary header must not take the
    /// rest of the header down with it: the `fr` after the payload still
    /// wins.
    #[test]
    fn a_hostile_candidate_is_skipped_and_the_next_one_is_tried() {
        let locale = negotiator().resolve(
            None,
            None,
            Some("../../etc/passwd;q=1.0,\0;q=0.95,fr;q=0.9"),
        );
        assert_eq!(locale.id(), &id("fr"));
        assert_eq!(locale.source(), LocaleSource::Header);
    }

    /// A registered locale is matched as a whole identifier, so a raw string
    /// that merely starts with one is not "close enough".
    #[test]
    fn matching_is_not_a_prefix_match_on_the_raw_string() {
        let negotiator = negotiator();
        for tag in ["fr-", "fr..", "fr/x", "frx", "fr\u{0}"] {
            assert_eq!(
                negotiator.resolve(Some(tag), None, None).source(),
                LocaleSource::Default,
                "{tag:?}"
            );
        }
    }

    #[test]
    fn an_unregistered_but_well_formed_locale_falls_back() {
        let locale = negotiator().resolve(Some("de-DE"), None, None);
        assert_eq!(locale.id(), &id("en"));
        assert_eq!(locale.source(), LocaleSource::Default);
    }

    // --- Precedence ---------------------------------------------------------

    #[test]
    fn the_url_beats_the_session_and_the_header() {
        let locale = negotiator().resolve(Some("fr"), Some("pt-BR"), Some("en"));
        assert_eq!(locale.id(), &id("fr"));
        assert_eq!(locale.source(), LocaleSource::Url);
    }

    #[test]
    fn the_session_beats_the_header() {
        let locale = negotiator().resolve(None, Some("fr"), Some("en"));
        assert_eq!(locale.id(), &id("fr"));
        assert_eq!(locale.source(), LocaleSource::Session);
    }

    #[test]
    fn the_header_beats_the_default() {
        let locale = negotiator().resolve(None, None, Some("fr"));
        assert_eq!(locale.id(), &id("fr"));
        assert_eq!(locale.source(), LocaleSource::Header);
    }

    #[test]
    fn nothing_at_all_is_the_default() {
        let locale = negotiator().resolve(None, None, None);
        assert_eq!(locale.id(), &id("en"));
        assert!(locale.is_default());
    }

    /// An override that was never configured is not an override, so an
    /// application that has not opted into `?lang=` cannot have its locale
    /// changed by a link somebody emailed a user.
    #[test]
    fn an_unconfigured_override_is_ignored() {
        let bare = LocaleNegotiator::new(catalogs());
        assert_eq!(
            bare.resolve(Some("fr"), Some("fr"), None).source(),
            LocaleSource::Default
        );
        assert_eq!(bare.resolve(None, None, Some("fr")).id(), &id("fr"));
    }

    // --- Accept-Language ----------------------------------------------------

    #[test]
    fn quality_values_order_the_candidates() {
        let locale = negotiator().resolve(None, None, Some("de;q=1.0,fr;q=0.8,en;q=0.9"));
        assert_eq!(locale.id(), &id("en"), "0.9 beats 0.8");
    }

    #[test]
    fn a_weightless_entry_is_q_1() {
        let locale = negotiator().resolve(None, None, Some("fr;q=0.9,en"));
        assert_eq!(locale.id(), &id("en"));
    }

    #[test]
    fn equal_weights_keep_the_order_they_were_sent_in() {
        assert_eq!(
            negotiator().resolve(None, None, Some("fr,en")).id(),
            &id("fr")
        );
        assert_eq!(
            negotiator().resolve(None, None, Some("en,fr")).id(),
            &id("en")
        );
    }

    #[test]
    fn a_refused_language_is_not_selected() {
        // `q=0` means "not this one", and a list of only refusals selects
        // nothing.
        let locale = negotiator().resolve(None, None, Some("fr;q=0,pt-BR;q=0"));
        assert_eq!(locale.source(), LocaleSource::Default);
    }

    #[test]
    fn a_wildcard_is_not_a_locale() {
        assert_eq!(
            negotiator().resolve(None, None, Some("*")).source(),
            LocaleSource::Default
        );
    }

    #[test]
    fn whitespace_and_casing_are_tolerated() {
        let locale = negotiator().resolve(None, None, Some("  DE-de ;q=0.2 , FR ; q=0.9 "));
        assert_eq!(locale.id(), &id("fr"));
    }

    /// `fr-CA` is not registered but `fr` is, and a Canadian reader is much
    /// better served by French than by the application's English default.
    #[test]
    fn a_region_falls_back_to_its_language() {
        let locale = negotiator().resolve(None, None, Some("fr-CA"));
        assert_eq!(locale.id(), &id("fr"));
        assert_eq!(locale.source(), LocaleSource::Header);
    }

    /// And the other direction: only `pt-BR` is registered, and a request for
    /// plain `pt` gets it rather than English.
    #[test]
    fn a_language_falls_back_to_a_registered_region() {
        let locale = negotiator().resolve(None, None, Some("pt"));
        assert_eq!(locale.id(), &id("pt-BR"));
    }

    /// An exact match is preferred over a language match even when the exact
    /// one is later in the header.
    #[test]
    fn an_exact_match_is_taken_before_a_language_match() {
        let locale = negotiator().resolve(None, None, Some("pt-PT;q=0.9,pt-BR;q=0.9"));
        assert_eq!(locale.id(), &id("pt-BR"));
    }

    #[test]
    fn a_pathological_header_is_bounded() {
        let header = "en;q=0.1,".repeat(10_000) + "fr";
        // Reads the head of the header and stops; `fr` at the far end is
        // never seen, which is the point of the bound.
        let locale = negotiator().resolve(None, None, Some(&header));
        assert_eq!(locale.id(), &id("en"));

        let candidates = accept_language_candidates(&header);
        assert!(candidates.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn an_empty_header_is_no_header() {
        assert_eq!(
            negotiator().resolve(None, None, Some("")).source(),
            LocaleSource::Default
        );
        assert_eq!(
            negotiator().resolve(None, None, Some(",,,")).source(),
            LocaleSource::Default
        );
    }

    // --- The query reader ---------------------------------------------------

    #[test]
    fn a_query_parameter_is_read_by_name() {
        assert_eq!(query_value(Some("lang=fr"), "lang"), Some("fr"));
        assert_eq!(query_value(Some("a=1&lang=fr&b=2"), "lang"), Some("fr"));
        assert_eq!(query_value(Some("language=fr"), "lang"), None);
        assert_eq!(query_value(Some("lang"), "lang"), None);
        assert_eq!(query_value(None, "lang"), None);
    }

    #[test]
    fn a_repeated_query_parameter_resolves_to_the_last_one() {
        // Parameter smuggling: two `lang`s, and the one this reads has to be
        // the one the rest of the stack would read.
        assert_eq!(query_value(Some("lang=fr&lang=en"), "lang"), Some("en"));
    }

    // --- Response annotation ------------------------------------------------

    #[test]
    fn a_response_says_what_language_it_is_in() {
        let mut headers = HeaderMap::new();
        annotate(&mut headers, &id("pt-BR"));
        assert_eq!(headers[CONTENT_LANGUAGE], "pt-BR");
        assert_eq!(headers[VARY], "Accept-Language");
    }

    #[test]
    fn a_content_language_the_handler_set_is_left_alone() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LANGUAGE, HeaderValue::from_static("mul"));
        annotate(&mut headers, &id("fr"));
        assert_eq!(headers[CONTENT_LANGUAGE], "mul");
    }

    /// Without this a shared cache serves the French page to the next English
    /// reader.
    #[test]
    fn vary_is_merged_and_never_duplicated() {
        let mut headers = HeaderMap::new();
        headers.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
        annotate(&mut headers, &id("fr"));
        assert_eq!(headers[VARY], "Accept-Encoding, Accept-Language");

        annotate(&mut headers, &id("fr"));
        assert_eq!(headers[VARY], "Accept-Encoding, Accept-Language");
    }

    #[test]
    fn an_existing_vary_accept_language_is_recognised_whatever_its_casing() {
        let mut headers = HeaderMap::new();
        headers.insert(VARY, HeaderValue::from_static("accept-language"));
        annotate(&mut headers, &id("fr"));
        assert_eq!(headers[VARY], "accept-language");
    }

    #[test]
    fn a_vary_star_is_left_alone() {
        let mut headers = HeaderMap::new();
        headers.insert(VARY, HeaderValue::from_static("*"));
        annotate(&mut headers, &id("fr"));
        assert_eq!(headers[VARY], "*");
    }

    // --- The layer ----------------------------------------------------------

    async fn serve(uri: &str, headers: &[(&str, &str)]) -> Response {
        use axum::Router;
        use axum::routing::get;
        use tower::ServiceExt as _;

        let app: Router =
            Router::new()
                .route(
                    "/{*rest}",
                    get(|locale: Locale| async move {
                        format!("{}:{:?}", locale.id(), locale.source())
                    }),
                )
                .route(
                    "/",
                    get(|locale: Locale| async move {
                        format!("{}:{:?}", locale.id(), locale.source())
                    }),
                )
                .layer(LocaleLayer::new(negotiator()));

        let mut request = Request::get(uri);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        app.oneshot(request.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn the_layer_negotiates_from_the_header() {
        let response = serve("/", &[("accept-language", "fr-CA,fr;q=0.9")]).await;
        assert_eq!(response.headers()[CONTENT_LANGUAGE], "fr");
        assert_eq!(response.headers()[VARY], "Accept-Language");
        assert_eq!(body_of(response).await, "fr:Header");
    }

    #[tokio::test]
    async fn the_layer_reads_the_url_override() {
        let response = serve("/?lang=pt-BR", &[("accept-language", "fr")]).await;
        assert_eq!(body_of(response).await, "pt-BR:Url");
    }

    #[tokio::test]
    async fn the_layer_falls_back_on_a_hostile_url_override() {
        let response = serve("/?lang=../../etc/passwd", &[]).await;
        assert_eq!(response.headers()[CONTENT_LANGUAGE], "en");
        assert_eq!(body_of(response).await, "en:Default");
    }

    /// A header that is not valid UTF-8, or that carries bytes `to_str`
    /// refuses, must not fail the request.
    #[tokio::test]
    async fn an_unreadable_header_is_simply_absent() {
        let response = serve("/", &[("accept-language", "fr\u{e9}")]).await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }

    /// The extractor is honest about missing wiring instead of inventing a
    /// locale, and says nothing about the framework's internals while doing
    /// it.
    #[tokio::test]
    async fn the_extractor_without_the_layer_is_a_500_that_leaks_nothing() {
        use axum::Router;
        use axum::routing::get;
        use tower::ServiceExt as _;

        let app: Router = Router::new().route(
            "/",
            get(|locale: Locale| async move { locale.id().to_string() }),
        );
        let response = app
            .oneshot(Request::get("/").body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        let body = body_of(response).await;
        assert!(!body.contains("LocaleLayer"), "{body}");
        assert!(!body.contains("negotiat"), "{body}");
    }
}
