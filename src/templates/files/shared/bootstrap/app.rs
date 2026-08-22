//! Assemble the [`Application`] from configuration.
//!
//! [`app`] is the composition root: it loads `.env`, reads the typed
//! [`Config`](crate::config::Config), and wires every enabled subsystem into
//! the `Application` builder. Layer *order* is not decided here -- the
//! framework pipeline imposes it -- so these calls read as a list of
//! decisions rather than as a sequence.

use std::sync::Arc;
use std::time::Duration;

use arcature::assets::{Assets, AssetsConfig};
use arcature::auth::{CsrfConfig, DbSessionStore, PasswordConfig, PasswordHasher, SessionConfig};
use arcature::http::{ErrorMapping, SecurityHeaders};
use arcature::inertia::{InertiaConfig, vite_root_document};
use arcature::jobs::Scheduler;
use arcature::prelude::*;

/// The Vite entry point. Must match `build.rollupOptions.input` in
/// `vite.config.ts`, because that string is the manifest key.
const JS_ENTRY: &str = "__JS_ENTRY__";

/// The assembled application plus the state closure that feeds it.
///
/// Returned together because the hasher is built once here and belongs in
/// both halves: the builder needs nothing from it, the state does.
pub struct Bootstrapped {
    /// The wired application.
    pub application: Application<crate::bootstrap::AppState>,
    /// The closure `run_with_state` calls once startup has finished.
    pub state_fn: Arc<dyn Fn(&Resources, &Lifecycle) -> crate::bootstrap::AppState + Send + Sync>,
}

/// How the process was asked to run.
///
/// The scheduler is a boot-time decision rather than a runtime one: it is a
/// builder input, and a process that both serves and fires cron would run one
/// copy of every scheduled job per replica.
#[derive(Clone, Copy, Debug, Default)]
pub struct BootOptions {
    /// Install the scheduler alongside the HTTP server.
    pub scheduler: bool,
}

