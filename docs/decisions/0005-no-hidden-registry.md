# 0005 — There is no hidden registry

## Decision

Arcature has no global registry. Specifically: no `inventory`, no `linkme`, no
distributed slices, no life-before-main constructors, no process-wide
`HashMap<TypeId, Box<dyn Any>>` service locator, and no thread-locals or
task-locals holding framework state.

All framework metadata is `&'static` const data emitted by macros at the
definition site and gathered by explicit code the application wrote:
`ControllerMetadata::METHODS`, `RouteDescriptor`, `FieldShape`,
`ModuleDescriptor`, `JobModel`, `LISTENER_BINDING`, `PageContractEntry`.
`module!` names its controllers, jobs and listeners; `application!` names its
modules; `PageContracts::register` is called with the contract. Nothing
registers itself.

Dependency injection is `Resolve<S>`: a trait resolved at compile time against
the application's own state type. There is no container to look anything up in.

## Context

A registry is genuinely convenient. Write `#[job]` on a struct, have it appear
in the worker's dispatch table, never maintain a list. The convenience is real
and the costs arrive later, all of them at once:

- **The wiring is unreadable.** "Where is this handler registered?" has no
  answer in the source. The link step is the answer, and the link step is not a
  file anyone can open.
- **Registration is a build artifact.** A job vanishes because its module was
  optimised out, or because it lives in a crate nothing references, or because
  it was compiled into a `dylib`. The symptom is a job that silently never
  runs, which is the failure mode that costs the most to diagnose.
- **Nothing can be inspected without running it.** `arc doctor` and the graph
  validation want to answer "what does this application contain" as a
  side-effect-free question. A link-time registry only answers it by starting
  the process.
- **Tests share state.** A process-wide registry is shared by every test in the
  binary, and the first test that registers something changes the second test's
  world.
- **`TypeId`-keyed containers fail at runtime.** A missing dependency in a
  service locator is a panic on the request that needed it, in production, at
  the worst moment. The same mistake against `Resolve<S>` is a compile error.

Const wiring gives up the convenience and keeps everything else. `&'static`
data costs no allocation, needs no initialisation, is visible in the source
where it was written, and can be walked by a tool that never starts a server.
`ApplicationGraph` validates duplicates, unknown imports and cycles from that
data alone.

The one place `Any` appears is `RequestCache`, and it is the opposite shape: a
map that lives in the request's own extensions, keyed by resolver name rather
than by type, unreachable from anywhere except the request that owns it, and
dropped with it. The reasoning is in `src/dx/request_cache_store.rs`. A memo
store that outlives its request is not a performance bug; it is one user
reading another user's data.

`deny.toml` denies `inventory`, `linkme` and `ctor` outright, so this decision
fails the build rather than relying on review.

## Cost

**Wiring is manual and it is boilerplate.** Adding a job means writing the type
*and* naming it in `module!`. Forgetting the second step means the job does not
run, and the compiler will not tell you — the same silent failure a registry
was meant to prevent, arrived at from the other direction. It is a shorter
distance to the mistake and the mistake is in a file you can read, but it is
still a mistake available to make.

**Macros do more work.** Emitting const metadata that stays in step with the
types around it is harder than emitting a submission to a global slice, and it
is why every DSL macro carries error handling and an `ARC-M<NNN>` code rather
than assuming its input.

**No third-party extension by declaration.** A crate cannot add a route or a
job to an application by depending on Arcature and declaring one. It has to
export something the application names. That is the intended trade, and it is
still a capability given up.
