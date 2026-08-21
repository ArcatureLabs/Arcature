# Testing

Arcature applications are tested the way Tower services are tested: build the
router, drive it in-process, assert on the response. There is no socket, no
port to allocate, and no teardown race.

## Driving the router

`Application::into_router` hands back the composed `Router`. From there
`tower::ServiceExt::oneshot` sends a single request through the whole
router-level pipeline and returns the response.

```rust
use arcature::axum::Router;
use arcature::axum::body::Body;
use arcature::axum::http::{Request, Response};
use tower::ServiceExt as _;

async fn send(router: Router, request: Request<Body>) -> Response<Body> {
    router.oneshot(request).await.expect("infallible")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request")
}
```

`tower` is not re-exported by Arcature, so a test crate using `oneshot`
needs `tower = { version = "0.5", features = ["util"] }` under
`[dev-dependencies]`. `axum` is re-exported, as `arcature::axum`.

This is the pattern the framework's own `tests/application.rs` uses. It
exercises stages 3 through 20 of the [pipeline](deployment.md#the-pipeline):
everything the builder composes onto the router.

It does **not** exercise stages 1 and 2 -- the dev proxy and the pre-routing
proxy. Those are composed around the router as a service in `run_with_state`
and `serve`, because they rewrite the URI before route selection. A test that
needs them has to build the service, not the router.

## Asserting layer order

Layer order is a contract, so the framework asserts it rather than only
documenting it. The technique is a marker layer that appends its own name to a
response header; the resulting header value spells out the order the layers
actually ran in. If a future edit reorders the stack, the assertion fails with
a diff a reader can act on, instead of a subtly different security posture
nobody notices.

If your application depends on where its own `.layer()` calls sit relative to
the framework's, the same technique works: user layers are stage 18, inside
everything the builder installs and outside the router.

## Route tables

A route table is data, so it can be asserted without a request at all. The
generated application ships exactly this as its smoke test:

```rust
use my_app::routes;

#[test]
fn home_route_is_registered() {
    let routes = routes::routes();
    assert_eq!(routes.url_for("home", &[]).unwrap(), "/");
}
```

`url_for` returns `Err(Error::NotFound(..))` for a name that is not in the
table, so a renamed route fails the test rather than silently producing a
broken link at runtime.

## Events

`Dispatcher::recording()` builds a dispatcher that remembers the names of the
events it dispatched. `was_dispatched(name)` and `dispatched_events()` read
that record back. Both return `false` / an empty vector on a dispatcher built
with `Dispatcher::new()` -- recording is opt-in and costs nothing in
production.

Assert on the event, not on the listener's side effect, when what you care
about is that the event fired. Assert on the listener when what you care about
is what it did.

## Mail

`Mailer::capture_ok()` accepts every message and keeps it; `Mailer::capture_error()`
rejects every message. Both are constructors, so a test builds one directly
instead of pointing SMTP at a local catcher.

```rust
use arcature::mail::Mailer;

let mailer = Mailer::capture_ok();
// ... run the code under test ...
let sent = mailer.captured().await.expect("capturing mailer");
assert_eq!(sent.len(), 1);
```

`captured()` returns `Option<Vec<(Envelope, String)>>` -- `None` when the
mailer is not a capturing one, so a test that accidentally runs against real
SMTP fails on the `expect` rather than passing vacuously. The `String` is the
serialised message; the `Envelope` carries the actual sender and recipients,
which is what you want to assert on, since the envelope and the `To:` header
can legitimately differ.

`capture_error()` is the one that finds the bugs: it proves the calling code
handles a send failure instead of unwrapping it.

## Jobs

A job handler is an ordinary async function over a deserialised payload. Test
it by calling it. That covers the interesting part -- the business logic and
the `JobError::Retryable` / `JobError::Permanent` decision -- without a
database.

Testing the queue itself needs PostgreSQL, because the claim protocol is
`FOR UPDATE SKIP LOCKED` and there is nothing to emulate it with. The
framework's own suite takes a `DATABASE_URL` for exactly this reason; see
[Deployment](deployment.md#continuous-integration) for the CI service
definition.

## Validation

`Validated<T>` and its siblings are extractors, so they are tested through a
request. A `422` with an `errors` extension on the problem document is the
success case for a validation test -- assert on the field names in that
extension, not on the message strings, which are not a stable interface.

## Databases in tests

There is no per-test transaction rollback helper and no test database
provisioner. A test that needs a database connects to one named by an
environment variable and is responsible for its own cleanup. This is less
convenient than the Laravel equivalent and is a known gap.

## The `test-kit` feature

`into_router` plus `oneshot` covers the router-level pipeline and needs no
framework support at all, which is why the chapter leads with it. When a test
needs the whole application -- subsystem startup, state construction, the lot
-- `arcature::test_kit` boots one in process and drives it as a
`tower::Service`, so there is no socket, no port and no teardown race.

Enable it under `[dev-dependencies]`:

```toml
[dev-dependencies]
arcature = { version = "2026.3", features = ["test-kit"] }
```

The feature belongs there and nowhere else: shipping a test harness inside a
production binary is the mistake the feature split exists to prevent.

What it holds: `TestApp` (the in-process driver) and `TestServer` (a real
socket, for the few things a `tower::Service` call cannot exercise, such as a
WebSocket upgrade); `TestRequest` and `TestResponse` with the assertions;
session seeding behind the `auth` feature, so `acting_as` has something to act
as; a two-condition database gate with transaction-per-test and
`assert_database_has` behind the `database` feature; and recorder fakes for
events, jobs and mail, wired into the seams those subsystems already expose
rather than being parallel copies of them. `#[arcature::test(app = ...)]`
binds a fresh `TestApp` to the test function's parameter.

It registers nothing globally -- no inventory, no thread-local, no ambient
application. A test names the thing it is testing and holds it in a value.
