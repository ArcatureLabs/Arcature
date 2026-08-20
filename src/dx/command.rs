//! Typed application commands and the command registry.
//!
//! Application commands are typed Rust functions invoked from the CLI via
//! `arc run <name>`. A command is
//! `#[command("users:prune")] pub async fn prune_users(...) -> Result<()>`.
//! The macro generates a `CommandBinding` for inspection and passes the
//! function through unchanged -- the application registers commands
//! explicitly with `CommandRegistry::register` at startup.
//!
//! This mirrors the `Dispatcher` design: type-erased handlers behind
//! `serde_json::Value`, no `TypeId`/`Any`, explicit registration, no hidden
//! dispatch. The difference is that commands take **no payload** (the CLI
//! passes only the command name) and resolve their dependencies from
//! application state `S` via `Resolve<S>`.
//!
//! The metadata binding ([`CommandBinding`]) lives in
//! [`crate::dx::graph`] alongside the other module-graph bindings.
//!
//! # Registration
//!
//! Registration is explicit. The application calls `CommandRegistry::register`
//! at startup:
//!
//! ```ignore
//! let mut commands = CommandRegistry::new();
//! commands.register("users:prune", move |state: AppState| {
//!     let users = Users::resolve(&state);
//!     async move { users.prune_inactive().await }
//! });
//! ```
//!
//! The `module!` macro's `commands:` section is metadata for `arc check`
//! inspection -- it does NOT register commands at runtime.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed future returned by a type-erased command handler.
type BoxFuture = Pin<Box<dyn Future<Output = Result<(), CommandError>> + Send>>;

/// A type-erased command handler: takes the application state and returns
/// a future. The handler closes over the real `Resolve<S>` construction
/// logic (captured at registration time).
type ErasedCommand<S> = Arc<dyn Fn(&S) -> BoxFuture + Send + Sync + 'static>;

/// The marker trait for typed Arcature commands.
///
/// A command is a typed async function that the CLI invokes by name. The
/// `#[command("name")]` macro generates `impl Command` and a
/// `CommandBinding` for inspection, and passes the function through
/// unchanged so the application registers it explicitly.
pub trait Command: Send + Sync + 'static {
    /// The static command name used for CLI invocation and registry lookup
    /// (e.g. `"users:prune"`).
    const NAME: &'static str;
}

/// A typed error from command execution.
///
/// No raw `String` errors. Each variant is a failure that can actually
/// happen -- no "future-proof" variants.
#[derive(Debug)]
pub enum CommandError {
    /// No command was registered with the requested name.
    NotFound(String),
    /// The command handler returned an error. The string is the handler's
    /// error message (the command decides what to expose -- Arcature does
    /// not leak internal details).
    Failed(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "command `{name}` is not registered"),
            Self::Failed(msg) => write!(f, "command failed: {msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// The typed application command registry.
///
/// Holds type-erased command handlers keyed by command name. The
/// application registers handlers at startup and the CLI (`arc run`)
/// calls `run` by name. There is no hidden dispatch -- the caller decides
/// when and where to run a command.
///
/// # Type erasure
///
/// The registry type-erases handlers behind `Arc<dyn Fn(&S) -> BoxFuture>`
/// where `S` is the application state type. Each handler closes over the
/// `Resolve<S>` construction of its dependencies. This avoids `TypeId`/`Any`
/// and keeps the dispatch path fully typed at the registration site.
///
/// # Clone
///
/// `CommandRegistry<S>` is `Clone` -- it holds an `Arc` internally. Clone
/// is cheap and safe for sharing across tasks (the handler map is behind
/// an `Arc`, not a `Mutex` -- the map is frozen after registration).
pub struct CommandRegistry<S: Send + Sync + 'static> {
    /// The handler map, keyed by command name. Frozen after registration.
    handlers: Arc<HashMap<String, ErasedCommand<S>>>,
}

impl<S: Send + Sync + 'static> Default for CommandRegistry<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: Send + Sync + 'static> CommandRegistry<S> {
    /// Create a new empty command registry (no handlers).
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Arc::new(HashMap::new()),
        }
    }

    /// Register a command handler under a name.
    ///
    /// The handler is `Fn(&S) -> Fut` where `Fut: Future<Output =
    /// Result<(), CommandError>> + Send`. The application state `S` is
    /// passed by reference so the handler can resolve its dependencies
    /// via `Resolve<S>`.
    ///
    /// Registering a name that already exists overwrites the previous
    /// handler -- this is a configuration error the application should
    /// avoid, but the registry does not panic (the last registration wins,
    /// matching the `Dispatcher` rebuild-on-register pattern).
    #[allow(clippy::needless_pass_by_value)]
    pub fn register<F, Fut>(self, name: &str, handler: F) -> Self
    where
        F: Fn(&S) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), CommandError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: ErasedCommand<S> = Arc::new(move |state: &S| Box::pin(handler(state)));

        let mut map = (*self.handlers).clone();
        map.insert(name.to_string(), erased);
        Self {
            handlers: Arc::new(map),
        }
    }

    /// Returns `true` if a command with the given name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Returns the number of registered commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns `true` if no commands are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// Returns the sorted list of registered command names, for CLI
    /// listing (`arc run` with no name, or `arc check`).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.handlers.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// Run a registered command by name, passing the application state.
    ///
    /// Returns `Err(CommandError::NotFound)` if no command is registered
    /// under `name`. Otherwise awaits the handler and returns its result.
    pub async fn run(&self, name: &str, state: &S) -> Result<(), CommandError> {
        match self.handlers.get(name) {
            Some(handler) => handler(state).await,
            None => Err(CommandError::NotFound(name.to_string())),
        }
    }
}

impl<S: Send + Sync + 'static> std::fmt::Debug for CommandRegistry<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandRegistry")
            .field("len", &self.handlers.len())
            .field("names", &self.names())
            .finish_non_exhaustive()
    }
}

impl<S: Send + Sync + 'static> Clone for CommandRegistry<S> {
    fn clone(&self) -> Self {
        Self {
            handlers: Arc::clone(&self.handlers),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct DummyState;

    #[tokio::test]
    async fn register_and_run_command() {
        let registry = CommandRegistry::new().register("greet", |_state: &DummyState| async {
            Ok::<_, CommandError>(())
        });
        assert!(registry.contains("greet"));
        let result = registry.run("greet", &DummyState).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_unknown_command_is_not_found() {
        let registry = CommandRegistry::<DummyState>::new();
        let err = registry.run("missing", &DummyState).await.unwrap_err();
        assert!(matches!(err, CommandError::NotFound(_)));
    }

    #[test]
    fn names_are_sorted() {
        let registry = CommandRegistry::<DummyState>::new()
            .register("zeta", |_| async { Ok(()) })
            .register("alpha", |_| async { Ok(()) })
            .register("middle", |_| async { Ok(()) });
        assert_eq!(registry.names(), vec!["alpha", "middle", "zeta"]);
    }
}
