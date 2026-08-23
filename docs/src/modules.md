# Your first module

A fresh application is laid out by kind: every controller in
`app/controllers/`, every service in `app/services/`, every page in
`app/pages/`. That is the right shape while there is one feature. It stops
being the right shape at ten, when "show me everything billing does" means
opening seven directories and knowing which files in each are billing's.

A *module* is the other shape. One directory owns one feature's controller,
service and routes, and a `module!` block at its root is the index of what is
inside. The two layouts coexist deliberately — the scaffold's own `Web` module
keeps the by-kind directories, and an application is free to use one, the
other, or both.

This page goes from `arc new` to a route that answers, without editing a file
by hand.

## Generate it

```console
$ arc new acme --stack react --db sqlite
$ cd acme
$ arc make:module billing
created app/modules/billing/mod.rs
updated app/modules/mod.rs
created app/modules/billing/controller.rs
created app/modules/billing/service.rs
created app/modules/billing/routes.rs
```

Four files written, one updated. The module is registered: nothing else has to
be touched before it serves.

```console
$ arc serve --port 3000
$ curl localhost:3000/billing
BillingController::index
```

## What landed

### `app/modules/billing/mod.rs`

The index. `module!` records what this feature contains, so the application
graph — and through it `arc routes`, `arc typegen` and `arc build` — can see
it.

```rust,ignore
pub mod controller;
pub mod routes;
pub mod service;

use arcature::prelude::*;

use controller::BillingController;

module! {
    pub Billing {
        controllers: [BillingController],
        services: [BillingService],
        routes: routes::BILLING_ROUTES,
    }
}
```

`BillingService` is named but not imported, and that is not an oversight.
`controllers:` and `routes:` are *resolved* at this site — the first reads the
controller's `ControllerMetadata::METHODS`, the second is a path to a const —
while `services:` and `policies:` are recorded as names only. Importing a type
the macro never resolves would be an unused import.

### `app/modules/billing/controller.rs`

```rust,ignore
use arcature::prelude::*;

pub struct BillingController;

#[controller]
impl BillingController {
    pub async fn index() -> Result<Response> {
        Ok(text(StatusCode::OK, "BillingController::index"))
    }
}
```

`Result`, `Response`, `StatusCode` and `text` all come from the prelude, which
is the point: a controller should not need a `use` list before it does any
work.

### `app/modules/billing/service.rs`

```rust,ignore
#[service]
pub struct BillingService {
    db: Db,
}
```

`#[service]` generates `Resolve<S>`, which composes the struct from the
application's resources per request: each field type is resolved in turn, so a
service may hold another service as a field. Keep the methods
framework-agnostic — take domain values, return domain values, and let the
controller map the result to HTTP.

### `app/modules/billing/routes.rs`

```rust,ignore
use super::controller::BillingController;
use crate::bootstrap::AppState;

routes! {
    pub billing {
        state: AppState;

        get "/billing" => BillingController::index { name: billing.index }
    }
}
```

The path is absolute. Living in a module adds no prefix — a module is a unit
of source organisation here, not a mount point. Reach for a
[route group](routing.md) when you want a shared prefix, inside the module or
outside it.

The route *name* carries the module's own, and that matters more than it
looks. `app/modules/mod.rs` merges every module's routes into one table. Two
modules claiming the same **path** is a panic at boot, from Axum, so it cannot
reach production unnoticed. Two modules claiming the same **name** is not: the
later one silently wins, and `url_for("index", ..)` starts resolving somewhere
else. Namespacing the name is what makes ten modules safe to merge.

## How it is wired

`arc make:module` adds three lines to `app/modules/mod.rs`: the `pub mod`
declaration, one entry in the descriptor list, and one in the route list.

```rust,ignore
pub fn modules() -> Vec<ModuleDescriptor> {
    vec![
        // arc:modules descriptors
        billing::billing_module().clone(),
        // arc:end
    ]
}

pub fn routes() -> Routes<AppState> {
    let collections: Vec<Routes<AppState>> = vec![
        // arc:modules routes
        billing::routes::billing_routes(),
        // arc:end
    ];
    collections.into_iter().fold(Routes::empty(), Routes::merge)
}
```

`app/mod.rs` appends `modules()` to the scaffold's `Web` module before handing
the list to `ApplicationGraph::new`; `bootstrap/app.rs` merges `routes()` into
the application's table. Both were already written by `arc new` — the
generator only ever inserts into the two marked regions.

