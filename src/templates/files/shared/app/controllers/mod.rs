//! The application's controllers (Axum handlers).
//!
//! A controller is a unit struct with a `#[controller]` impl block. Every
//! method is a free `pub async fn` with no `self`, so a handler's inputs are
//! visible in its signature and nothing is smuggled in through a field.
//!
//! Add one file per controller with `arc make:controller`, then register it
//! in the `module!` block in `app/mod.rs` and give it a route.

pub mod home_controller;

pub use home_controller::HomeController;
