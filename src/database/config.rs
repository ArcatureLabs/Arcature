//! Database configuration with credential redaction.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use super::ConnectOptions;
#[cfg(feature = "db-postgres")]
use sqlx::postgres::PgSslMode;

/// Resolved database configuration for the one connection pool.
///
/// `Debug` and `Display` never expose the username, password, or the full
/// connection URL.
#[derive(Clone)]
pub struct DatabaseConfig {
    connect_options: ConnectOptions,
    pool: PoolConfig,
    session: SessionConfig,
    application_name: Option<String>,
}

/// Pool sizing and timeout configuration.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    max_connections: u32,
    min_connections: u32,
    acquire_timeout: Duration,
    idle_timeout: Option<Duration>,
    max_lifetime: Option<Duration>,
}

/// Session-level PostgreSQL timeout configuration.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    statement_timeout: Option<Duration>,
    lock_timeout: Option<Duration>,
    idle_in_transaction_session_timeout: Option<Duration>,
}

impl PoolConfig {
    /// Production-oriented defaults: 10 max connections, 0 min, 10s acquire,
    /// 10min idle, 30min lifetime.
    pub fn new() -> Self {
        Self {
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(10),
            idle_timeout: Some(Duration::from_secs(600)),
            max_lifetime: Some(Duration::from_secs(1800)),
        }
    }

