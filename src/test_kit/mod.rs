//! The in-process test harness.
//!
//! Boots an application and drives it as a `tower::Service`, so a test needs
//! no socket, no port, and no teardown race. Enabled by the `test-kit`
//! feature, which belongs under `[dev-dependencies]` -- shipping the harness
//! in a production binary is a mistake the feature split is there to prevent.
