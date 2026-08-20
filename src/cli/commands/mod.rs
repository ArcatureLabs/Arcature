//! The per-subcommand modules.
//!
//! One file per command: each holds its argument parser, its executor, and the
//! command-specific error type. The dispatcher in [`super::parser`] routes to
//! these via `commands::<name>::parse` / `commands::<name>::run`.
//!
//! The `queue` and `doctor` commands touch the database and are gated on the
//! `database` (and `jobs` for `queue`) features; without them the dispatcher
//! reports the command as unavailable.

pub mod new;
pub mod version;
pub mod serve;
pub mod migrate;
pub mod schedule;

#[cfg(feature = "database")]
pub mod doctor;
#[cfg(all(feature = "database", feature = "jobs"))]
pub mod queue;