/// Build the fully-wired application.
///
/// # Errors
///
/// Returns [`Error::Config`] when the environment is incomplete (a missing
/// `APP_KEY`, an unparseable URL) or when the asset manifest is absent in a
/// production build.
pub fn app(options: BootOptions) -> Result<Bootstrapped> {
    dotenvy::dotenv().ok();
    let config = crate::config::load()?;

    let hasher = Arc::new(
        PasswordHasher::new(PasswordConfig::default()).map_err(|e| Error::Config(e.to_string()))?,
    );

    // Resolved once, at startup: in development this yields source paths for
    // Vite to serve over the one port, in production the hashed names from
    // `public/build/.vite/manifest.json`.
    let assets_config = AssetsConfig::new();
    let assets = Assets::detect(&assets_config).map_err(|e| Error::Config(e.to_string()))?;

    let inertia = InertiaConfig::new(
        env!("CARGO_PKG_VERSION"),
        vite_root_document(&config.app_name, &assets, JS_ENTRY),
    )
    .map_err(|e| Error::Config(e.to_string()))?;

    // Two policies, not a default with an override. `SessionConfig::new` names
    // the cookie `__Host-id`, and the `__Host-` prefix *mandates* `Secure`
    // (RFC 6265bis) -- so `new().with_secure(false)` is not a relaxed
    // production policy, it is a config error, and it is the one that stopped
    // a freshly scaffolded application from starting. `dev` is the coherent
    // plain-HTTP policy: cookie `arcature-id`, no prefix, no `Secure`.
    //
    // The switch is `config.production`, which is `!cfg!(debug_assertions)` --
    // fixed when the binary is compiled, not read from `APP_ENV`. A cookie
    // policy that an environment variable could downgrade would not be a
    // policy.
    let mut session = if config.production {
        SessionConfig::new(&config.app_key)
    } else {
        SessionConfig::dev(&config.app_key)
    }
    .map_err(|e| Error::Config(e.to_string()))?;
    session = session.with_max_age(Duration::from_secs(60 * 60 * 8));

    // Sessions go in the application's own database, in `arcature_sessions`.
    // The alternative -- a process-local `HashMap` -- makes every deploy a
    // mass logout and makes a second replica a coin flip on every request,
    // which is a property to discover before production rather than during
    // it.
    //
    // Built here, before `.database(..)` takes the config, because the
    // builder wants the store while the application is still being described
    // and there is no pool yet. Nothing connects on this line: the pool is
    // lazy and opens its first connection when the first request needs one.
    //
    // The table is created by `--migrate`, not on boot. See `Mode` in
    // `src/lib.rs`: a deploy that migrates as a side effect of starting has
    // every replica racing to do it.
    let sessions = DbSessionStore::connect_lazy(&config.database);

    let state_fn = crate::bootstrap::state_fn(&config, hasher);

    let mut builder = Application::new()
        .routes(crate::routes::app_routes())
        .bind(&config.bind_addr)
        .port(config.port)
        .database(config.database)
        .storage(config.storage)
        .mail(config.mail)
        .jobs(jobs_registry())
        .compression()
        .security_headers(SecurityHeaders::new())
        .request_id()
        .access_log()
        .catch_panic()
        .error_mapping(ErrorMapping::new())
        .body_limit(2 * 1024 * 1024)
        .timeout(Duration::from_secs(30))
        .inertia(inertia)
        .session(session, sessions)
        .map_err(|e| Error::Config(e.to_string()))?
        // The axios-compatible preset: cookie `XSRF-TOKEN`, header
        // `X-XSRF-TOKEN`, readable by JavaScript. Inertia's client reads the
        // cookie and echoes it back, which a `HttpOnly` cookie would prevent.
        //
        // `Secure` follows the same rule as the session cookie above, for the
        // same reason and with one extra consequence: a `Secure` cookie is
        // never stored by a browser talking plain HTTP, so leaving it on in
        // development does not make development safer -- it makes every form
        // post fail its CSRF check with no cookie to blame. The preset keeps
        // `SameSite=Lax`, which is the part actually doing the work here.
        .csrf(
            CsrfConfig::inertia()
                .with_secure(config.production)
                .map_err(|e| Error::Config(e.to_string()))?,
        )
        .static_files(&assets_config)
        .health(true)
        // The shapes `arc typegen` turns into `pages.d.ts`. Carried as a
        // request extension so the graph endpoint below and the page
        // machinery read the same artifact.
        .page_contracts(crate::app::page_contracts())
        .layer(arcature::from_fn(
            crate::bootstrap::error_pages::error_pages,
        ));

    // Wired only when `REDIS_URL` names one. Redis is a separate server, and
    // startup connects to whatever it is handed, so an unconditional
    // `.cache(..)` with a localhost default would make "is redis-server
    // running?" the first question a freshly scaffolded application asks --
    // and it asks it by refusing to boot. Absent is a coherent state: the
    // resource is simply not registered, and `Inject<Cache>` says so.
    if let Some(cache) = config.cache {
        builder = builder.cache(cache);
    }

    // `GET /_arcature/uag.json`, which is how `arc dev` regenerates the
    // TypeScript after every restart without building anything extra. Three
    // things have to agree before it answers: the `dev` feature (which turns
    // on the framework's graph module), this call, and a debug build. A
    // release binary cannot serve it however it is invoked.
    #[cfg(feature = "dev")]
    {
        builder = builder.uag_endpoint(crate::app::graph());
    }

    if options.scheduler {
        builder = builder.scheduler(schedule());
    }

    Ok(Bootstrapped {
        application: builder.build(),
        state_fn,
    })
}

/// The cron table.
///
/// Add one `scheduler.schedule(&SOME_BINDING, || ...)` per cadence. Empty by
/// default, and an empty scheduler is a scheduler: it starts, fires nothing,
/// and stops cleanly.
fn schedule() -> Scheduler {
    Scheduler::new()
}

/// Register job handlers.
///
/// Add one `registry.add(&MyJob::JOB, |job| async move { ... })` line per job
/// kind. Empty by default; the worker still runs and the scheduler (if any)
/// still enqueues, but unknown kinds are logged and dropped.
fn jobs_registry() -> Registry {
    let mut registry = Registry::new();
    // registry.add(&MyJob::JOB, |job| async move {
    //     // ...run the job...
    //     Ok(())
    // }).expect("duplicate job registration");
    let _ = &mut registry;
    registry
}
