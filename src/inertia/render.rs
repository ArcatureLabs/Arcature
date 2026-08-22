//! The handler-facing Inertia entry point, the Axum extractor, and the
//! Inertia middleware layer.

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::body::HttpBody;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use super::config::InertiaConfig;
use super::error::InertiaError;
use super::headers::Headers;
use super::page::{Component, Page, PageOptions};
use super::props::Props;
use super::request::InertiaRequest;
use super::response::{ensure_vary_x_inertia, html, json_response, serialize};
use crate::http::security::CspNonce;

/// The Inertia adapter entry point extracted in a handler. Carries the
/// request context, the resolved configuration, and this request's
/// Content-Security-Policy nonce when one was minted.
#[derive(Clone)]
pub struct Inertia {
    request: Arc<InertiaRequest>,
    config: InertiaConfig,
    nonce: Option<CspNonce>,
}

impl Inertia {
    /// The parsed Inertia request context.
    pub fn request(&self) -> &InertiaRequest {
        &self.request
    }

    /// The resolved Inertia configuration.
    pub fn config(&self) -> &InertiaConfig {
        &self.config
    }

    /// This request's Content-Security-Policy nonce, if
    /// [`SecurityHeaders::with_csp_nonce`] is installed.
    ///
    /// The renderer already puts it on the payload script; this is for a
    /// handler that renders something else of its own.
    ///
    /// [`SecurityHeaders::with_csp_nonce`]: crate::http::security::SecurityHeaders::with_csp_nonce
    pub fn nonce(&self) -> Option<&CspNonce> {
        self.nonce.as_ref()
    }

    /// Render a page from any serializable props. The normal path: serializes
    /// `props` to a JSON object and renders the page. On a first visit
    /// (non-Inertia request) returns the initial HTML; on an Inertia visit
    /// returns the JSON page object.
    pub async fn render(
        &self,
        component: impl Into<Component>,
        props: impl serde::Serialize,
    ) -> Result<Response, InertiaError> {
        self.render_with_options(component, props, PageOptions::new())
            .await
    }

    /// Render a page behind the Client Exposure Firewall.
    ///
    /// Identical to [`render`](Self::render) except for the bound: `P` must
    /// implement [`ClientData`](crate::inertia::contracts::ClientData), the
    /// explicit browser-safety opt-in the `#[page]` macro generates. A type
    /// that merely derives `Serialize` -- an internal domain model, say --
    /// does not compile here, so it cannot reach the browser by accident.
    ///
    /// `render` stays available for ad-hoc JSON props (the `inertia!()`
    /// macro path); `render_page` is the typed path a `#[page]` prop struct
    /// travels.
    pub async fn render_page<P>(
        &self,
        contract: crate::inertia::contracts::PageContract<P>,
        props: P,
    ) -> Result<Response, InertiaError>
    where
        P: crate::inertia::contracts::ClientData,
    {
        self.render(contract.name(), props).await
    }

    /// Render with page-level options (history flags, flash data).
    pub async fn render_with_options(
        &self,
        component: impl Into<Component>,
        props: impl serde::Serialize,
        options: PageOptions,
    ) -> Result<Response, InertiaError> {
        let page_props = serde_json::to_value(&props)?;
        let props = Props::from_serialized(page_props)?;
        self.render_advanced_with_options(component, props, options)
            .await
    }

    /// Render with advanced per-prop behavior (deferred, optional, merge).
    pub async fn render_advanced(
        &self,
        component: impl Into<Component>,
        props: Props,
    ) -> Result<Response, InertiaError> {
        self.render_advanced_with_options(component, props, PageOptions::new())
            .await
    }

    /// The single sink: resolve props, build the page object, dispatch JSON or
    /// HTML based on whether this is an Inertia request.
    pub async fn render_advanced_with_options(
        &self,
        component: impl Into<Component>,
        props: Props,
        options: PageOptions,
    ) -> Result<Response, InertiaError> {
        let component = component.into();
        let resolved = super::props::resolve(
            props,
            self.config.shared_props(),
            &self.request,
            component.as_str(),
        )
        .await?;
        let status = options.resolved_status();
        let mut metadata = resolved.metadata;
        metadata.apply_options(options);
        let page = Page {
            component: component.to_string(),
            props: serde_json::Value::Object(resolved.props),
            url: self.request.url().to_string(),
            version: self.config.version().map(|v| v.as_str().to_string()),
            metadata,
        };
        self.respond(page, status)
    }