Editing the file by hand is fine. Reorder the entries, reformat them, add one
yourself. The generator matches the `arc:modules` / `arc:end` markers and
nothing else about the surrounding text, and it skips an entry that is already
there, so re-running it after deleting a directory does not leave a duplicate
behind.

Deleting a marker is the one thing that breaks it. When that happens it says
so and writes the module's files anyway, leaving one line for you to paste:

```console
$ arc make:module billing
created app/modules/billing/mod.rs
updated app/modules/mod.rs
note: app/modules/mod.rs has no `// arc:modules routes` marker -- add `billing::routes::billing_routes(),` to its routes list by hand
created app/modules/billing/controller.rs
...
```

## Nested names

A name with slashes nests, and the intermediate `mod.rs` files are created as
needed:

```console
$ arc make:module admin/reports
created app/modules/admin/reports/mod.rs
updated app/modules/admin/mod.rs
updated app/modules/mod.rs
```

The registration follows the nesting — `admin::reports::reports_module()` —
and so does the route name, which becomes `admin.reports.index`. The URL does
not: `routes.rs` still declares an absolute path, so the module serves
`/admin/reports` because that is what is written there, not because of where
it sits on disk.

## What the graph checks

`ApplicationGraph::new` runs at boot and rejects three wiring mistakes:

| `GraphError` | Means |
| --- | --- |
| `DuplicateModule` | two modules declared the same name |
| `UnknownImport` | an `imports:` entry names a module the graph does not hold |
| `CircularDependency` | modules import each other in a loop; the error lists the cycle in order |

`app/mod.rs` calls `.expect(..)` on the result, so all three are a panic at
boot in the scaffold. That is the honest answer: each one is the same on every
run and has nothing to do with the request, so surfacing it on one unlucky
request later would only make it harder to place.

A type missing from a `module!` block still compiles and still serves. It is
simply invisible to the graph, and therefore to `arc routes`, `arc typegen`
and `arc build`. That is the trap `module!` exists to close, one level up.

## What is deliberately not scaffolded

A module gets four files, not five. `arc make:policy` is one command away, but
a generated policy cannot compile until it is pointed at a model and a user
type — `Policy<M>` bounds its associated `User` by `AuthUser`, so there is no
placeholder that type-checks. Shipping one inside a module would mean
`arc make:module billing` produces a project that does not build.

Add one once the feature has a model and a user type to point it at:

```console
$ arc make:policy invoice
created app/policies/invoice.rs
updated app/policies/mod.rs
note: invoice names `Invoice` and `User`; point them at the model this policy guards and the application's user type
```

That lands in `app/policies/`, not in the module — nearly every `make` kind
writes to the by-kind directory it belongs to. The two exceptions are
`make:module`, because a module *is* a directory, and `make:auth`, which
writes a feature's worth of files into `app/auth/`. Move the file under
`app/modules/billing/policy.rs` if you want the feature to own it, add the
`pub mod policy;` line, and name the type in the module's `policies:` list.
Nothing in the generator or the graph depends on where the file sits; the list
is what the graph reads.

## The `module!` sections

| Section | Holds | Resolved at the call site? |
| --- | --- | --- |
| `imports` | module names this one depends on | no — names |
| `exports` | module names this one provides | no — names |
| `controllers` | controller types | **yes** — reads `ControllerMetadata::METHODS` |
| `services` | service type names | no — names |
| `policies` | policy type names | no — names |
| `routes` | a path to a `&'static [RouteDescriptor]` const | **yes** |
| `listeners` | event → listener name pairs | no — names |
| `jobs` | job kind, version and handler name | no — names |
| `commands` | command name → function name pairs | no — names |
| `schedules` | job kind, version and cadence | no — names |
| `pages` | paths to `#[page]` types | **yes** — reads `PAGE_CONTRACT_ENTRY` |

Order between sections is free, every section is optional, and the trailing
comma is optional. The right-hand column is the whole rule for what has to be
in scope: a name is a string the graph compares, a resolved entry is a type or
const the macro reads through. Which way a section falls decides what a typo
costs. A misspelled entry in `controllers:` is an ordinary "cannot find type"
at the `module!` line. A misspelled entry in `services:` compiles, because the
macro only ever records the string — the graph reports the name you wrote, and
nothing anywhere disagrees with it.

## Next

[Routing](routing.md) for groups, middleware and the route table;
[Controllers](controllers.md) for what `#[controller]` emits;
[Testing](testing.md) for driving a module's routes without a server.