    pub fn max_connections(mut self, max: u32) -> Self {
        self.max_connections = max;
        self
    }
    pub fn min_connections(mut self, min: u32) -> Self {
        self.min_connections = min;
        self
    }
    pub fn acquire_timeout(mut self, timeout: Duration) -> Self {
        self.acquire_timeout = timeout;
        self
    }
    pub fn idle_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.idle_timeout = timeout;
        self
    }
    pub fn max_lifetime(mut self, lifetime: Option<Duration>) -> Self {
        self.max_lifetime = lifetime;
        self
    }

    pub(crate) fn get_max_connections(&self) -> u32 {
        self.max_connections
    }
    pub(crate) fn get_min_connections(&self) -> u32 {
        self.min_connections
    }
    pub(crate) fn get_acquire_timeout(&self) -> Duration {
        self.acquire_timeout
    }
    pub(crate) fn get_idle_timeout(&self) -> Option<Duration> {
        self.idle_timeout
    }
    pub(crate) fn get_max_lifetime(&self) -> Option<Duration> {
        self.max_lifetime
    }
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionConfig {
    /// Production-oriented defaults: 30s statement, 10s lock, 60s idle-in-tx.
    pub fn new() -> Self {
        Self {
            statement_timeout: Some(Duration::from_secs(30)),
            lock_timeout: Some(Duration::from_secs(10)),
            idle_in_transaction_session_timeout: Some(Duration::from_secs(60)),
        }
    }

    /// Disable all session timeouts (for migrations or batch jobs).
    pub fn none() -> Self {
        Self {
            statement_timeout: None,
            lock_timeout: None,
            idle_in_transaction_session_timeout: None,
        }
    }

    pub fn statement_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.statement_timeout = timeout;
        self
    }
    pub fn lock_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.lock_timeout = timeout;
        self
    }
    pub fn idle_in_transaction_session_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.idle_in_transaction_session_timeout = timeout;
        self
    }

    #[cfg_attr(not(feature = "db-postgres"), allow(dead_code))]
    pub(crate) fn set_statement(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = self.statement_timeout {
            parts.push(format!("SET statement_timeout = {}", t.as_millis()));
        }
        if let Some(t) = self.lock_timeout {
            parts.push(format!("SET lock_timeout = {}", t.as_millis()));
        }
        if let Some(t) = self.idle_in_transaction_session_timeout {
            parts.push(format!(
                "SET idle_in_transaction_session_timeout = {}",
                t.as_millis()
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl DatabaseConfig {
    /// Parse a PostgreSQL connection URL into resolved configuration.
    pub fn new(database_url: &str) -> Result<Self, crate::Error> {
        let connect_options = ConnectOptions::from_str(database_url)
            .map_err(|e| crate::Error::Config(format!("invalid database URL: {e}")))?;
        Ok(Self {
            connect_options,
            pool: PoolConfig::new(),
            session: SessionConfig::new(),
            application_name: None,
        })
    }

    pub fn pool(mut self, pool: PoolConfig) -> Self {
        self.pool = pool;
        self
    }

    pub fn session(mut self, session: SessionConfig) -> Self {
        self.session = session;
        self
    }

    /// Set the PostgreSQL `application_name` for connection identification.
    pub fn application_name(mut self, name: impl Into<String>) -> Self {
        self.application_name = Some(name.into());
        self
    }

    pub(crate) fn connect_options(&self) -> ConnectOptions {
        // `application_name` is a PostgreSQL startup parameter; the other
        // drivers have no equivalent knob, so it is simply carried and
        // reported rather than applied.
        let options = self.connect_options.clone();
        #[cfg(feature = "db-postgres")]
        let options = match &self.application_name {
            Some(name) => options.application_name(name),
            None => options,
        };
        options
    }

    pub(crate) fn pool_config(&self) -> &PoolConfig {
        &self.pool
    }

    #[cfg_attr(not(feature = "db-postgres"), allow(dead_code))]
    pub(crate) fn session_config(&self) -> &SessionConfig {
        &self.session
    }

    pub(crate) fn validate(&self) -> Result<(), crate::Error> {
        if self.pool.get_max_connections() == 0 {
            return Err(crate::Error::Config("max_connections must be > 0".into()));
        }
        if self.pool.get_min_connections() > self.pool.get_max_connections() {
            return Err(crate::Error::Config(
                "min_connections must be <= max_connections".into(),
            ));
        }
        if self.pool.get_acquire_timeout().is_zero() {
            return Err(crate::Error::Config("acquire_timeout must be > 0".into()));
        }
        Ok(())
    }
}

/// Manual `Debug` -- redacts credentials.
///
/// The identifying fields differ per driver (a SQLite database is a file, not
/// a host and port), so the field set is chosen per driver rather than
/// flattened to a lowest common denominator that would print `""` for SQLite.
impl fmt::Debug for DatabaseConfig {
    #[cfg(feature = "db-postgres")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.connect_options.get_host())
            .field("port", &self.connect_options.get_port())
            .field("database", &self.connect_options.get_database())
            .field(
                "ssl_mode",
                &ssl_mode_name(self.connect_options.get_ssl_mode()),
            )
            .field("pool", &self.pool)
            .field("session", &self.session)
            .field("application_name", &self.application_name)
            .finish()
    }

    #[cfg(feature = "db-sqlite")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("filename", &self.connect_options.get_filename())
            .field("pool", &self.pool)
            .field("session", &self.session)
            .field("application_name", &self.application_name)
            .finish()
    }

    #[cfg(feature = "db-mysql")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("host", &self.connect_options.get_host())
            .field("port", &self.connect_options.get_port())
            .field("database", &self.connect_options.get_database())
            .field("pool", &self.pool)
            .field("session", &self.session)
            .field("application_name", &self.application_name)
            .finish()
    }
}

#[cfg(feature = "db-postgres")]
fn ssl_mode_name(mode: PgSslMode) -> &'static str {
    match mode {
        PgSslMode::Disable => "disable",
        PgSslMode::Allow => "allow",
        PgSslMode::Prefer => "prefer",
        PgSslMode::Require => "require",
        PgSslMode::VerifyCa => "verify-ca",
        PgSslMode::VerifyFull => "verify-full",
    }
}

#[cfg(all(test, feature = "db-postgres"))]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_url() {
        let config = DatabaseConfig::new("postgres://user:pass@localhost:5432/mydb").unwrap();
        assert_eq!(config.connect_options().get_host(), "localhost");
        assert_eq!(config.connect_options().get_port(), 5432);
    }

    #[test]
    fn debug_redacts_credentials() {
        let config =
            DatabaseConfig::new("postgres://secret_user:secret_pass@localhost:5432/mydb").unwrap();
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret_user"));
        assert!(!debug.contains("secret_pass"));
        assert!(debug.contains("localhost"));
    }

    #[test]
    fn pool_defaults_sensible() {
        let p = PoolConfig::new();
        assert_eq!(p.get_max_connections(), 10);
        assert_eq!(p.get_min_connections(), 0);
    }

    #[test]
    fn session_set_statement() {
        let s = SessionConfig::new();
        let stmt = s.set_statement().unwrap();
        assert!(stmt.contains("statement_timeout = 30000"));
        assert!(stmt.contains("lock_timeout = 10000"));
    }
}
