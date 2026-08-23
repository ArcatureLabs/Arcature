//! Feature modules -- one directory per feature, written by
//! `arc make:module`.
//!
//! The rest of `app/` is laid out by kind: every controller in
//! `app/controllers/`, every service in `app/services/`. That is the right
//! shape while there is one feature. It stops being the right shape at ten,
//! when "show me everything billing does" means opening seven directories and
//! knowing which files in each are billing's.
//!
//! A module here owns its controller, service and routes in one directory and
//! declares them in a `module!` block, which the application graph validates
//! at build time. The two layouts coexist deliberately: the scaffold's `Web`
//! module keeps the by-kind directories, and an application is free to use
//! one, the other, or both.
//!
//! # This file is written by a generator
//!
//! `arc make:module` registers a new module by adding its `pub mod` line at
//! the bottom and one entry inside each of the two marked regions below. It
//! only ever inserts, and it skips a name that is already there, so
//! re-running it after deleting a directory does not leave a duplicate
//! behind.
//!
//! Editing by hand is fine -- reorder the entries, reformat them, add one
//! yourself. The generator matches the `arc:modules` / `arc:end` markers and
//! nothing else about the surrounding text. Deleting a marker is the one
//! thing that breaks it, and when that happens it says so and writes the
//! module's files anyway, leaving the registration to you.

use arcature::prelude::*;

use crate::bootstrap::AppState;

/// Every feature module in this directory, for the application graph.
///
/// `graph()` in `app/mod.rs` appends these to the scaffold's `Web` module. A
/// module missing from this list still compiles and still serves; it is
/// simply invisible to `arc routes`, `arc typegen`, and the duplicate- and
/// cycle-checking the graph does at build time. That is the same trap
/// `module!` exists to close, one level up.
#[must_use]
pub fn modules() -> Vec<ModuleDescriptor> {
    vec![
        // arc:modules descriptors
        // arc:end
    ]
}

/// Every feature module's routes, merged into one collection.
///
/// `bootstrap/app.rs` merges this into the application's own table. Merging
/// rather than nesting is what lets a module keep its paths in its own
/// `routes!` block while the URLs stay flat: a module is a unit of source
/// organisation here, not a mount point.
///
/// A list folded into one collection, rather than a `.merge(..).merge(..)`
/// chain, because the chain is where `cargo fmt` and a line-inserting
/// generator disagree: rustfmt indents a one-call chain differently from a
/// three-call one, so adding the second module would reformat the first
/// module's line. A `vec!` entry is indented the same at one entry as at
/// twenty.
#[must_use]
pub fn routes() -> Routes<AppState> {
    let collections: Vec<Routes<AppState>> = vec![
        // arc:modules routes
        // arc:end
    ];
    collections.into_iter().fold(Routes::empty(), Routes::merge)
}

// The module declarations live at the bottom, which is unusual and
// deliberate: `arc make:module` uses the same `mod.rs` bookkeeping every
// other generator uses, and that puts a new `pub mod` line after the last one
// it finds -- or at the end of the file when there is none yet. Keeping the
// block last means the first module and the tenth land in the same place.
// Rust does not care about declaration order, so the two functions above may
// name a module declared below them.
