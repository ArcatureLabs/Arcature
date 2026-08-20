//! Application configuration: read environment variables and build typed
//! configs for each subsystem.
//!
//! `load()` reads `.env` (loaded by `bootstrap::app`) and the process
//! environment, returning a [`Config`] the bootstrap layer feeds into the
//! `Application` builder. Override any value via the env vars below; see
//! `.env.example` for the full list.

use arcature::prelude::*;

/// The resolved application configuration.
pub struct Config {
    /// The bind address (env `APP_BIND`, default `127.0.0.1`).
    pub bind_addr: String,
    /// The bind port (env `APP_PORT`, default `3000`).
    pub port: u16,
    /// The database config (env `DATABASE_URL`).
    pub database: DatabaseConfig,
    /// The cache config (env `REDIS_URL`).
    pub cache: CacheConfig,
    /// The storage config (env `STORAGE_ROOT`, default `./storage`).
    pub storage: StorageConfig,
    /// The mail config (env `MAIL_URL`).
    pub mail: SmtpConfig,
}

/// Load configuration from the environment.
///
/// Reads `.env` first (the bootstrap layer calls `dotenvy::dotenv` before
/// this), then the process environment. Every value has a default so the app
/// boots without any configuration; override via the env vars documented on
/// [`Config`].
pub fn load() -> Result<Config> {
    Ok(Config {
        bind_addr: env_or("APP_BIND", "127.0.0.1"),
        port: arcature::config::env_parsed("APP_PORT", 3000),
        database: DatabaseConfig::new(&env_or(
            "DATABASE_URL",
            "postgres://postgres:postgres@localhost:5432/__RUST_NAME__",
        ))?,
        // `CacheConfigError` has no `From<...> for Error` impl; fold it into
        // the framework `Error::Config` variant.
        cache: CacheConfig::new(&env_or("REDIS_URL", "redis://127.0.0.1:6379/0"))
            .map_err(|e| Error::Config(e.to_string()))?,
        storage: StorageConfig::fs(&env_or("STORAGE_ROOT", "./storage"))
            .map_err(|e| Error::Config(e.to_string()))?,
        mail: SmtpConfig::from_url(&env_or("MAIL_URL", "smtp://127.0.0.1:25"))?,
    })
}
