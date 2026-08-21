# Controllers

A handler is an ordinary `async fn`. A controller is a `struct` with an
`impl` block full of them, which is a convention rather than a requirement —
`Route::get("/", index)` takes a free function just as happily.

```rust,ignore
pub struct HomeController;

#[arcature::controller]
impl HomeController {
    pub async fn index() -> String {
        "hello".to_string()
    }

    pub async fn show(id: u64) -> String {
        format!("show {id}")
    }
}
```

The impl block is emitted unchanged. `HomeController::index()` is still a
plain async function a test can call directly, and still a genuine Axum
handler.

## What the macro adds

`#[controller]` additionally emits `impl ControllerMetadata`, whose `METHODS`
const carries one entry per handler: the method name, its parameter names,
and the page it renders.

```rust,ignore
use arcature::ControllerMetadata;

let methods = <HomeController as ControllerMetadata>::METHODS;
assert_eq!(methods[0].name, "index");
assert_eq!(methods[1].params, ["id"]);
```

`METHODS` is a `&'static` const. Nothing registers itself at startup and
nothing is looked up by `TypeId`; see
[Decisions](decisions.md).

## The contract the macro enforces

Every method in the block must be `pub`, `async`, have a return type, and
take no `self` receiver — an Axum handler is a free function. Breaking any of
those produces `error[ARC-M004]` at the method, not a page of trait-bound
noise.

## The page edge

A handler that returns `Page<T>` has its page identity read off the return
type:

```rust,ignore
#[arcature::page("Dashboard")]
pub struct DashboardPage {
    pub title: String,
}

#[arcature::controller]
impl DashboardController {
    pub async fn index() -> arcature::Page<DashboardPage> {
        arcature::dx::page(DashboardPage {
            title: "Dashboard".to_string(),
        })
    }

    #[page("Reports")]
    pub async fn reports() -> String {
        "reports".to_string()
    }
}
```

`methods[0].page` is `Some("Dashboard")`, derived from the signature and
never from the body. The derivation compiles to `<T>::PAGE_CONTRACT.name()`,
a const that exists only for `#[page]` types — so a handler that tries to
return a non-page type as a page fails to compile. That is the Client
Exposure Firewall applied to the return type; [Inertia](inertia.md) covers
the rest of it.

Any other return shape (`Response`, `Json<T>`, `String`, `impl
IntoResponse`) yields `page: None`. A handler that renders a page without
returning `Page<T>` declares the identity with an explicit `#[page("Name")]`
helper attribute, as `reports` does above.

## Extractors

Handler arguments are Axum extractors, unchanged, because the router is
Axum's:

```rust,ignore
use arcature::axum::extract::{Path, Query, State};

pub async fn show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response> {
    Ok(json(&id))
}
```

Arcature adds its own: `Auth` and `Current` for the signed-in user,
`Session`, `Flash`, `CsrfToken`, `Validated<T>` and its typed variants, and
`RequestCache`. Each is documented in the chapter that owns it.

## Responses

Four builders cover the common shapes.

| Call | Produces |
| --- | --- |
| `text(StatusCode::OK, "hello")` | a `text/plain` response |
| `json(&value)` | an `application/json` response, status 200 |
| `no_content()` | 204 |
| `redirect().to("/dashboard")` | 303, or 308 after `.permanent()` |

`json` takes one argument. It does not take a status; build the response
directly if you need a different one.

`redirect()` takes no arguments — it returns a builder. `.to(path)`,
`.back()`, `.permanent()`. Unlike the other three it returns a
`RedirectResponse`, not a `Response`, so a handler declared
`-> Result<Response>` finishes with `.into_response()`:

```rust,ignore
Ok(redirect().to("/dashboard").into_response())
```

Redirect targets are validated against open redirects: an absolute URL to
another host is rejected rather than followed.

`redirect().route("links.show", 42)` resolves the name against the
application's route table, and `redirect().with("status", "saved")` writes
flash data through the session. Neither can be finished by `into_response`,
which sees no request and so has neither the table nor the session: the
builder rides along in the response extensions and `RedirectMapper` -- stage
20 of the pipeline, installed by default -- takes it out and completes it.
`.back()` works the same way, reading `Referer` through the same open-redirect
validation.

The one thing to know is what happens without that layer. An application that
assembles its own pipeline instead of using the builder, and does not install
`RedirectMapper`, gets the fallback response unchanged: a literal path still
redirects, a named route answers **400**, and flash data is dropped.

## Errors

Handlers return `Result<Response>` — Arcature's `Result`, whose error type
converts into an HTTP response. `bad_request`, `forbidden` and `not_found`
build the common ones; `Problem` builds an RFC 9457 body. The pipeline's
error-mapping stage gives a body to errors that were returned bodiless, and
in release builds it redacts 5xx detail rather than leaking it.

## Grouping controllers into a module

`module!` names the controllers, services, jobs and listeners that belong
together and aggregates their metadata into one `ModuleDescriptor`:

```rust,ignore
arcature::module! {
    pub Dashboard {
        controllers: [DashboardController],
    }
}

let descriptor = dashboard_module();
assert_eq!(descriptor.controllers, ["DashboardController"]);
```

The descriptor is built from the same `&'static` consts the macros emit, so
the module is a description of wiring rather than a container that resolves
things at runtime.