    fn respond(&self, page: Page, status: StatusCode) -> Result<Response, InertiaError> {
        if self.request.is_inertia() {
            let json = serialize(&page)?;
            Ok(json_response(json, status))
        } else {
            html(&page, &self.config, self.nonce.clone(), None, status)
        }
    }

    /// Build a standard redirect (302/303) selecting the status from the
    /// request method.
    pub fn redirect(&self, location: impl Into<String>) -> super::redirect::Redirect {
        super::redirect::Redirect::to(location, self.request.method().clone())
    }
}

impl<S> FromRequestParts<S> for Inertia
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let Some(config) = parts.extensions.get::<InertiaConfig>().cloned() else {
            return Err(InertiaError::ConfigMissing.into_response());
        };
        let request = parts
            .extensions
            .get::<InertiaRequest>()
            .cloned()
            .map(Arc::new)
            .unwrap_or_else(|| {
                Arc::new(InertiaRequest::parse(
                    &parts.headers,
                    &parts.method,
                    &parts.uri,
                ))
            });
        // Put there by `SecurityHeaders` (pipeline stage 6) on the way down,
        // long before this extractor runs. Absent whenever the application did
        // not ask for a nonce, which is the common case.
        let nonce = parts.extensions.get::<CspNonce>().cloned();
        Ok(Inertia {
            request,
            config,
            nonce,
        })
    }
}

// --- The Inertia middleware layer -------------------------------------------

/// A Tower layer that installs the Inertia protocol behavior on a router.
#[derive(Clone)]
pub struct InertiaLayer {
    config: InertiaConfig,
}

impl InertiaLayer {
    /// Create a layer with the given Inertia configuration.
    pub fn new(config: InertiaConfig) -> Self {
        InertiaLayer { config }
    }
}

impl<S> Layer<S> for InertiaLayer {
    type Service = InertiaMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        InertiaMiddleware {
            inner,
            config: self.config.clone(),
        }
    }
}

/// The service produced by [`InertiaLayer`].
#[derive(Clone)]
pub struct InertiaMiddleware<S> {
    inner: S,
    config: InertiaConfig,
}

impl<S, ReqBody> Service<Request<ReqBody>> for InertiaMiddleware<S>
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

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let config = self.config.clone();
        let (mut parts, body) = req.into_parts();
        let request_context = InertiaRequest::parse(&parts.headers, &parts.method, &parts.uri);

        // GET-only asset-version mismatch short-circuit.
        if let Some(short_circuit) = version_mismatch_response(&config, &request_context) {
            drop(body);
            return Box::pin(async move { Ok(short_circuit) });
        }

        parts.extensions.insert(config.clone());
        parts.extensions.insert(request_context.clone());
        // Read, not inserted: `SecurityHeaders` (stage 6) minted it well
        // outside this layer. A deferred `Page<T>` render happens below,
        // after the request has been consumed, so the value has to be taken
        // out while the parts are still in hand.
        let nonce = parts.extensions.get::<CspNonce>().cloned();
        req = Request::from_parts(parts, body);

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let resp = inner.call(req).await?;
            // Deferred renders first: a `Page<T>` handler could only record
            // what to render, because `IntoResponse` never sees the request.
            // This is the first point that holds both halves. It must happen
            // before `post_process`, whose empty-body rule would otherwise
            // read the placeholder as "handler returned nothing".
            let resp = render_pending(resp, &request_context, &config, nonce).await;
            Ok(post_process(resp, &request_context))
        })
    }
}

