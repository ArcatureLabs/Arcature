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
