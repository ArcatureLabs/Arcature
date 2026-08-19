//! The job handler registry.
//!
//! Handlers are closures `Fn(J) -> Future<Output = Result<(), JobError>>`
//! registered via [`Registry::add`]. Registration is typed (`J:
//! DeserializeOwned + Send + Sync + 'static`), dispatch is erased (`dyn
//! ErasedHandler`). A cloned `Registry` cheaply shares handlers via `Arc`.

use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use uuid::Uuid;

use super::config::JobModel;
use super::error::{JobError, RegisterError};
use super::validate::{validate_kind, validate_version};

/// A boxed future returned by type-erased handlers.
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The internal error from dispatching a handler (not public; the worker
/// translates it into dead/retry rows).
#[derive(Debug)]
pub(crate) enum HandlerError {
    /// The payload did not deserialize to the registered schema (poison job).
    Malformed,
    /// The handler ran and returned an error.
    Job(JobError),
}

/// The type-erased handler trait.
pub(crate) trait ErasedHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        job_id: Uuid,
    ) -> BoxFut<'a, Result<(), HandlerError>>;
}

/// A typed handler wrapping a closure.
struct TypedHandler<J> {
    handler: Arc<dyn Fn(J) -> BoxFut<'static, Result<(), JobError>> + Send + Sync>,
    _job: PhantomData<J>,
}

impl<J> ErasedHandler for TypedHandler<J>
where
    J: DeserializeOwned + Send + Sync + 'static,
{
    fn handle<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        job_id: Uuid,
    ) -> BoxFut<'a, Result<(), HandlerError>> {
        Box::pin(async move {
            // A payload that does not deserialize to the registered schema is a
            // poison job: never retry it forever. The serde error is
            // intentionally dropped (it can echo the offending payload, which
            // would leak payload content into the stored error).
            let job: J = serde_json::from_value(payload.clone())
                .map_err(|_| HandlerError::Malformed)?;
            let result = (self.handler)(job).await;
            // The job id is available for the observability seam; handlers do
            // not receive it by default (the payload is the contract).
            let _ = job_id;
            result.map_err(HandlerError::Job)
        })
    }
}

/// The registry of job handlers.
///
/// Built via [`Registry::add`] and passed to [`Worker::new`](super::Worker::new).
/// A second registration for the same `(kind, version)` is rejected so the
/// dispatch target is unambiguous.
#[derive(Clone)]
pub struct Registry {
    handlers: HashMap<(String, i16), Arc<dyn ErasedHandler + Send + Sync>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry")
            .field("handler_count", &self.handlers.len())
            .finish_non_exhaustive()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a typed handler for a [`JobModel`].
    ///
    /// The handler is `Fn(J) -> Fut` where `Fut: Future<Output =
    /// Result<(), JobError>> + Send`. A second registration for the same
    /// `(kind, version)` is rejected.
    pub fn add<J, F, Fut>(
        &mut self,
        model: &JobModel<J>,
        handler: F,
    ) -> Result<&mut Self, RegisterError>
    where
        J: DeserializeOwned + Send + Sync + 'static,
        F: Fn(J) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        validate_kind(model.kind()).map_err(RegisterError::invalid_kind)?;
        validate_version(model.version()).map_err(|_| RegisterError::invalid_version(model.version()))?;

        let key = (model.kind().to_string(), model.version());
        if self.handlers.contains_key(&key) {
            return Err(RegisterError::already_registered(model.kind(), model.version()));
        }

        let boxed: Arc<dyn Fn(J) -> BoxFut<'static, Result<(), JobError>> + Send + Sync> =
            Arc::new(move |job| Box::pin(handler(job)));
        let erased: Arc<dyn ErasedHandler + Send + Sync> = Arc::new(TypedHandler {
            handler: boxed,
            _job: PhantomData::<J>,
        });
        self.handlers.insert(key, erased);
        Ok(self)
    }

    /// Look up the handler for a kind and version.
    pub(crate) fn get(
        &self,
        kind: &str,
        version: i16,
    ) -> Option<Arc<dyn ErasedHandler + Send + Sync>> {
        self.handlers
            .get(&(kind.to_string(), version))
            .cloned()
    }

    /// Whether the registry has no handlers.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// The number of registered handlers.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Whether a handler is registered for this kind and version.
    pub fn handles(&self, kind: &str, version: i16) -> bool {
        self.handlers.contains_key(&(kind.to_string(), version))
    }
}
