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

/// The Inertia adapter entry point extracted in a handler. Carries the
/// request context and resolved configuration.
#[derive(Clone)]
pub struct Inertia {
    request: Arc<InertiaRequest>,
    config: InertiaConfig,
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
        let mut metadata = resolved.metadata;
        metadata.apply_options(options);
        let page = Page {
            component: component.to_string(),
            props: serde_json::Value::Object(resolved.props),
            url: self.request.url().to_string(),
            version: self.config.version().as_str().to_string(),
            metadata,
        };
        self.respond(page)
    }

    fn respond(&self, page: Page) -> Result<Response, InertiaError> {
        if self.request.is_inertia() {
            let json = serialize(&page)?;
            Ok(json_response(json))
        } else {
            html(&page, &self.config)
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
        Ok(Inertia { request, config })
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
        req = Request::from_parts(parts, body);

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let resp = inner.call(req).await?;
            Ok(post_process(resp, &request_context))
        })
    }
}

fn version_mismatch_response(config: &InertiaConfig, request: &InertiaRequest) -> Option<Response> {
    if request.method() != Method::GET || !request.is_inertia() {
        return None;
    }
    if request.request_version().unwrap_or_default() == config.version().as_str() {
        return None;
    }
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(request.url()) {
        headers.insert(Headers::LOCATION, v);
    } else {
        return None;
    }
    if let Ok(v) = HeaderValue::from_str(config.version().as_str()) {
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
