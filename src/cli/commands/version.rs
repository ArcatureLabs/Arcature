//! `arc version` — print the framework version.
//!
//! Also reached via `--version` / `-V`. No arguments; no error path beyond the
//! shared `Missing` from the dispatcher.

/// Execute the `version` subcommand: print `arcature <version>`.
pub fn run() {
    println!("arcature {}", crate::FRAMEWORK_VERSION);
}
