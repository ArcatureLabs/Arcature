# Releasing Arcature

This is the operator's copy of `.github/workflows/release.yml`. The workflow
is what actually publishes; this page says what a person has to do around it,
in what order, and what to do when a step fails halfway.

Read the whole page before the first release you run. Publication is
irreversible -- crates.io has no delete, only [yank], which withdraws a
version from new resolution and leaves every existing lockfile resolving
exactly as before. A wrong version number is not something you fix, it is
something you supersede.

[yank]: https://doc.rust-lang.org/cargo/commands/cargo-yank.html

## What publishes, and what triggers it

Two crates, in a fixed order:

1. `arcature-macros` -- the proc macros. Rust requires them in a crate of
   their own; nothing outside this repository should depend on it directly.
2. `arcature` -- everything else. It depends on
   `arcature-macros = { path = "macros", version = "=<same version>" }`, an
   **exact** requirement, so the macro crate has to be on crates.io before
   this one can be verified against it.

Both go up from the `publish` job in `release.yml`, and that job runs on one
trigger: **pushing a tag matching `v[0-9]+.[0-9]+.[0-9]+`**. There is no
manual `cargo publish` in this project and no registry token in anybody's
`~/.cargo/credentials.toml`. The job mints a short-lived credential through
crates.io Trusted Publishing: GitHub issues an OIDC id-token, crates.io
exchanges it for a token scoped to that one run, and the action revokes it
when the job ends. Publishing from a laptop would bypass all of that, and is
not the path.

A `workflow_dispatch` run rebuilds the `arc` binaries for an existing tag and
deliberately does **not** republish.

## Before you tag

Everything in this section is local and reversible. Nothing leaves the
machine.

### 1. Pick the number

Arcature is in `0.x`, where Cargo reads the leftmost non-zero field as the
major. So the **minor** is the breaking bump and the **patch** is the
compatible one:

| The release | Bump | Requires |
|---|---|---|
| Additions and fixes only | `0.1.0` -> `0.1.1` | Nothing removed, no signature changed, no default changed |
| Anything breaking | `0.1.x` -> `0.2.0` | A `### Removed` or `### Changed` entry naming the replacement |

"No default changed" is the clause people get wrong. A fixed bug that alters
what an unchanged application does is still a behaviour change. It may ship in
a patch release, but it belongs in `### Fixed` **with the change spelled out**
rather than buried in prose about the fix. If a reader upgrading by patch
could be surprised, the entry is not finished.

### 2. Fold the changelog

`## [Unreleased]` becomes `## [X.Y.Z] - YYYY-MM-DD`, and a fresh empty
`## [Unreleased]` goes back above it. Then update the two link references at
the bottom of the file:

```
[Unreleased]: https://github.com/ArcatureLabs/Arcature/compare/vX.Y.Z...HEAD
[X.Y.Z]: https://github.com/ArcatureLabs/Arcature/compare/vPREV...vX.Y.Z
```

Keep the section order Keep a Changelog specifies -- Added, Changed,
Deprecated, Removed, Fixed, Security -- and omit sections rather than writing
them empty. This project appends Performance and Documentation after Security.

Watch the anchor when moving entries between sections: `### Added`, `### Fixed`
and `### Security` each occur once per release, so a search for a bare heading
matches the released sections too. Anchor on the first heading *after* the
release you are editing.

If an open issue is not closed by this release, say so in the release preamble
and say why. A changelog that lists only wins is a marketing page.

### 3. Bump the version in four places

```
Cargo.toml          version = "X.Y.Z"                            # the package
Cargo.toml          arcature-macros = { ..., version = "=X.Y.Z" }
macros/Cargo.toml   version = "X.Y.Z"
Cargo.lock          both entries -- through cargo, not by hand
```

Regenerate the lock file rather than editing it:

```
cargo update --workspace --offline
```

The `verify` job re-derives all three manifest numbers from `cargo metadata`
and fails the release if the tag, the package and the macro crate disagree.
It cannot fail *helpfully* once a tag is public, though, so check here.

The scaffold needs no edit. `arc new` writes `__ARCATURE_VERSION__` into the
generated `Cargo.toml`, and `src/templates/render.rs` substitutes the running
crate's version at generation time.

Prose that names the old number is not caught by anything. Grep for it:

```
grep -rn '0\.1\.0' README.md docs/src/ .github/CONTRIBUTING.md
```

### 4. Run the gate

`just` is a convenience, not the contract. On a machine where an
application-control policy blocks unsigned binaries it will not run at all,
and the release must not depend on it. The underlying commands are these:

