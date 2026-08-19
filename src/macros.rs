//! Runtime macros: `#[arcature::main]`.
//!
//! Re-exports the certified Tokio multi-thread runtime macro so a normal
//! application needs no direct `tokio` dependency for `#[tokio::main]`.

pub use tokio::main;
