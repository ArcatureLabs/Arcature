# Routing

A route is a method, a path, a handler, and optionally a name. `Routes` is a
collection of them, and it is what `Application::routes` takes.

```rust,ignore
use arcature::prelude::*;

pub fn routes() -> Routes {
    Routes::new([
        Route::get("/", home).name("home"),
        Route::get("/links", index).name("links.index"),
        Route::post("/links", store).name("links.store"),
    ])
}
```

Constructors exist for `get`, `post`, `put`, `patch`, `delete`, `head` and
`options`. Anything else falls through to `any`.

## Paths and parameters

Paths are Axum paths, because the router *is* Axum's. `/links/{id}` captures a
segment; the handler takes it with `Path`:

```rust,ignore
use arcature::axum::extract::Path;

async fn show(Path(id): Path<i64>) -> Result<Response> {
    Ok(text(StatusCode::OK, format!("link {id}")))
}
```

## Names and URL generation

A named route can be turned back into a URL. Parameters are filled in
declaration order:

```rust,ignore
let url = routes.url_for("links.show", &["42"])?;   // "/links/42"
```

`url_for` returns `Err(Error::NotFound(..))` for a name that was never
declared, so a typo is a runtime error rather than a silently broken link.
`Routes::named()` iterates every name and its path template.

## Groups

`RouteGroup` shares a path prefix and, optionally, middleware:

```rust,ignore
Routes::new([
    RouteGroup::new("/admin", [
        Route::get("/panel", panel).name("admin.panel"),
        Route::get("/users", users).name("admin.users"),
    ])
    .middleware(RequireAuth),
])
```

The prefix is joined onto each route's path when the group is flattened.

## Where middleware can attach

Middleware attaches at three scopes, and the difference between them is not
cosmetic:

| Scope | Call | Reaches |
| --- | --- | --- |
| One route | `Route::middleware` | that route only |
| One group | `RouteGroup::middleware` | each route in the group, individually |
| A collection | `Routes::middleware` | every route present *at the time of the call* |

`Route` owns a `MethodRouter`, not a folded `Router`. That is deliberate: a
`Router::layer` fold applies to everything in the router, so per-route
middleware written that way leaks onto sibling routes. Holding a
`MethodRouter` per route makes the leak impossible to express.

`Routes::middleware` is the one that folds a whole router, and its scope rule
is stated in the API: routes merged in *afterwards* are not covered. That is
what lets a guarded collection and a public collection be merged without the
guard spreading:

```rust,ignore
let guarded = Routes::new([...]).middleware(RequireAuth);
let public  = Routes::new([...]);
let all = guarded.merge(public);   // `public` is still public
```

## Writing middleware

`Middleware` is a `Clone` type whose `handle` returns a boxed future.
`#[middleware]` writes that plumbing for an ordinary async function:

```rust,ignore
use arcature::routing::{Request, Response};
use arcature::{Next, Result, middleware};

#[middleware]
pub async fn require_auth(request: Request, next: Next) -> Result<Response> {
    Ok(next.run(request).await)
}
```

The function is emitted unchanged, so it stays callable directly from a test.
Alongside it the macro generates a unit struct named after the function in
PascalCase — `RequireAuth` here — implementing `Middleware`. Override the name
with `#[middleware(RequireAdmin)]`.

Returning `Err` maps the framework error to a response instead of continuing.
Not calling `next.run` short-circuits.

For middleware that is not a `Middleware` — a `tower_http` layer, say —
`Route::layer`, `RouteGroup::layer` and `Routes::layer` take a raw
`tower::Layer`.

## The `routes!` macro

The builder API above is the whole runtime. `routes!` is a declarative front
end over it that additionally emits `&'static` metadata and typed URL
helpers. This block compiles today:

```rust,ignore
routes! {
    pub app {
        get "/" => home { name: home, page: "Home" }

        group "/auth" {
            get  "/login" => login { name: auth.login }
            post "/login" => store { name: auth.store }
        }

        group "/admin" {
            middleware: [RequireAuth];
            get "/panel" => panel { name: admin.panel }
        }
    }
}
```

It generates three things from `pub app`:

- `app_routes() -> Routes` — the collection, built with the same builder API.
- `APP_ROUTES: &[RouteDescriptor]` — a `&'static` const describing every
  route: method, path, handler name, declared page names. Nothing registers
  itself at startup; the array is the whole registry.
- `app_route` — a module of URL functions mirroring the dotted names:
  `app_route::home()`, `app_route::auth::login()`,
  `app_route::admin::panel()`. A path parameter becomes a function argument,
  so `app_route::links::show(42)` is `"/links/42"` and a missing parameter is
  a compile error rather than a broken URL.

Declare state with a `state:` line:

```rust,ignore
routes! {
    pub api {
        state: AppState;
        get "/health" => health { name: api.health }
    }
}
```

And expand a controller into its conventional actions with `resource`:

```rust,ignore
routes! {
    pub web {
        resource "/links" => LinksController {
            name: links,
            only: [index, show, destroy]
        }
    }
}
```

That produces `links.index` (`GET /links`), `links.show` (`GET /links/{id}`)
and `links.destroy` (`DELETE /links/{id}`), each with its helper.

## Falling through to Axum

`Routes::into_router` hands back the `axum::Router`, and `Routes::router`
borrows it. `Route::layer` takes a `tower::Layer`. The escape hatch is not
hidden and using it is not a defeat; the framework's opinions are meant to
run out somewhere visible.

## Where routing sits in the pipeline

The router is stage 19 of 20. Everything a request passes through before it —
security headers, CORS, request id, panic catching, error mapping, body
limit, timeout, sessions, CSRF, Inertia, page contracts — is fixed and
documented in [Deployment](deployment.md) and in
`src/application/pipeline.rs`. User `.layer()` calls go in at stage 18, just
outside the router; static files are stage 20, reached only when the router
does not match.
