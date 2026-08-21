# 0001 — Arcature publishes no npm package

## Decision

Arcature ships Rust and nothing else. There is no `@arcature/*` package on npm,
no Vite plugin, no virtual module, no client runtime, and no CSRF shim.
Applications install the official Inertia adapters — `@inertiajs/react`,
`@inertiajs/vue3`, `@inertiajs/svelte` — and import from those.

Everything the Rust side needs to tell the JavaScript side travels as
**generated `.ts` files written into the application's own source tree**, under
`resources/js/generated/`, gitignored and regenerated from the page contracts.
A generated file is read by the TypeScript compiler and disappears at build
time. Nothing imports an Arcature package at runtime, because there is none.

## Context

The predecessor framework published `@arcature/client`: a Vite plugin, a
virtual module the plugin resolved, an action/query runtime that wrapped fetch,
and a CSRF shim that reconfigured axios. Each piece existed to serve another
piece. The plugin existed so the virtual module could exist; the virtual module
existed so the runtime could be imported without a path; the runtime existed so
the shim had somewhere to live. None of it was what an application asked for,
and all of it had to be versioned in lockstep with the Rust crate across two
registries with two release cadences and two dependency resolvers.

The failure mode is not that any single piece was bad. It is that a JavaScript
package published by a Rust framework accretes: once the channel exists,
everything the server would like the client to know starts arriving through it,
and the framework acquires a second public API it did not intend to have.

The Inertia protocol does not require any of it. Inertia is an HTTP contract —
a header, a JSON page object, an asset version. A stock client speaks it. The
only thing the predecessor's shim actually fixed was a cookie name, and a
cookie name can be changed on the server (see
[0002](0002-xsrf-token-cookie.md)).

## Cost

**Generated files live in the source tree.** A virtual module is invisible: it
is resolved by the bundler and never appears on disk. Generated `.ts` files
appear in `resources/js/generated/`, have to be gitignored, have to be
regenerated when contracts change, and will occasionally be stale when someone
edits Rust and does not rerun the generator. The staleness is visible — a type
error — rather than silent, which is why it is the cost worth paying, but it is
a real cost and the reader will meet it.

**No plugin means no bundler integration.** Arcature cannot hook Vite's module
graph, so it cannot invalidate generated types on change, and it cannot inject
configuration into `vite.config.ts`. Applications write those few lines
themselves.

**How the generator gets its input.** `src/inertia/contracts/` builds the
artifact it consumes — `PageContracts`, `PageSchema`, and
`ContractArtifact` with its `arcature.page-contract.v1` format — and the
pipeline carries that artifact in a request extension.
`arc typegen` emits `resources/js/generated/{routes.ts,pages.d.ts,forms.ts}`
from that artifact, and `arc dev` re-runs the same emitter after every
successful restart by reading the graph back out of the running application
over `GET /_arcature/uag.json`. No build script, no npm package, and no extra
binary in the dev loop.
