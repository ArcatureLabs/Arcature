# Inertia

Arcature implements the server side of the Inertia v3 protocol natively. A
stock official `@inertiajs/react` or `@inertiajs/vue3` client talks to it
without knowing Arcature exists.

There is no `@arcature/client` package, and there will not be one. The
reasoning is in [ADR 0001](decisions.md): everything Rust hands JavaScript
goes as generated `.ts` files on disk under `resources/js/generated/`, not
through a bundler plugin the framework has to keep alive.

## The mental model

The browser's Inertia client makes ordinary HTTP requests. On a first visit
Arcature renders the initial HTML document with the page object embedded in
it. On subsequent visits — requests carrying `X-Inertia` — it returns the page
object as JSON. Same route, same handler, two representations.

## Configuring it

`InertiaConfig::new` takes an asset version and a root-document renderer:

```rust,ignore
use arcature::assets::{Assets, AssetsConfig};
use arcature::inertia::{InertiaConfig, vite_root_document};

let assets = Assets::detect(&AssetsConfig::new())?;
let config = InertiaConfig::new(
    env!("CARGO_PKG_VERSION"),
    vite_root_document("Acme", &assets, "resources/js/app.tsx"),
)?;

Application::new()
    .routes(routes())
    .inertia(config)
    .build()
```

`default_root_document(title)` is the minimal renderer if you are not using
Vite. `with_shared(shared_props)` registers props every page receives.

A root document is any `Fn(ScriptBody) -> String`. `ScriptBody` displays as
the `<script data-page>` payload plus the mount `<div>`, and it also carries
this request's CSP nonce when
[`SecurityHeaders::with_csp_nonce`](deployment.md#csp-nonces) is installed.
Both built-in renderers stamp it onto every tag they emit; a hand-written one
has to stamp its own, and `body.nonce_attribute()` is the attribute (with its
leading space, or empty when there is no nonce) to interpolate:

```rust,ignore
let config = InertiaConfig::new(env!("CARGO_PKG_VERSION"), |body: ScriptBody| {
    let nonce = body.nonce_attribute();
    format!(
        "<!doctype html><html><body>{body}\
         <script{nonce} type=\"module\" src=\"/js/app.js\"></script>\
         </body></html>"
    )
})?;
```

`.inertia(config)` is what installs `InertiaLayer`. Without it the `Inertia`
extractor fails: a handler taking `inertia: Inertia` in an application that
never called `.inertia(..)` returns `500 inertia adapter error`. That is
documented on the builder method and is worth remembering, because the
failure looks like a handler bug rather than a wiring one.

## Rendering

The untyped path takes any `Serialize`:

```rust,ignore
pub async fn index(inertia: Inertia) -> Result<Response> {
    let response = inertia
        .render("users/index", serde_json::json!({ "users": [] }))
        .await?;
    Ok(response)
}
```

The `inertia!` macro is sugar over it. It requires an in-scope binding
literally named `inertia`, because it expands to a call on that name:

```rust,ignore
pub async fn index(inertia: Inertia, State(state): State<AppState>) -> Result<Response> {
    let db = state.db.as_ref().ok_or_else(|| not_found("no database"))?;
    let users = user::Entity::query(db).all().await?;
    inertia!("users/index", { users })
}
```

`render_with_options` adds page-level options (history flags, flash data);
`render_advanced` takes a `Props` value for per-prop behaviour.

The first argument to `InertiaConfig::new` is the asset version — any string
that changes when the built assets change. The Inertia client compares it and
does a full page reload when it moves. A release tag or a manifest hash both
work; a constant means the client never reloads on deploy.

## The Client Exposure Firewall

`Serialize` does not mean "safe to send to a browser". A domain model derives
`Serialize` for a hundred reasons, and any one of them makes it one field
reference away from the wire. Arcature makes browser exposure a separate,
explicit opt-in.

Two macros grant it.

`#[page("name")]` declares a page's prop struct:

```rust,ignore
#[arcature::page("users/show")]
pub struct ShowUserPage {
    pub user: UserResource,
    pub can_edit: bool,
}
```

`#[resource]` declares a value that nests inside page props:

```rust,ignore
#[arcature::resource]
pub struct UserResource {
    pub id: String,
    pub name: String,
    pub avatar: Option<AvatarResource>,
}
```

Both emit `impl ClientData`, whose `exposure_schema()` is built from the named
fields. A non-primitive field type maps to `PropsSchema::nested::<FieldType>`,
which requires `FieldType: ClientData`. So nesting an internal model inside a
page does not compile — the failure is a trait bound at build time, not a leak
in production.

`#[page]` additionally emits a `PAGE_CONTRACT` const (the typed handle) and a
`PAGE_CONTRACT_ENTRY` const (the non-generic one `module!` aggregates). Both
are `&'static`. Nothing registers itself; `application!` builds the
`PageContracts` registry from the graph.

`#[resource]` emits no `PAGE_CONTRACT`: resources are values inside pages, not
pages.

A database model is not a resource. A SeaORM entity stays an entity, and
application code converts explicitly with `impl From<User> for UserResource`.
The conversion is the place where you decide what the browser sees, which is
the point of writing it out.

## Rendering through the firewall

```rust,ignore
pub async fn show(inertia: Inertia) -> Result<Response> {
    let page = ShowUserPage {
        user: UserResource { id: "1".into(), name: "Ada".into(), avatar: None },
        can_edit: true,
    };
    Ok(inertia.render_page(ShowUserPage::PAGE_CONTRACT, page).await?)
}
```

`render_page` is `render` with a `ClientData` bound. The component name comes
from the contract rather than a string literal, so a renamed page cannot
drift from its route.

A controller method may instead return `Page<T>` and let `#[controller]` read
the page identity off the return type — see [Controllers](controllers.md).
`page!(ShowUserPage { .. })` constructs one with a compile-time `ClientData`
assertion at the call site.

## Prop behaviours

`Props` carries per-prop evaluation strategy, matching the Inertia protocol:

| Constructor | Behaviour |
| --- | --- |
| `eager(value)` | always serialized |
| `always(value)` | included even in partial reloads |
| `lazy(f)` | resolved only when requested |
| `optional(f)` | omitted unless the client asks for it |
| `deferred(f)` | sent in a follow-up request |
| `deferred_group(name, f)` | deferred, batched under a group |

`merge(prop)`, `prepend(prop)` and `deep_merge(prop)` set the client-side
merge strategy for a prop that accumulates across visits.

## Contracts as an artifact

`.page_contracts(artifact)` publishes the collected page contracts as a
request extension. It changes no response; it is data for the dev-only UAG
endpoint and for `arc typegen` to read when generating TypeScript.

`arc typegen` reads that artifact and writes the TypeScript, which is the
generated-types pipeline ADR 0001 describes and the reason the contracts are
collected at all.

## Redirects

`inertia.redirect(location)` builds an Inertia-aware redirect. `external(url)`
produces a `409` with `X-Inertia-Location`, which is how the protocol tells
the client to leave the SPA. `fragment(..)` targets a fragment.

## One port

In development, Vite runs in `middlewareMode` with no TCP port of its own and
the Rust process forwards to it over an IPC endpoint. There is one port in
development and one in production, and no `localhost:5173` fallback. See
[ADR 0003](decisions.md) and [Deployment](deployment.md).
