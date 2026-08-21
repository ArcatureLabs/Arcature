//! The per-subcommand modules.
//!
//! One file per command: each holds its executor and its command-specific
//! error type. Parsing is not here -- clap owns the whole command surface in
//! [`super::parser`], and [`super::execute`] routes a parsed
//! [`Subcommand`](super::parser::Subcommand) to `commands::<name>::run`.
//!
//! Keeping the executors split this way is what lets a command's error say
//! something specific: `db:fresh` refusing without `--force` and
//! `storage:link` refusing to overwrite are different failures, and neither
//! reads well as a generic "command failed".
//!
//! Commands that need a capability the build may not have are gated on the
//! same features their parser variants are: `queue` on `database` + `jobs`,
//! `doctor` on `database`, `key:generate` on `auth`, and the three
//! application-graph commands -- `routes`, `typegen` and `build` -- on `uag`,
//! which is what carries the artifact they all read.

pub mod db;
pub mod dev;
pub mod make;
pub mod migrate;
pub mod new;
pub mod schedule;
pub mod serve;
pub mod storage_link;
pub mod version;

// The application-graph commands. They deserialize the UAG artifact and run
// its validator and codegen, so without `uag` there is nothing for them to
// read and they are absent rather than present-and-failing.
/// A cause whose concrete type is an implementation detail.
///
/// The three application-graph commands all fail the same way when the graph
/// cannot be read, and the reader lives in a private module. Boxing keeps
/// [`std::error::Error::source`] working without publishing that module's
/// error type as part of the crate's surface.
pub type Cause = Box<dyn std::error::Error + Send + Sync + 'static>;

#[cfg(feature = "uag")]
pub mod build;
#[cfg(feature = "uag")]
pub mod routes;
#[cfg(feature = "uag")]
pub mod typegen;
#[cfg(feature = "uag")]
pub(crate) mod uag_source;

#[cfg(feature = "auth")]
pub mod key_generate;

#[cfg(feature = "database")]
pub mod doctor;
#[cfg(all(feature = "database", feature = "jobs"))]
pub mod queue;