```
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo deny check
cargo publish --dry-run --no-verify
cargo publish --dry-run --no-verify --manifest-path macros/Cargo.toml
```

Then once per off-by-default feature that has tests of its own, because the
default run does not compile them:

```
cargo clippy --no-default-features --features "<feature>,db-postgres" --all-targets -- -D warnings
cargo test --features "<feature>" --no-fail-fast
```

`--no-fail-fast` is not optional. Without it `cargo test` stops at the first
failing binary, so one failure -- including an environmental one -- leaves the
remaining binaries and **every doctest** unrun. The output then looks like a
short green run with one red line rather than like the coverage gap it is.

Do not run `cargo test --all-features` or `cargo clippy --all-features`. The
three database drivers are mutually exclusive by design and `--all-features`
turns on all three; the resulting compile errors are the invariant working,
not a regression. The honest equivalent is one `cargo check` per driver over
every other feature, which is what `just drivers` spells out.

A red gate is worth one sanity check before you go looking for the bug. Two
failure modes here look exactly like broken code and are not: a disk below
roughly 8 GB fails at link time, and a code-integrity policy can reject a
freshly linked test binary until it is relinked. Re-run the single failing
target before reading any source.

### 5. Commit

One commit, `chore: release X.Y.Z`, carrying the version bumps and the lock
file and nothing else. The changelog fold, the upgrade note and any prose
corrections are separate commits before it. This is the commit people read
first when a release goes wrong, and it should be readable at a glance.

## Tagging and pushing

```
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin main
git push origin vX.Y.Z
```

Push the branch **before** the tag. The tag is what starts the publish, and a
tag arriving at a remote whose `main` does not yet contain it produces a
release nobody can find the source for.

Use an annotated tag (`-a`) rather than a lightweight one: it carries an
author and a date, and `git describe` prefers it.

## What the workflow does, in order

| Job | What it proves |
|---|---|
| `verify` | The tag, `arcature` and `arcature-macros` all name the same version. Then every category slug in both manifests is checked against the crates.io category API. |
| `publish` | `cargo test --locked` against a live PostgreSQL, then mint a token, publish `arcature-macros`, wait for the index to serve it, publish `arcature`. |
| `binaries` | Cross-builds `arc` for each release target and uploads the archives as artifacts. |
| `release` | Creates or updates the GitHub release and attaches the archives. |

The category check exists because crates.io validates slugs **server-side at
the end of an upload**, after packaging and a full verify build have already
passed -- and, since the two crates go up in sequence, potentially after the
macro crate is published and unrecallable. That is how `0.1.0` first failed,
on the word `framework`. Neither `cargo package` nor `cargo publish --dry-run`
catches it, which is why it is a separate job that runs before anything is
sent.

## When a step fails

**Before `publish` ran.** Nothing left the machine. Delete the tag locally and
on the remote, fix, re-tag:

```
git push origin :refs/tags/vX.Y.Z
git tag -d vX.Y.Z
```

**After `arcature-macros` published, before `arcature` did.** Do not bump the
version. Both publish steps skip a version that is already on crates.io, so
the job is safe to re-run: fix what failed and re-run the workflow from the
Actions tab. That skip is what keeps a half-published release recoverable --
without it a re-run dies on "crate version already exists", and the only way
out is burning a version number.

**After both published.** The release is out. If it is broken, yank it and
ship the fix as the next patch:

```
cargo yank --version X.Y.Z arcature
cargo yank --version X.Y.Z arcature-macros
```

Yank withdraws the version from new resolution. It does not remove the files,
does not break existing lockfiles, and is not a substitute for a fix.

## After the release

- Confirm the index serves both crates:
  `curl -s https://index.crates.io/ar/ca/arcature | tail -1`
- Sanity-check resolution from a clean project. This is the only check that
  exercises what a user actually types:

  ```
  cargo new /tmp/arcature-smoke && cd /tmp/arcature-smoke
  cargo add arcature@X.Y.Z --features fullstack
  cargo build
  ```

  Do it in a directory with no `[patch.crates-io]` above it. Inside a clone of
  this repository, the patch that `just scaffold` appends would resolve
  `arcature` to the working tree and prove nothing about the registry.
- Check that docs.rs built: <https://docs.rs/arcature>. A docs.rs failure does
  not fail the release workflow, so nothing tells you but looking.
- Confirm the issues the release closes are closed. `Closes #N` trailers fire
  when the commit reaches the default branch, so commits that sat unpushed on
  a local `main` close their issues at push time rather than at commit time --
  and a release pushing dozens of commits at once is exactly when that is easy
  to miss.
