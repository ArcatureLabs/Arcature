//! Application configuration: read environment variables and build typed
//! configs for each subsystem.
//!
//! `load()` reads the process environment (`.env` has already been loaded by
//! `bootstrap::app`) and returns a [`Config`] the bootstrap layer feeds into
//! the `Application` builder. Every value has a documented default except
//! `APP_KEY`, which has none on purpose -- see [`app_key`].

use arcature::prelude::*;

/// The resolved application configuration.
pub struct Config {
    /// The human-readable application name (env `APP_NAME`).
    pub app_name: String,
    /// The externally reachable base URL, no trailing slash (env `APP_URL`).
    ///
    /// Used to build absolute links in outgoing mail, which is the one place
    /// the application cannot infer its own origin from the request.
    pub app_url: String,
    /// The 64-byte master secret, decoded from the hex in `APP_KEY`.
    pub app_key: Vec<u8>,
    /// The bind address (env `APP_BIND`, default `127.0.0.1`).
    pub bind_addr: String,
    /// The bind port (env `APP_PORT`, default `3000`).
    pub port: u16,
    /// True in a release build.
    ///
    /// This decides whether the session cookie is marked `Secure`, so it is
    /// deliberately **not** read from `APP_ENV`. A `Secure` cookie is never
    /// sent over plain HTTP, which is why it cannot simply be on always --
    /// it would break every local `http://127.0.0.1` login -- but an
    /// environment variable is the wrong switch for it: `APP_ENV` is
    /// operator-settable, so a stale or copied `APP_ENV=development` on a
    /// real deployment would silently drop `Secure` and put the session
    /// cookie on the wire in clear, with nothing anywhere reporting it. The
    /// build profile cannot be changed after the binary ships, which is the
    /// property a security default needs. This is the same key the framework
    /// uses for error redaction and for the dev-only graph endpoint.
    ///
    /// `APP_ENV` still exists, and is still what the Dockerfile sets, but it
    /// is for display and logging -- never for a security decision.
    pub production: bool,
    /// The `From` address on outgoing mail (env `MAIL_FROM`).
    pub mail_from: String,
    /// The database config (env `DATABASE_URL`).
    pub database: DatabaseConfig,
    /// The cache config, or `None` when `REDIS_URL` is unset (the default).
    ///
    /// Optional because Redis is a *separate server*, and a scaffold that
    /// will not start until one is running is a scaffold that fails the first
    /// time anyone tries it. Unset means the cache subsystem is not wired at
    /// all: nothing to connect to, nothing to time out against, and
    /// `Inject<Cache>` reports a missing resource rather than a stale one.
    /// Fill `REDIS_URL` in when you actually want a cache -- the wiring in
    /// `bootstrap::app` is already there waiting for it.
    pub cache: Option<CacheConfig>,
    /// The storage config (env `STORAGE_ROOT`, default `./storage`).
    pub storage: StorageConfig,
    /// The mail config (env `MAIL_URL`).
    pub mail: SmtpConfig,
}

/// Load configuration from the environment.
///
/// # Errors
///
/// Returns [`Error::Config`] when `APP_KEY` is missing or malformed, or when
/// any subsystem URL fails to parse.
pub fn load() -> Result<Config> {
    Ok(Config {
        app_name: env_or("APP_NAME", "__PROJECT_NAME__"),
        app_url: env_or("APP_URL", "http://127.0.0.1:3000")
            .trim_end_matches('/')
            .to_string(),
        app_key: app_key()?,
        bind_addr: env_or("APP_BIND", "127.0.0.1"),
        port: arcature::config::env_parsed("APP_PORT", 3000),
        production: !cfg!(debug_assertions),
        mail_from: env_or("MAIL_FROM", "noreply@localhost"),
        database: DatabaseConfig::new(&env_or("DATABASE_URL", "__DATABASE_URL__"))?,
        cache: cache()?,
        storage: StorageConfig::fs(&env_or("STORAGE_ROOT", "./storage"))
            .map_err(|e| Error::Config(e.to_string()))?,
        mail: SmtpConfig::from_url(&env_or("MAIL_URL", "smtp://127.0.0.1:1025"))?,
    })
}

/// Build the cache config, but only if somebody asked for a cache.
///
/// There is no default URL on purpose. Defaulting to
/// `redis://127.0.0.1:6379/0` is not a harmless guess: the builder connects
/// to whatever it is given during ordered startup, so the guess becomes a
/// hard dependency on a server the person who just ran `arc new` has no
/// reason to be running, and the application dies with
/// `cache connect: timed out` before it ever serves a route.
///
/// An empty value counts as unset. A `.env` that carries `REDIS_URL=` as a
/// placeholder line -- which is exactly what the generated one does -- should
/// mean "no cache", not "connect to the empty string".
///
/// `CacheConfigError` has no `From<...> for Error` impl; fold it into the
/// framework `Error::Config` variant.
fn cache() -> Result<Option<CacheConfig>> {
    let url = std::env::var("REDIS_URL").unwrap_or_default();
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    CacheConfig::new(url)
        .map(Some)
        .map_err(|e| Error::Config(e.to_string()))
}

/// Decode `APP_KEY` into the 64 bytes the session signer needs.
///
/// There is deliberately no default. A generated fallback key would make
/// every deployment of this scaffold share a signing secret, and a
/// zero-filled one would make the failure invisible until someone forged a
/// cookie. `arc key:generate` writes a real key into `.env`.
fn app_key() -> Result<Vec<u8>> {
    let hex = std::env::var("APP_KEY").unwrap_or_default();
    if hex.trim().is_empty() {
        return Err(Error::Config(
            "APP_KEY is empty; run `arc key:generate` to write one into .env".to_string(),
        ));
    }
    decode_hex(hex.trim()).ok_or_else(|| {
        Error::Config(
            "APP_KEY is not 128 lowercase hex characters; run `arc key:generate` \
             to replace it"
                .to_string(),
        )
    })
}

/// Decode an even-length hex string into bytes, or `None` if it is malformed.
fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}
