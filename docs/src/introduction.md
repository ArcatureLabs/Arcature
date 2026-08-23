# The Arcature Guide

Arcature is a full-stack web framework for Rust. One crate, batteries included:
HTTP routing, the Inertia protocol, a database layer, authentication,
validation, background jobs, events, cache, storage, mail, and the `arc`
command-line tool. One dependency line is the whole install -- see
[Getting started](getting-started.md), and the **Status** section below for
what "one crate" currently buys you.

It is opinionated. It integrates proven components — Axum, Tower, Tokio,
SeaORM, SQLx, OpenDAL, lettre, tracing — and owns what sits between them: the
application lifecycle, the request pipeline, the conventions, and the
vocabulary. Where the opinions run out, the underlying crates are re-exported
and reachable.

## What this guide assumes

That you know Rust, and that you have written a web application before in
something. It does not re-teach async, and it does not explain what a migration
is. It does explain what Arcature does differently from what you would expect,
and why.

## How to read the examples

Every code sample in this guide was written against the code as it exists, not
against the API as it is planned. Where a feature is documented but not built,
the chapter says so under a **Not yet implemented** heading and shows what
works today instead. A missing example costs you a search; a wrong one costs
you an afternoon.

Samples elide `use` statements when the prelude covers them:

```rust,ignore
use arcature::prelude::*;
```

## Status

`arcature 0.1.1` is on crates.io. `0.1.0` was the first release of this
codebase, there is no upgrade path from anything earlier, and the API is still
moving. `0.1.1` removes nothing and changes no signature, so `arcature = "0.1"`
carries you to it; it adds fourteen subsystems, all behind feature flags that
are off by default. Read
[the changelog](https://github.com/ArcatureLabs/Arcature/blob/main/CHANGELOG.md)
and [the upgrade note](upgrade.md) before relying on anything here.

The `2026.x` versions under the `arcature*` names on crates.io come from an
abandoned predecessor repository. They are yanked and share nothing with this
codebase but the name.

Versioning is semantic, starting at `0.1.0`. Under `0.x` the fields shift one
place left: a minor bump is the breaking one, a patch bump is compatible, and
Cargo reads them that way too, so `arcature = "0.1"` will not silently carry
you to `0.2`.

## Where the reasoning lives

Doc comments explain what a type does. This guide explains how the pieces fit.
[Decisions](decisions.md) explains why some of them are shaped in ways that
will surprise you: no npm package, one TCP port in development, a fixed layer
order, no hidden registry.