/// Perform a render a `Page<T>` deferred to this layer, if there is one.
///
/// Responses without a [`PendingPage`](super::pending::PendingPage) pass
/// through untouched -- a handler that built its own response, a redirect, an
/// error, a static file.
async fn render_pending(
    mut resp: Response,
    request: &InertiaRequest,
    config: &InertiaConfig,
    nonce: Option<CspNonce>,
) -> Response {
    let Some(pending) = resp
        .extensions_mut()
        .remove::<super::pending::PendingPage>()
    else {
        return resp;
    };
    let (component, props) = pending.into_parts();
    let inertia = Inertia {
        request: Arc::new(request.clone()),
        config: config.clone(),
        nonce,
    };
    let mut rendered = match inertia.render(component, props).await {
        Ok(rendered) => rendered,
        Err(error) => return error.into_response(),
    };

    // The status belongs to the render -- the placeholder's was a stand-in
    // for "nobody rendered this". Headers the handler set are its own
    // (`Set-Cookie` from a flash, a `Cache-Control`), so they carry over,
    // except the two that describe the body that was just replaced.
    for (name, value) in resp.headers() {
        if name == axum::http::header::CONTENT_TYPE
            || name == axum::http::header::CONTENT_LENGTH
            || rendered.headers().contains_key(name)
        {
            continue;
        }
        rendered.headers_mut().append(name.clone(), value.clone());
    }
    rendered
}

fn version_mismatch_response(config: &InertiaConfig, request: &InertiaRequest) -> Option<Response> {
    if request.method() != Method::GET || !request.is_inertia() {
        return None;
    }
    // Absent on both sides is a match: an application with no build step
    // has no version to compare, and the client sends none back.
    let current = config.version_str();
    if request.request_version().unwrap_or_default() == current {
        return None;
    }
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(request.url()) {
        headers.insert(Headers::LOCATION, v);
    } else {
        return None;
    }
    if let Ok(v) = HeaderValue::from_str(current) {
        headers.insert(Headers::VERSION, v);
    } else {
        return None;
    }
    ensure_vary_x_inertia(&mut headers);
    Some((StatusCode::CONFLICT, headers, Body::empty()).into_response())
}

