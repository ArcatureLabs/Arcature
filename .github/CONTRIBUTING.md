# Contributing to Arcature

Arcature is one crate with an opinion. Contributions that sharpen the opinion
are welcome; contributions that turn the framework into a self-assembly kit are
not. If you are unsure which yours is, open an issue before writing the code.

## Requirements

- Rust `1.97.1` or newer. The repository pins a toolchain in
  `rust-toolchain.toml`, so `rustup` installs the right one on first build.
  `1.97.1` is also the MSRV declared in `Cargo.toml` and one half of the CI
  matrix; the other half is `stable`.
- PostgreSQL 17 for the tests that need a database. CI runs one as a service
  and sets `DATABASE_URL`; locally, set the same variable:

  ```sh
  export DATABASE_URL=postgres://postgres:postgres@localhost:5432/arcature_test
  ```

  No test in the suite connects to a database today -- the job-queue tests
  exercise the model, the retry policy and the worker configuration, not the
  queue itself. The service is in CI so that the tests which will need it do
  not have to be special-cased when they arrive.
- `cargo-hack` for the feature-matrix checks: `cargo install cargo-hack`.
- Optionally [`just`](https://github.com/casey/just), which wraps the commands
  below into the recipes in the `justfile`.

## Build, test, lint

These are the commands CI runs, in the order it runs them. Anything that fails
here fails the pull request. Note that CI sets `RUSTFLAGS: "-D warnings"`, so a
warning is an error there even though it is not one locally.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets
cargo build
cargo test
```

Then the feature matrix. Arcature has twenty-odd features and a build that only
ever compiles the default set will not notice when one of them stops standing
on its own:

```sh
# The kernel with nothing enabled.
cargo build --no-default-features

# Everything except the operator opt-ins.
cargo build --features fullstack

# No database, no dx.
cargo build --no-default-features --features "macros,inertia,auth,validation,cache,storage-fs,mail,jobs,events,api,observe,pages,realtime,cli,templates"

# Every feature on its own. Fails fast and names the culprit.
cargo hack check --each-feature --no-dev-deps

# The thorough one: slow, --keep-going, easy to misread. Run it before a
# release, not before every commit.
cargo hack build --feature-powerset --skip database --keep-going
```

And the packaging check, which is what catches a `Cargo.toml` that would not
publish:

```sh
cargo publish --dry-run --no-verify
```

With `just` installed, `just check`, `just fmt`, `just lint`, `just test`,
`just features` and `just docs` cover the same ground.

## What a change should look like

**One file, one responsibility.** The codebase is arranged that way throughout
(`src/jobs/claim.rs`, `src/jobs/complete.rs`, `src/jobs/worker.rs`, and so on).
A new concern gets a new file rather than another five hundred lines in an
existing one.

**No hidden registry.** No `inventory`, no `linkme`, no `TypeId` to `Any` map,
no thread-locals, no global mutable state. Metadata is `&'static` const wiring
emitted by macros, and wiring is explicit. See
`docs/decisions/0005-no-hidden-registry.md`.

**No new npm package.** Arcature publishes no JavaScript. If your change wants
to hand something to the browser, generate a `.ts` file on disk. See
`docs/decisions/0001-no-npm-package.md`.

**Document the cost.** The module headers in this repository state what a
module owns, what it deliberately does not own, and what the chosen approach
gives up. Match that. A doc comment that only restates the function signature
is not worth the line.

**Tests pin behaviour that fails silently.** `tests/routing.rs` exists because
per-route middleware once wrapped sibling routes; `tests/application.rs` exists
because the pipeline order was a comment nobody could observe. If your change
fixes something that failed quietly, the test is the point of the change.

**`unsafe` is forbidden.** `#![forbid(unsafe_code)]` is set at the crate root
and in `Cargo.toml`. There is no exception process.

That covers this crate and not the several hundred beneath it, which is where
the `unsafe` actually is. `baselines/unsafe-baseline.<host-target>.txt` records the count
per crate; `just geiger` diffs the current graph against the file for this
host, and `just geiger-accept` records a new answer. A pull request that
changes a dependency is expected to say what moved and why it is acceptable.
On your machine that diff is a question rather than a gate; on Linux it is
both, because `.github/workflows/geiger.yml` runs the same command.
`just geiger` checks the whole graph through its own `rustc` wrapper and into
the ordinary `target/`, so it costs minutes and leaves the next `cargo check`
rebuilding from cold. It is a before-you-open-the-pull-request command, not a
before-every-commit one. The reading is per host target -- the platform crates
near the leaves differ by OS -- which is why the filename carries the target
and why a host with no baseline yet gets one recorded rather than a diff full
of noise. The Linux file is the one CI enforces, because Linux is what the
generated application deploys to; the others are conveniences for whoever
works on that host.

## The roadmap board

Every issue opened here lands on the
[roadmap board](https://github.com/orgs/ArcatureLabs/projects/2) by itself.
You do not have to add it, and you should not have to remember to: a board
that is only as current as the last time someone dragged something onto it
is a tracker that quietly lies about what is outstanding.

Two things it deliberately does not do. It does not set Status, Priority,
Area or Proof -- those are triage judgements, and a default nobody chose
reads exactly like a decision somebody made. And it does not add pull
requests, which live and die inside a week and would bury the work items
the board exists to show.

The automation is the project's own built-in *Auto-add to project*
workflow, filtered to `is:issue`. It runs inside GitHub with no credential
of any kind. An earlier version of this was a repository workflow calling
`actions/add-to-project`, which needs a personal access token with
`project` scope, because the default `GITHUB_TOKEN` cannot write to a
Projects v2 board at all -- `project` is not among the permissions it can
be granted. Trading a long-lived token for a setting was the better deal.

## Branches and pull requests

- Branch from `main`. Name the branch after the work: `feat/inertia-deferred-props`,
  `fix/csrf-bearer-exemption`, `docs/deployment-chapter`.
- Keep a pull request to one subject. Mechanical changes (a `cargo fmt` sweep, a
  rename) go in their own commit, and preferably their own pull request, so they
  do not hide inside a behaviour change.
- CI must be green. Do not disable a lint to get there; if a lint is wrong, say
  why in the pull request.
- A pull request that changes behaviour needs a test that fails before it and
  passes after. A pull request that changes public API needs the doc comment
  updated in the same commit.
- Rebase rather than merge `main` into your branch. The history here is linear.

## Commit messages

Conventional-commit prefix, then a subject line, then prose. The body explains
**why the change exists and what it costs** — not a bullet list of the files
touched, which the diff already shows.

```
fix: make module! compilable and give the builder a layer installation path

Two defects that between them made a scaffolded app unable to serve its own
home page.

`#[controller]` validated its impl block and re-emitted it unchanged, while
`module!` unconditionally emitted `<Ctrl as ControllerMetadata>::METHODS` for
every name in `controllers:`. Any real `module!` therefore failed to compile,
which is why the scaffold template used no DSL at all.

...
```

Rules that fall out of that:

- Prefixes in use: `feat`, `fix`, `test`, `docs`, `style`, `chore`, `refactor`.
- Subject line in the imperative mood, lower case after the prefix, no trailing
  period, under about 72 characters.
- Wrap the body at 72 columns.
- Name the defect, not the symptom. "The dev proxy could only be switched on by
  an explicit builder call the scaffold does not make, so Vite requests would
  have 404'd" is worth more than "fixed dev proxy bug".
- State deliberate deviations from the obvious approach and why. Several commits
  in this history have a "two deviations from the plan, both deliberate"
  paragraph; that is the shape to aim for.
- Reference the test that pins the new behaviour by name where one exists.

## Releases and versioning

Arcature uses semantic versioning, currently `0.1.1`.

- `MAJOR` increments on a breaking change, and stays `0` until the API is
  frozen.
- `MINOR` increments on a compatible addition -- and, while `MAJOR` is `0`, on
  a breaking change too, because Cargo treats the leftmost non-zero field as
  the major.
- `PATCH` increments on a compatible fix.

So under `0.x` the release that breaks something is a minor bump, and it needs
a `### Removed` or `### Changed` entry naming the replacement.

`arcature` and `arcature-macros` are on crates.io and publish together, the
macro crate first — the standard `serde` / `serde_derive` ordering, because
`arcature` depends on an exact version of it. Nothing publishes from a laptop:
a version tag starts `.github/workflows/release.yml`, which mints a
short-lived credential through crates.io Trusted Publishing. The operator's
walkthrough, including what to do when a step fails between the two crates, is
[`docs/RELEASING.md`](../docs/RELEASING.md).

Record every user-visible change in `CHANGELOG.md` under `## [Unreleased]` as
part of the pull request that makes it.

## Reporting security issues

Do not open a public issue. See `SECURITY.md`.

## Licence

By contributing you agree that your contribution is licensed under Apache-2.0,
the licence in `LICENSE`.
