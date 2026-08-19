//! In-process typed event dispatch.
//!
//! Listeners are closures `Fn(E) -> Future<Output = Result<(), DispatchError>>`
//! registered via [`Dispatcher::register`]. Dispatch is sequential in
//! registration order; a listener failure is logged and does not stop other
//! listeners. Type erasure is via `serde_json::Value` (serialize once at
//! dispatch, deserialize per listener); the crate avoids `TypeId`/`Any`.
//!
//! # Example
//!
//! ```ignore
//! use arcature::events::{Event, Dispatcher, DispatchError};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Clone, Serialize, Deserialize, Event)]
//! pub struct UserRegistered {
//!     pub user_id: u64,
//!     pub email: String,
//! }
//!
//! let dispatcher = Dispatcher::new()
//!     .register(|event: UserRegistered| async move {
//!         println!("welcome {}", event.email);
//!         Ok(())
//!     });
//!
//! dispatcher.dispatch(&UserRegistered { user_id: 1, email: "a@b.com".into() }).await.ok();
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::de::DeserializeOwned;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Event trait
// ---------------------------------------------------------------------------

/// The marker trait for typed Arcature events.
///
/// An event type must have a static [`NAME`](crate::DxComponent::NAME) used
/// for dispatch lookup, and must be `Serialize + DeserializeOwned` (the user
/// adds `#[derive(Serialize, Deserialize)]`). The `#[derive(Event)]` macro
/// generates `impl DxComponent` (with `NAME = stringify!(StructName)`) and
/// the empty `impl Event`.
pub trait Event: crate::DxComponent + Send + Sync + 'static {}

// ---------------------------------------------------------------------------
// DispatchError
// ---------------------------------------------------------------------------

/// A typed error from event dispatch.
///
/// The `Deserialize` variant carries no message: serde errors may echo the
/// payload, so the message is dropped to avoid information disclosure.
#[derive(Debug)]
pub enum DispatchError {
    /// The event could not be serialized for type-erased dispatch.
    /// The string is the serde error message, not the event payload.
    Serialize(String),
    /// The event payload could not be deserialized by a listener.
    /// No message is included (serde errors may echo the payload).
    Deserialize,
    /// A listener returned an error. The string is the listener's error
    /// message; the listener decides what to expose.
    Listener(String),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(msg) => write!(f, "event serialization failed: {msg}"),
            Self::Deserialize => write!(f, "event payload did not deserialize by listener"),
            Self::Listener(msg) => write!(f, "listener error: {msg}"),
        }
    }
}

impl std::error::Error for DispatchError {}

// ---------------------------------------------------------------------------
// Type aliases for erased listeners
// ---------------------------------------------------------------------------

/// A boxed future returned by type-erased listeners.
type BoxFuture = Pin<Box<dyn Future<Output = Result<(), DispatchError>> + Send>>;

/// A type-erased listener: takes a serialized event and returns a future.
type ErasedListener = Arc<dyn Fn(serde_json::Value) -> BoxFuture + Send + Sync>;

// ---------------------------------------------------------------------------
// ListenerBinding — compile-time metadata for inspection.
// ---------------------------------------------------------------------------

/// A compile-time event-to-listener binding for `arc check` / `arc modules`
/// inspection. This is metadata only; it does not register the listener at
/// runtime. The application registers listeners explicitly via
/// [`Dispatcher::register`] at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerBinding {
    /// The event type name (e.g. `"UserRegistered"`).
    pub event: &'static str,
    /// The listener function name (e.g. `"send_welcome"`).
    pub listener: &'static str,
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// The typed event dispatcher.
///
/// Holds an `Arc`-frozen listener map (copy-on-write on `register`). Listeners
/// run sequentially in registration order; all listeners always run regardless
/// of failures; the first listener error is returned.
#[derive(Clone)]
pub struct Dispatcher {
    /// The listener map, keyed by event type name. Frozen after registration.
    listeners: Arc<HashMap<String, Vec<ErasedListener>>>,
    /// For testing: records dispatched event names. `None` in production.
    record: Option<Arc<Mutex<Vec<String>>>>,
}

impl Dispatcher {
    /// Create a new empty dispatcher (no listeners, no recording).
    #[must_use]
    pub fn new() -> Self {
        Self {
            listeners: Arc::new(HashMap::new()),
            record: None,
        }
    }

    /// Create a recording dispatcher for tests. Records dispatched event names
    /// so tests can assert [`was_dispatched`](Self::was_dispatched).
    #[must_use]
    pub fn recording() -> Self {
        Self {
            listeners: Arc::new(HashMap::new()),
            record: Some(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    /// Register a listener for an event type.
    ///
    /// The listener is `Fn(E) -> Fut` where `Fut: Future<Output =
    /// Result<(), DispatchError>> + Send`. The event type `E` must implement
    /// `Event + Serialize + DeserializeOwned`. Multiple listeners can be
    /// registered for the same event type; they run in registration order.
    #[allow(clippy::needless_pass_by_value)]
    pub fn register<E, F, Fut>(self, handler: F) -> Self
    where
        E: Event + Serialize + DeserializeOwned,
        F: Fn(E) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), DispatchError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: ErasedListener = Arc::new(move |event: serde_json::Value| {
            let handler = handler.clone();
            Box::pin(async move {
                let event: E =
                    serde_json::from_value(event).map_err(|_| DispatchError::Deserialize)?;
                handler(event).await
            })
        });

        let mut map = (*self.listeners).clone();
        map.entry(E::NAME.to_string()).or_default().push(erased);
        Self {
            listeners: Arc::new(map),
            record: self.record,
        }
    }

    /// Dispatch an event to all registered listeners.
    ///
    /// Listeners run sequentially in registration order. A listener failure is
    /// logged (stderr) and does NOT stop other listeners. Returns `Ok(())` if
    /// all listeners succeeded, or the first listener error if any failed
    /// (but all listeners still ran). If no listeners are registered, this is
    /// a no-op.
    pub async fn dispatch<E>(&self, event: &E) -> Result<(), DispatchError>
    where
        E: Event + Serialize,
    {
        // Record the event name if in recording mode.
        if let Some(record) = &self.record
            && let Ok(mut guard) = record.lock()
        {
            guard.push(E::NAME.to_string());
        }

        let listeners = self.listeners.get(E::NAME);
        if listeners.is_none_or(|l| l.is_empty()) {
            return Ok(());
        }

        let value =
            serde_json::to_value(event).map_err(|e| DispatchError::Serialize(e.to_string()))?;

        let listeners = listeners.expect("checked non-empty above");
        let mut first_error: Option<DispatchError> = None;

        for listener in listeners {
            match listener(value.clone()).await {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("event listener error for {}: {e}", E::NAME);
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }

        match first_error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Returns `true` if an event with the given type name was dispatched
    /// (recording mode only). In production mode, always returns `false`.
    #[must_use]
    pub fn was_dispatched(&self, name: &str) -> bool {
        self.record
            .as_ref()
            .map(|r| {
                r.lock()
                    .map(|guard| guard.iter().any(|n| n == name))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Returns the names of all dispatched events (recording mode only).
    #[must_use]
    pub fn dispatched_events(&self) -> Vec<String> {
        self.record
            .as_ref()
            .and_then(|r| r.lock().ok())
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// Returns the number of listeners registered for an event type.
    #[must_use]
    pub fn listener_count(&self, event_name: &str) -> usize {
        self.listeners.get(event_name).map_or(0, Vec::len)
    }
}

impl Default for Dispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("event_types", &self.listeners.len())
            .field("is_recording", &self.record.is_some())
            .finish_non_exhaustive()
    }
}
