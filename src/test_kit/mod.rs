//! The in-process test harness.
//!
//! Boots an application and drives it as a `tower::Service`, so a test needs
//! no socket, no port, and no teardown race. Enabled by the `test-kit`
//! feature, which belongs under `[dev-dependencies]` -- shipping the harness
//! in a production binary is a mistake the feature split is there to prevent.
//!
//! # Layout
//!
//! * [`app`] -- [`TestApp`], the in-process driver, and [`TestServer`], the
//!   real-socket mode for the few tests that need one (a WebSocket upgrade
//!   cannot be exercised through a `tower::Service` call).
//! * [`request`] -- the fluent request builder ([`TestRequest`]).
//! * [`response`] -- [`TestResponse`] and every assertion.
//! * [`session`] -- seeding a session so `acting_as` has something to act as.
//! * [`database`] -- the two-condition safety gate, transaction-per-test, and
//!   `assert_database_has`.
//! * [`fakes`] -- recorders wired into the seams the subsystems already
//!   expose, rather than parallel copies of them.
//!
//! # What this harness does not do
//!
//! It registers nothing globally. There is no inventory, no thread-local,
//! no ambient application. A test names the thing it is testing, and the
//! harness holds it in a value.

pub mod app;
#[cfg(feature = "database")]
pub mod database;
pub mod fakes;
pub mod request;
pub mod response;
#[cfg(feature = "auth")]
pub mod session;

pub use app::{IntoTestApp, TestApp, TestServer};
#[cfg(feature = "database")]
pub use database::{TestDatabase, TestDatabaseError, TestTransaction, assert_database_has};
#[cfg(feature = "events")]
pub use fakes::Events;
#[cfg(feature = "jobs")]
pub use fakes::{JobRecord, Jobs};
#[cfg(feature = "mail")]
pub use fakes::{Mail, SentMail};
pub use request::TestRequest;
pub use response::TestResponse;
#[cfg(feature = "auth")]
pub use session::{TestSessionError, TestSessions};

/// The `#[arcature::test(app = ...)]` attribute.
///
/// Expands to an ordinary `#[test]` that runs an async body on a fresh
/// runtime and binds the function's single parameter to a [`TestApp`] built
/// from the `app` expression. See the `arcature-macros` `test_attr` module
/// for the full contract.
pub use arcature_macros::test;

/// Run `future` to completion on a runtime created for this call alone.
///
/// The `#[arcature::test]` expansion calls this. It is public because the
/// expansion is written in the user's crate, and because a hand-written
/// `#[test]` occasionally wants the same one-runtime-per-test guarantee
/// without taking a direct `tokio` dependency.
///
/// A multi-thread runtime rather than a current-thread one: application code
/// spawns, and a test that deadlocks only because the harness gave it one
/// worker teaches the wrong lesson.
///
/// # Panics
///
/// Panics if the runtime cannot be created, which means the process is out
/// of threads or file descriptors -- not a condition a test can recover from.
pub fn block_on<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("test runtime could not be created")
        .block_on(future)
}

/// Render up to `limit` bytes of a response body for a failure message.
///
/// Assertions print the body they were looking at. A body can be a megabyte
/// of HTML, so it is truncated -- but never omitted, because an assertion
/// message without the actual value is an assertion message that costs
/// another test run to act on.
pub(crate) fn preview(body: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(body);
    if text.len() <= limit {
        return text.into_owned();
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes total)", &text[..end], body.len())
}
