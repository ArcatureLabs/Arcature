//! The `arc` CLI entry point.
//!
//! Shipped from the same `arcature` package as a binary target. A normal
//! application never compiles the CLI; it is built by
//! `cargo install arcature` or `cargo build --bin arc`.

#[cfg(feature = "cli")]
fn main() -> std::process::ExitCode {
    arcature::cli::run(std::env::args_os())
}

#[cfg(not(feature = "cli"))]
fn main() -> std::process::ExitCode {
    eprintln!("arc: the CLI is not enabled. Build with the `cli` feature.");
    std::process::ExitCode::from(1)
}