fn post_process(mut resp: Response, request: &InertiaRequest) -> Response {
    ensure_vary_x_inertia(resp.headers_mut());

    // Empty-response fallback: Inertia request with 200 + empty body -> 302
    // to referer (or request URL).
    if request.is_inertia()
        && resp.status() == StatusCode::OK
        && resp.body().size_hint().exact() == Some(0)
    {
        let destination = request.referer().unwrap_or_else(|| request.url());
        if let Ok(location) = HeaderValue::from_str(destination) {
            *resp.status_mut() = StatusCode::FOUND;
            resp.headers_mut()
                .insert(axum::http::header::LOCATION, location);
        }
    }

    // Convert bare 302 after PUT/PATCH/DELETE to 303.
    //
    // Only 302, and deliberately so. 302 is the ambiguous one -- the spec
    // says preserve the method, every browser turns it into a GET -- so the
    // protocol pins it down. 307 and 308 are unambiguous: a handler that
    // returned one asked for the method to be repeated, and overriding that
    // would be Arcature inventing a rule the official adapters do not have.
    if request.is_inertia()
        && matches!(
            request.method(),
            &Method::PUT | &Method::PATCH | &Method::DELETE
        )
        && resp.status() == StatusCode::FOUND
    {
        *resp.status_mut() = StatusCode::SEE_OTHER;
    }

    // Fragment redirect: 3xx with a `#` in Location -> 409 + X-Inertia-Redirect.
    if request.is_inertia() && !request.is_prefetch() {
        let is_redirect = matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308);
        if is_redirect
            && let Some(location) = resp.headers().get(axum::http::header::LOCATION).cloned()
            && let Ok(loc_str) = location.to_str()
            && loc_str.contains('#')
        {
            let mut headers = HeaderMap::new();
            headers.insert(Headers::REDIRECT, location);
            ensure_vary_x_inertia(&mut headers);
            *resp.status_mut() = StatusCode::CONFLICT;
            resp.headers_mut().remove(axum::http::header::LOCATION);
            for (k, v) in headers {
                if let Some(k) = k {
                    resp.headers_mut().append(k, v);
                }
            }
        }
    }

    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header::{CONTENT_TYPE, LOCATION, REFERER};
    use axum::http::{HeaderName, Uri};

    fn config(version: Option<&str>) -> InertiaConfig {
        let document = super::super::config::default_root_document("Test");
        match version {
            Some(version) => InertiaConfig::new(version, document).expect("config"),
            None => InertiaConfig::versionless(document),
        }
    }

    fn request(method: Method, pairs: &[(&'static str, &str)]) -> InertiaRequest {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                HeaderName::from_static(name),
                HeaderValue::from_str(value).expect("header value"),
            );
        }
        InertiaRequest::parse(&headers, &method, &Uri::from_static("/users"))
    }

    fn inertia_get() -> InertiaRequest {
        request(Method::GET, &[("x-inertia", "true")])
    }

    fn empty(status: StatusCode) -> Response {
        Response::builder()
            .status(status)
            .body(Body::empty())
            .expect("response")
    }

    fn redirect_to(status: StatusCode, location: &'static str) -> Response {
        let mut response = empty(status);
        response
            .headers_mut()
            .insert(LOCATION, HeaderValue::from_static(location));
        response
    }

    async fn render(request: InertiaRequest, options: PageOptions) -> Response {
        Inertia {
            request: Arc::new(request),
            config: config(Some("v1")),
            nonce: None,
        }
        .render_advanced_with_options("users/index", Props::new(), options)
        .await
        .expect("render succeeds")
    }

    async fn body_of(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf-8")
    }

    #[test]
    fn a_stale_asset_version_turns_a_get_into_a_conflict() {
        let response = version_mismatch_response(
            &config(Some("v2")),
            &request(
                Method::GET,
                &[("x-inertia", "true"), ("x-inertia-version", "v1")],
            ),
        )
        .expect("a mismatch must short-circuit");
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[Headers::LOCATION], "/users");
        assert_eq!(response.headers()[Headers::VERSION], "v2");
        assert_eq!(response.headers()[Headers::VARY], "X-Inertia");
    }

    #[test]
    fn only_an_inertia_get_is_short_circuited() {
        // A stale POST still reaches the handler. The client resolves a
        // mismatch by reloading the location it is given, and a reload cannot
        // replay a form submission -- so short-circuiting one would lose it.
        let stale = &[("x-inertia", "true"), ("x-inertia-version", "v1")];
        assert!(
            version_mismatch_response(&config(Some("v2")), &request(Method::POST, stale)).is_none()
        );
        // Not an Inertia request at all: a plain browser navigation has no
        // version to be stale, and answering it with a 409 would show the
        // user an error page instead of the site.
        assert!(
            version_mismatch_response(
                &config(Some("v2")),
                &request(Method::GET, &[("x-inertia-version", "v1")])
            )
            .is_none()
        );
    }

    #[test]
    fn an_application_without_an_asset_version_never_forces_a_reload() {
        assert!(version_mismatch_response(&config(None), &inertia_get()).is_none());
    }

    #[tokio::test]
    async fn an_absent_asset_version_reaches_the_client_as_null() {
        let response = Inertia {
            request: Arc::new(inertia_get()),
            config: config(None),
            nonce: None,
        }
        .render_advanced("users/index", Props::new())
        .await
        .expect("render succeeds");
        let page: serde_json::Value =
            serde_json::from_str(&body_of(response).await).expect("json page");
        assert_eq!(page["version"], serde_json::Value::Null);
        assert_eq!(page["component"], "users/index");
        assert_eq!(page["url"], "/users");
    }

    #[tokio::test]
    async fn an_inertia_visit_gets_the_page_object_as_json() {
        let response = render(inertia_get(), PageOptions::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[Headers::INERTIA], "true");
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        let page: serde_json::Value =
            serde_json::from_str(&body_of(response).await).expect("json page");
        assert_eq!(page["version"], "v1");
        assert_eq!(page["props"]["errors"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn a_first_visit_gets_html_carrying_the_page_object() {
        let response = render(request(Method::GET, &[]), PageOptions::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert!(!response.headers().contains_key(Headers::INERTIA));
        let html = body_of(response).await;
        assert!(html.contains("data-page=\"app\""), "{html}");
        assert!(html.contains("users\\/index"), "{html}");
    }

    #[tokio::test]
    async fn a_page_can_render_with_an_error_status() {
        let response = render(
            inertia_get(),
            PageOptions::new().status(StatusCode::NOT_FOUND),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[Headers::INERTIA], "true");
    }

    #[tokio::test]
    async fn an_error_status_survives_the_html_path_too() {
        let response = render(
            request(Method::GET, &[]),
            PageOptions::new().status(StatusCode::NOT_FOUND),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
    }

    #[test]
    fn an_empty_inertia_ok_redirects_back_to_the_referer() {
        // The handler returned nothing, which for a form submission means
        // "stay where you were". Without this the client would replace the
        // page with an empty one.
        let response = post_process(
            empty(StatusCode::OK),
            &request(
                Method::POST,
                &[("x-inertia", "true"), ("referer", "/dashboard")],
            ),
        );
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[LOCATION], "/dashboard");
    }

    #[test]
    fn an_empty_inertia_ok_falls_back_to_the_request_url() {
        let response = post_process(empty(StatusCode::OK), &inertia_get());
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[LOCATION], "/users");
    }

    #[test]
    fn a_response_with_a_body_is_left_where_it_is() {
        let response = post_process(Response::new(Body::from("{}")), &inertia_get());
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(LOCATION));
    }

    #[test]
    fn an_empty_ok_outside_inertia_is_still_an_empty_ok() {
        let response = post_process(empty(StatusCode::OK), &request(Method::GET, &[]));
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn a_found_after_a_delete_becomes_a_see_other() {
        for method in [Method::PUT, Method::PATCH, Method::DELETE] {
            let response = post_process(
                redirect_to(StatusCode::FOUND, "/users"),
                &request(method.clone(), &[("x-inertia", "true")]),
            );
            assert_eq!(response.status(), StatusCode::SEE_OTHER, "{method}");
        }
    }

    #[test]
    fn a_temporary_redirect_keeps_the_method_it_asked_to_keep() {
        // 307 says "repeat the method" on purpose. Rewriting it to 303 would
        // be this adapter overruling the handler, and no official adapter
        // does that -- only the ambiguous 302 is pinned down.
        let response = post_process(
            redirect_to(StatusCode::TEMPORARY_REDIRECT, "/users"),
            &request(Method::DELETE, &[("x-inertia", "true")]),
        );
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    }

    #[test]
    fn a_redirect_to_a_fragment_becomes_a_client_side_redirect() {
        // A fetch follows the redirect and drops the fragment on the floor,
        // so the destination has to travel in a header the client reads.
        let response = post_process(
            redirect_to(StatusCode::FOUND, "/users#team"),
            &inertia_get(),
        );
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()[Headers::REDIRECT], "/users#team");
        assert!(
            !response.headers().contains_key(LOCATION),
            "leaving Location behind would have the client follow it twice"
        );
    }

    #[test]
    fn a_prefetch_is_not_navigated_on_its_behalf() {
        // The user has not visited anything yet. Turning a prefetch into a
        // 409 would move the page they are still looking at.
        let response = post_process(
            redirect_to(StatusCode::FOUND, "/users#team"),
            &request(
                Method::GET,
                &[("x-inertia", "true"), ("purpose", "prefetch")],
            ),
        );
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[LOCATION], "/users#team");
    }

    #[test]
    fn every_response_advertises_that_it_varies_on_x_inertia() {
        // Without it a cache serves an HTML document to an Inertia visit, or
        // a JSON page object to a browser navigation.
        let response = post_process(Response::new(Body::from("hi")), &request(Method::GET, &[]));
        assert_eq!(response.headers()[Headers::VARY], "X-Inertia");
    }

    #[test]
    fn an_application_vary_is_kept_alongside_it() {
        let mut original = Response::new(Body::from("hi"));
        original
            .headers_mut()
            .insert(Headers::VARY, HeaderValue::from_static("Accept-Encoding"));
        let response = post_process(original, &inertia_get());
        assert_eq!(
            response.headers()[Headers::VARY],
            "Accept-Encoding, X-Inertia"
        );
    }

    #[test]
    fn a_referer_is_read_from_the_standard_header() {
        // Guards the header name itself: `REFERER` is the misspelling the
        // HTTP standard froze, and a typo here silently loses the fallback.
        let mut headers = HeaderMap::new();
        headers.insert(REFERER, HeaderValue::from_static("/back"));
        let parsed = InertiaRequest::parse(&headers, &Method::GET, &Uri::from_static("/users"));
        assert_eq!(parsed.referer(), Some("/back"));
    }
}
