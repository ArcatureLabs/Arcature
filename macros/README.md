# arcature-macros

[![crates.io](https://img.shields.io/crates/v/arcature-macros.svg)](https://crates.io/crates/arcature-macros)
[![docs.rs](https://img.shields.io/docsrs/arcature-macros)](https://docs.rs/arcature-macros)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

The proc-macro half of [Arcature](https://github.com/ArcatureLabs/Arcature), an
opinionated full-stack Rust web framework.

## You probably want `arcature` instead

This crate is a companion, not an entry point. Every macro in it expands to
paths rooted at `::arcature::`, so an expansion only compiles in a crate that
depends on `arcature` — and `arcature` already re-exports all of them behind
its `macros` feature. Depending on `arcature-macros` directly buys you nothing
and gives you a version pair to keep in step by hand.

```toml
[dependencies]
arcature = "0.1"
```

```rust,ignore
use arcature::prelude::*;

#[arcature::model(table = "users")]
pub struct User {
    pub id: i64,
    pub email: String,
}
```

The dependency is `= "0.1.0"`-pinned from `arcature`'s side, and the two crates
are released together from the same tag. Treat their version numbers as one
number.

## What is in here

Twenty-three entry points, grouped by what they attach to.

| Kind | Macros |
|---|---|
| Domain types | `#[model]`, `#[request]`, `#[derive(Job)]`, `#[derive(Event)]`, `#[derive(DxComponent)]` |
| Request handling | `#[controller]`, `#[middleware]`, `#[route_model]`, `#[resource]`, `#[page]` |
| Wiring | `#[service]`, `#[provider]`, `#[policy]`, `#[listener]`, `#[job_handler]`, `#[command]` |
| Declarative DSL | `application!`, `module!`, `routes!`, `page_macro!`, `redirect!` |
| Performance | `#[request_cache]` |
| Testing | `#[arcature::test]` |

One file, one macro. `lib.rs` is only a dispatch surface: it declares each
`#[proc_macro_*]` entry point and forwards to the module that implements it.

## Two properties worth knowing about

**No hidden registry.** Nothing here writes into `inventory`, `linkme`, a
`TypeId → Any` map, a thread-local, or a task-local. Every macro emits
`&'static` const metadata attached to the type it annotates, and the
application wiring reads it through ordinary trait impls. That is what makes
the whole application graph — routes, modules, page contracts, field shapes —
inspectable without running the application, which in turn is what
`arc typegen` and the OpenAPI output are built on.

**No panics on ordinary mistakes.** Every macro implementation returns
`Result<TokenStream, MacroError>`; the `lib.rs` entry point turns an error into
a `compile_error!` carrying a stable `ARC-M<NNN>` code. Misspell a key in
`#[job(...)]` and the compiler says `error[ARC-M009]` with a span on the
offending token, not `proc macro panicked`.

The codes are stable and greppable:

| Code | Meaning |
|---|---|
| `ARC-M001` | Input is not valid Rust syntax for this macro |
| `ARC-M002` | Unknown key, wrong value type, or missing value in an attribute |
| `ARC-M003` | Unknown action name in a `resource` route's `only` / `except` |
| `ARC-M004` | A `#[controller]` method is not `pub async` with a return type |
| `ARC-M005` | `#[route_model]` is missing `entity`, `key` or `key_type` |
| `ARC-M006` | `#[service]` / `#[provider]` on something other than a named-field struct |
| `ARC-M007` | A `#[middleware]` function is not `pub async` with a return type |
| `ARC-M008` | A `#[listener]` function is not `pub async` with a return type |
| `ARC-M009` | Bad `#[job(...)]` argument, or `version` / `attempts` below 1 |
| `ARC-M010` | A `#[job_handler]` function is not `pub async` with a return type |
| `ARC-M011` | A `#[command]` function is not `pub async` with a return type |
| `ARC-M012` | `#[arcature::test]` is missing `app = <expr>`, or was given a string |
| `ARC-M013` | `#[request_cache]` is missing `name` / `key`, or has both `key` and `keys` |
| `ARC-M014` | A `#[request_cache]` function is not `pub async` with a return type |

A code is added only when a macro grows a genuinely new failure mode, never
speculatively.

## Dependencies

`syn`, `quote`, `proc-macro2`. Nothing else, and in particular **not**
`arcature` — that would be a cycle.

## Documentation

Per-macro reference lives in the [API docs](https://docs.rs/arcature-macros).
How the macros fit together is [the guide](https://github.com/ArcatureLabs/Arcature/blob/main/docs/src/SUMMARY.md).

## Status

Pre-release. `0.x` under Cargo's rules means a **minor** bump is the breaking
one, and Arcature uses it that way deliberately: `arcature-macros = "0.1"` will
not carry you to `0.2`. Read the
[changelog](https://github.com/ArcatureLabs/Arcature/blob/main/CHANGELOG.md)
before a minor bump.

`0.1.0` is this crate's first release. The `2026.x` versions under the
`arcature*` names on crates.io are from an abandoned predecessor repository and
are yanked; `arcature-macros` was never part of it.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
