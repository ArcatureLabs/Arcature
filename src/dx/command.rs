//! Typed application commands and the command registry.
//!
//! Application commands are typed Rust functions dispatched by name
//! through [`CommandRegistry::run`]. A command is
//! `#[command("users:prune")] pub async fn prune_users(...) -> Result<()>`.
//!
//! The macro emits the function unchanged, a [`CommandBinding`] for
//! inspection, and a zero-sized *command type* carrying the
//! [`Command`] impl. The command type exists because an attribute macro on
//! a function has nothing else to hang a trait impl on -- a function item
//! type cannot be named -- and without a nameable type there is no way to
//! hand the command to the registry by type. `prune_users` gets
//! `PruneUsersCommand`.
//!
//! This mirrors the `Dispatcher` design: type-erased handlers, explicit
//! registration, no hidden dispatch. The difference is that commands take
//! **no payload** (the CLI passes only the command name) and resolve their
//! dependencies from application state `S` via `Resolve<S>`.
//!
//! The metadata binding ([`CommandBinding`]) lives in
//! [`crate::dx::graph`] alongside the other module-graph bindings.
//!
//! # Registration
//!
//! Registration stays explicit -- nothing scans the binary for commands --
//! but it is one line per command, by type:
//!
//! ```ignore
//! let commands = CommandRegistry::<AppState>::new()
//!     .register_command::<PruneUsersCommand>()
//!     .register_command::<ReindexCommand>();
//!
//! commands.run("users:prune", &state).await?;
//! ```
//!
//! [`CommandRegistry::register`] remains for handlers that are not
//! `#[command]` functions (a closure built at startup, say).
//!
//! The `module!` macro's `commands:` section is metadata for the Unified
//! Application Graph -- it does NOT register commands at runtime.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// The future a [`Command::run`] hands back once its dependencies have been
/// resolved from application state.
///
/// It is boxed rather than an `impl Future` return, because `run` takes
/// `&S` but the registry stores handlers for the life of the process: the
/// future must not borrow the state it was built from. Boxing forces the
/// split -- resolve eagerly from `&S`, then return an owned `'static`
/// future -- which is exactly the shape the registry already erases to, so
/// the box costs nothing extra on the dispatch path.
pub type CommandFuture = Pin<Box<dyn Future<Output = Result<(), CommandError>> + Send + 'static>>;

/// A type-erased command handler: takes the application state and returns
/// a future. The handler closes over the real `Resolve<S>` construction
/// logic (captured at registration time).
type ErasedCommand<S> = Arc<dyn Fn(&S) -> CommandFuture + Send + Sync + 'static>;

/// A typed Arcature command: a name plus the code that runs it against
/// application state `S`.
///
/// Implemented by the command type the `#[command("name")]` macro
/// generates. The generated `run` resolves each of the annotated function's
/// parameters through `Resolve<S>`, calls it, and maps its error to
/// [`CommandError::Failed`] -- so the trait carries no business behavior of
/// its own, only the wiring the CLI needs.
///
/// The trait is generic over `S` rather than carrying an associated state
/// type, because one command function is usable from any state that can
/// resolve its dependencies -- including the reduced state a test builds.
pub trait Command<S>: Send + Sync + 'static {
    /// The static command name used for CLI invocation and registry lookup
    /// (e.g. `"users:prune"`).
    const NAME: &'static str;

    /// Resolve the command's dependencies from `state` and run it.
    ///
    /// Dependency resolution happens *before* the returned future is
    /// created, so the future owns everything it needs and can outlive the
    /// borrow of `state`.
    fn run(state: &S) -> CommandFuture;
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
/// application registers handlers at startup and calls `run` by name.
/// There is no hidden dispatch -- the caller decides when and where to run
/// a command.
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

    /// Register a `#[command]`-generated command type under its own
    /// [`Command::NAME`].
    ///
    /// This is what keeps the names aligned: the name in the attribute, the
    /// name in `module!`'s `commands:` section, and the name the registry
    /// answers to all come from the same `const`, so they cannot drift
    /// apart.
    #[must_use]
    pub fn register_command<C>(self) -> Self
    where
        C: Command<S>,
    {
        self.register(C::NAME, C::run)
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

    /// Returns the registered command names in sorted order, so a listing
    /// of them is stable.
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

    /// Stands in for what `#[command("dummy:run")]` generates: a zero-sized
    /// type whose `run` resolves dependencies and calls the function.
    struct DummyCommand;

    impl Command<DummyState> for DummyCommand {
        const NAME: &'static str = "dummy:run";

        fn run(_state: &DummyState) -> CommandFuture {
            Box::pin(async { Ok(()) })
        }
    }

    struct FailingCommand;

    impl Command<DummyState> for FailingCommand {
        const NAME: &'static str = "dummy:fail";

        fn run(_state: &DummyState) -> CommandFuture {
            Box::pin(async { Err(CommandError::Failed("boom".to_string())) })
        }
    }

    #[tokio::test]
    async fn a_registered_command_type_runs_under_its_own_name() {
        let registry = CommandRegistry::new().register_command::<DummyCommand>();
        assert!(registry.contains("dummy:run"));
        assert!(registry.run("dummy:run", &DummyState).await.is_ok());
    }

    #[tokio::test]
    async fn a_command_type_that_fails_surfaces_its_message() {
        let registry = CommandRegistry::new().register_command::<FailingCommand>();
        let err = registry.run("dummy:fail", &DummyState).await.unwrap_err();
        assert_eq!(err.to_string(), "command failed: boom");
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
