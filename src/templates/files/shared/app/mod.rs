//! The application layer: controllers, models, pages, resources, requests,
//! services, and policies. One responsibility per subdirectory.
//!
//! Most of these directories start empty. That is deliberate -- `arc new`
//! lays out where code goes, and `arc make:controller`, `arc make:model`,
//! `arc make:page` and the rest fill them in.
//!
//! The `module!` block below is the machine-readable index of what this
//! layer contains. `arc build` and `arc typegen` read it, so a controller or
//! page that is not listed there is invisible to the tooling even though it
//! compiles. Add to the lists as you add files.

pub mod controllers;
pub mod models;
pub mod pages;
pub mod policies;
pub mod requests;
pub mod resources;
pub mod services;
pub mod views;

use arcature::prelude::*;

// Named here because `module!` emits bare identifiers for `controllers:`
// and reads the route descriptors through `APP_ROUTES`; both have to resolve
// at this invocation site.
use crate::app::controllers::HomeController;
use crate::routes::APP_ROUTES;

module! {
    pub Web {
        controllers: [HomeController],
        routes: APP_ROUTES,
        pages: [
            pages::HomePage,
            pages::NotFoundPage,
            pages::PageExpiredPage,
            pages::ServerErrorPage,
        ],
    }
}

/// The page contracts this application exposes to the client.
///
/// `module!` records page *names* and nothing else, because a name is all the
/// dependency graph needs. `arc typegen` needs the shapes, so they are
/// registered here, from the same list. Adding a page means adding it twice:
/// once above so the graph knows it exists, once here so its props reach
/// `pages.d.ts`.
///
/// # Panics
///
/// Panics if two pages declare the same name. That is a duplicate
/// registration in the list below, not a runtime condition, so it fails at
/// boot rather than producing a half-populated artifact.
#[must_use]
pub fn page_contracts() -> arcature::inertia::contracts::ContractArtifact {
    arcature::inertia::contracts::PageContracts::new()
        .register_entry(&pages::HomePage::PAGE_CONTRACT_ENTRY)
        .and_then(|c| c.register_entry(&pages::NotFoundPage::PAGE_CONTRACT_ENTRY))
        .and_then(|c| c.register_entry(&pages::PageExpiredPage::PAGE_CONTRACT_ENTRY))
        .and_then(|c| c.register_entry(&pages::ServerErrorPage::PAGE_CONTRACT_ENTRY))
        .expect("two pages in this application share a name")
        .artifact()
}

/// The application graph: every module this crate declares.
///
/// Read by the dev-only `/_arcature/uag.json` endpoint and by the `uag`
/// binary, which is how `arc typegen`, `arc routes` and `arc build` see the
/// application without linking it into the tool.
///
/// # Panics
///
/// Panics if a module imports something no module exports, or if two modules
/// share a name. Both are wiring mistakes in the `module!` blocks above and
/// are the same on every run, so failing at boot is the honest answer.
#[must_use]
pub fn graph() -> ApplicationGraph {
    ApplicationGraph::new(vec![web_module().clone()]).expect("the module graph does not resolve")
}
