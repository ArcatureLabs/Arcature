# The dev loop

One number decides whether a framework is pleasant to work in: the time
between saving a Rust file and seeing the result in the browser. `arc dev`
exists to make that number small, and this page says what it currently is,
how it was measured, and where the time goes.

Numbers without a method are folklore, so the method is here in full and the
machine is named. Reproduce it before believing it.

## What one save costs

`arc dev` holds the TCP port itself and runs the application as a child
process, so a rebuild replaces only the child. The supervisor prints what each
part of the trip took:

```text
cargo 7.85s (check 2.73s, codegen+link 5.06s)  swap 0.21s  spawn 1.44s  boot 0.31s  total 9.81s
```

Those stages are:

| Stage | What it is |
|---|---|
| `cargo` | `cargo build --features dev`, spawn to exit. |
| `check` | Start of the build to the last non-executable artifact. |
| `codegen+link` | That boundary to the linked executable. |
| `swap` | Stopping the old process and staging the new binary. |
| `spawn` | Asking the operating system to start it. |
| `boot` | The new process starting to its first accepted IPC connection. |
| Vite | Nothing. A `.tsx`, `.vue` or `.css` edit never reaches this loop. |

`cargo` dominates, so it is what the measurement below isolates.

## How this was measured

1. `arc new demo --stack react --db postgres`, with `arcature` patched to the
   working tree so the framework under test is the one in the repository.
2. `cargo build --features dev` once, cold, to fill `target/`.
3. Change **one line** in `app/controllers/home_controller.rs` -- the string
   the welcome page renders -- and time `cargo build --features dev`. Three
   times, each with a different string, so no run can be answered from a
   previous run's cache.
4. A fourth run with `--timings`, for the per-unit breakdown and the
   fresh/dirty split.

`--features dev` is what `arc dev` itself runs, so this is the same build the
loop performs, not an approximation of it.

## The baseline

Measured 2026-08-21 on the machine described below.

| Measurement | Result |
|---|---|
| Cold build, empty `target/` | 52m 38s |
| `cargo build` with nothing to do | 4.1s |
| One-line handler change | **33.2s / 34.3s / 37.6s** |
| `demo.exe` | 18.8 MB |
| `demo.pdb` | 71.2 MB |

The `--timings` run breaks the rebuild into exactly two units of work out of
489 in the graph:

| Unit | Time | Of which |
|---|---|---|
| `demo` lib | 50.6s | frontend 6.9s, **codegen 43.7s** |
| `demo` bin | 39.4s | codegen of a nine-line `main.rs`, then the link |

Two dirty units, 487 fresh. That is the first thing the numbers settle: on a
Rust-only change **nothing is rebuilt that need not be**. Not `arcature`, not
`arcature-macros`, not the embedded scaffold templates, not a dependency. The
loop is not slow because it recompiles too much; it is slow because the two
units it does compile are expensive.

The second thing they settle is where inside those two units the time is.
Type-checking the application crate -- the part a developer thinks of as
"compiling" -- is 6.9 seconds of a 90-second trip. Everything else is code
generation and linking, and the 71 MB of debug information is why: every
frame of it has to be written by rustc, read by the linker, and merged into a
program database on each save.

### The machine

This is a small, busy machine, and the absolute numbers are worse than a
developer laptop would show:

- Windows 11, x86_64-pc-windows-msvc, **4 logical CPUs**.
- `rustc 1.98.0`, `cargo 1.98.0`.
- Microsoft Defender watching `target/`.
- Other Cargo builds running concurrently throughout. Cargo reported
  `Max concurrency: 1 (jobs=4 ncpu=4)` for the timed run, and that run took
  96.8s against 33-38s for the same work untimed -- a two-to-three times
  spread from contention alone.

Treat the absolute figures as an upper bound and the *shape* -- 5% frontend,
95% codegen and link, nothing spurious rebuilt -- as the finding. The shape is
what any change has to move.

