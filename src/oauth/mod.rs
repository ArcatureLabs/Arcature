//! OAuth 2.0 Authorization Code with PKCE.
//!
//! Provider-agnostic: an application names its own authorization and token
//! endpoints. The bundled providers are plain consts holding those URLs, not
//! a closed enum -- a provider the framework has never heard of is configured
//! the same way as GitHub.
