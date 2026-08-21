//! Native Inertia.js server protocol.
//!
//! Arcature implements the server side of the Inertia v3 protocol so a stock
//! official `@inertiajs/react` or `@inertiajs/vue3` client talks to an Axum
//! application without knowing Arcature exists.
//!
//! The mental model is simple: the browser's official Inertia client makes
//! HTTP requests; Arcature's native Inertia implementation renders the
//! initial HTML page (with the embedded page object) on the first visit and
//! the Inertia JSON page object on subsequent visits.
//!
//! ```ignore
//! use arcature::prelude::*;
//!
//! pub async fn index(inertia: Inertia) -> Result<Response> {
//!     inertia.render("users/index", serde_json::json!({ "users": [] })).await
//! }
//! ```
//!
//! Or with the `inertia!()` macro for the directive's target syntax:
//!
//! ```ignore
//! pub async fn index(inertia: Inertia) -> Result<Response> {
//!     inertia!("users/index", { users })
//! }
//! ```

pub mod config;
pub mod contracts;
pub mod error;
pub mod headers;
pub mod page;
pub mod pending;
pub mod props;
pub mod redirect;
pub mod render;
pub mod request;
pub mod response;

pub use config::{
    AssetVersion, InertiaConfig, RootDocument, ScriptBody, default_root_document,
    vite_root_document,
};
pub use contracts::{
    ClientData, ContractArtifact, ContractError, ContractType, PageContract, PageContractEntry,
    PageContracts, PageProps, PageSchema, PageType, PropSchema, PropsSchema,
};
pub use error::InertiaError;
pub use page::{Component, PageIdentifier, PageOptions, ScrollProp};
pub use pending::PendingPage;
pub use props::{
    MergeStrategy, OnceProp, Prop, Props, SharedProps, always, deep_merge, deferred,
    deferred_group, eager, infinite_scroll, lazy, match_on, merge, merge_path, once, once_with,
    optional, prepend, rescue,
};
pub use redirect::{Redirect, external, fragment, redirect};
pub use render::{Inertia, InertiaLayer};
pub use request::{InertiaRequest, MergeIntent, PartialSelection};

// The `inertia!()` macro is exported at the crate root via `#[macro_export]`.
// See `lib.rs` for the re-export path `arcature::inertia!`.

/// The `inertia!()` macro renders an Inertia page with named props.
///
/// This is the directive's target syntax. It serializes the given props to a
/// JSON object and renders the page through an [`Inertia`] extractor named
/// `inertia` that must be in scope (a handler parameter). On a first visit it
/// returns the initial HTML; on an Inertia visit it returns the JSON page
/// object.
///
/// ```ignore
/// pub async fn index(inertia: Inertia, db: Db) -> Result<Response> {
///     let users = User::all(&db).await?;
///     inertia!("users/index", { users })
/// }
/// ```
#[macro_export]
macro_rules! inertia {
    ($component:expr, { $($name:ident),* $(,)? }) => {
        $crate::inertia::Inertia::render(
            &inertia,
            $component,
            $crate::serde_json::json!({ $(stringify!($name): $name,)* }),
        )
            .await
            .map(|r| r)
            .map_err($crate::Error::from)
    };
}
