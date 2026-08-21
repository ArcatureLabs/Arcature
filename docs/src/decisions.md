# Decisions

Some of Arcature's shapes are surprising enough to be worth a written record.
Each one states the decision, the context that forced it, and the cost paid.
They live in
[`docs/decisions/`](https://github.com/ArcatureLabs/Arcature/tree/main/docs/decisions)
in the repository.

- [**Arcature publishes no npm package**](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0001-no-npm-package.md).
  Applications use the official `@inertiajs/*` adapters. Everything the Rust
  side hands the browser travels as generated `.ts` files in the application's
  own tree, not through a framework runtime.
- [**The CSRF cookie is `XSRF-TOKEN`, not `__Host-csrf`**](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0002-xsrf-token-cookie.md).
  axios hard-codes those names, so the server moves to meet the client rather
  than making every application write a shim.
- [**Exactly one TCP port, in development as well as production**](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0003-one-tcp-port.md).
  Vite runs in `middlewareMode` over IPC and binds no port of its own.
- [**Layer order is a written contract**](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0004-layer-order-contract.md).
  The pipeline composes in a fixed, documented order, not in builder call
  order.
- [**There is no hidden registry**](https://github.com/ArcatureLabs/Arcature/blob/main/docs/decisions/0005-no-hidden-registry.md).
  No `inventory`, no `linkme`, no `TypeId` map, no thread-locals. All metadata
  is `&'static` const data emitted by macros and named by code you wrote.