Because the load varied, only measurements taken under `--timings` are
compared against each other below: those report per-unit compile time rather
than wall clock, and both the before and the after run reported the same
`Max concurrency: 1 (jobs=4 ncpu=4)`. Plain wall-clock series taken minutes
apart on this machine differ by more than any change being measured, and are
not used as evidence for anything.

## What was cut

The baseline points at one thing: debug information. Not the application's
own -- the scaffold has always built it with `line-tables-only` -- but its
dependencies'.

The instinct is that a dependency compiles once and then sits in `target/`,
so its profile is a one-time cost. That is wrong for generic code. Every
`Vec<MyThing>`, every `tokio` combinator, every `sea-orm` query builder used
with the application's own types is monomorphised *into the application's
crate*, and its debug information is emitted by rustc and merged by the
linker there -- on every save, for as long as the project exists. Nobody
steps through `tokio` while debugging a controller, so the scaffold now sets:

```toml
[profile.dev.package."*"]
opt-level = 2
debug = false

[profile.dev.build-override]
opt-level = 2
debug = false
```

Same machine, same application, same one-line change, both runs under
`--timings`:

| | Before | After |
|---|---|---|
| `demo` lib | 50.6s (frontend 6.9s, codegen 43.7s) | **25.5s** (frontend 4.9s, codegen 20.6s) |
| `demo` bin | 39.4s | **19.8s** |
| Both dirty units | 90.0s | **45.3s** |
| `demo.pdb` | 71.2 MB | **29.5 MB** |
| `demo.exe` | 18.8 MB | 18.8 MB |

Half, and the executable is byte-for-byte the same size, because none of this
was ever in it. Backtraces still carry file and line: the application's own
crates were never touched. A developer who wants a step debugger through a
dependency can have it for one run with
`CARGO_PROFILE_DEV_PACKAGE_tokio_DEBUG=2`.

Three levers that look obvious are not taken, and the manifests say why:

- `opt-level = 0` and a high `codegen-units` are Cargo's dev defaults.
  Writing them down changes nothing.
- `split-debuginfo` is target-specific. `rustc --print split-debuginfo`
  reports `packed` as the only stable value on `*-pc-windows-msvc`, which is
  what MSVC already does by writing a `.pdb`. A fixed value in the manifest
  would be a no-op for some developers and a hard error for others.
- A fast linker is already configured. `.cargo/config.toml` puts Windows on
  the toolchain's own `rust-lld.exe`, and leaves Linux and macOS on the
  system linker with `mold` and `wild` as commented opt-ins -- a config that
  fails on a machine without the tool is worse than a slow link.

## What is left

**The 2.5 second target is not met on this machine, and halving the cost was
not enough to meet it.** What remains, in order:

1. **Linking the executable.** Even with a third of the debug information,
   the `demo` bin unit is 19.8s for a `main.rs` of nine lines. Almost all of
   that is `rust-lld` pulling every rlib in the graph together. It is
   proportional to the size of the program, not to the size of the change,
   so it does not shrink as the diff shrinks.
2. **Code generation for the application crate**, 20.6s. This is
   monomorphisation: the application instantiates a large amount of generic
   machinery from `axum`, `tokio` and `sea-orm`, and each instantiation is
   compiled into this crate.

Type-checking -- 4.9s, and the only part proportional to what was actually
edited -- is already inside the budget. The loop is not slow because the
compiler is slow at understanding the change; it is slow because the whole
program is rebuilt around it.

Getting to 2.5s therefore needs a structural change rather than another
profile flag, and the candidates all have real costs:

- **Fewer generics crossing the boundary.** `-Zshare-generics` is nightly.
  Doing it by hand means erasing types at the framework's public edges, which
  trades compile time against the type safety the framework exists to
  provide.
- **A different codegen backend.** `rustc_codegen_cranelift` is dramatically
  faster at `-O0` and is nightly-only, x86-64 Linux first.
- **Not relinking at all.** Hot-patching the running process, as `subsecond`
  does, skips both remaining costs. It is a large piece of machinery and it
  does not survive every kind of change.

None of these is a patch-release change, so none of them is here. Issue #8
stays open with a measured number against it instead of a quoted one.
