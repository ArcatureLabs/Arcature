# Arcature — the everyday commands.
#
# These mirror `.github/workflows/ci.yml`. If a recipe here and a step there
# disagree, CI is right and this file is the bug.

# Show the recipes.
default:
    @just --list

# Type-check the default feature set, tests included.
check:
    cargo check --all-targets

# Format every crate in the workspace.
fmt:
    cargo fmt --all

# The CI lint gate: formatting is checked, not applied, and a warning is fatal.
lint:
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings

# No test needs a live database today, but CI provides a postgres:17 service
# and a DATABASE_URL so that the ones that will do not have to be
# special-cased when they arrive.

# Run the test suite.
test:
    cargo test

# The feature matrix. `--each-feature` fails fast and names the culprit;
# the powerset is thorough, slow, and --keep-going, so read its tail.
# Needs `cargo install cargo-hack`.
#
# The flags are not tuning. They are the compile-time invariant in
# `src/database/mod.rs` -- a build speaks exactly one dialect, no more and no
# fewer -- written out as cargo-hack arguments:
#
#   --features db-postgres   Six features (`database`, `jobs`, `dx`, `uag`,
#                            `cli`, `api-docs`) pull `database` in without
#                            naming a driver, so on their own they hit the
#                            "needs a driver" error. The driver is a
#                            build-wide choice like a target, not something
#                            each feature opts into, so it is pinned for the
#                            whole matrix rather than excluded from it.
#   --skip db-sqlite,db-mysql  otherwise cargo-hack would add a second driver
#                            on top of the pinned one. The other two drivers
#                            are covered by `just drivers` instead, which
#                            gives each a full-breadth build of its own.
#   --exclude-all-features   `--all-features` is all three drivers at once.
#   --depth 2                the crate has 29 features. An uncapped powerset
#                            is 292,672 builds -- not slow, unrunnable, and
#                            the recipe never returned. Depth 2 is 263, and
#                            pairwise is where feature-interaction bugs
#                            actually live: a feature that fails alone is
#                            caught by --each-feature above, and one that
#                            fails only in a specific trio is rare enough not
#                            to be worth three orders of magnitude. Raise it
#                            to 3 (1,599 builds) when chasing one.

# In CI the two lines below are separate jobs: `--each-feature` runs on every
# pull request, the powerset runs on a nightly schedule. 263 builds is too much
# to put in front of a pull request but cheap enough to run once a night.

# Check every feature on its own, then all pairs. Needs cargo-hack.
features:
    cargo hack check --each-feature --no-dev-deps --features db-postgres --skip db-sqlite,db-mysql --exclude-all-features
    cargo hack build --feature-powerset --depth 2 --features db-postgres --skip database,db-sqlite,db-mysql --exclude-all-features --keep-going

# Every feature the crate has, once per database driver.
#
# This is the closest honest equivalent of `--all-features`, which cannot work
# here: a build speaks exactly one dialect, so "all features" is three builds
# rather than one. Anything that hard-codes a driver type, or writes SQL only
# one of them parses, fails here and nowhere else -- `just features` pins
# PostgreSQL and would never see it.

# Build the full feature set once per database driver.
drivers:
    #!/usr/bin/env bash
    set -euo pipefail
    feats=api,api-docs,auth,cache,cli,database,dev-proxy,dx,events,inertia,jobs,macros,mail
    feats=$feats,oauth,observe,otel,pages,realtime,storage-fs,storage-s3,templates,test-kit,uag,validation
    for driver in db-postgres db-sqlite db-mysql; do
        echo "== $driver =="
        cargo check --no-default-features --features "$feats,$driver" --all-targets
    done

# Build the API documentation for the full feature set.
docs:
    cargo doc --no-deps --features fullstack

# Build the guide. Needs `cargo install mdbook`.
book:
    mdbook build docs

# Serve the guide with live reload on http://localhost:3000.
book-serve:
    mdbook serve docs

# Dependency licences, advisories and bans. Needs `cargo install cargo-deny`.
deny:
    cargo deny check

# Generate an application with `arc new` and compile it.
#
# This is the check that four empty packages under `examples/` used to stand in
# for. The tree it compiles is the one the templates actually write, so a
# template change that does not compile fails here rather than in a user's
# first `cargo build`.
#
# The `[patch.crates-io]` append is temporary: `arc new` writes
# `arcature = "0.1.0"`, which does not resolve until the crate is published.
# Drop it after the first `cargo publish`.

# Scaffold every stack-and-driver combination and build it.
scaffold:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    cargo build --bin arc --features cli
    out="$(mktemp -d)"
    trap 'rm -rf "$out"' EXIT
    # One target directory for all nine, not nine. Each generated app depends
    # on the same `arcature` path with the same feature set bar the driver, so
    # a shared directory turns nine dependency builds into three -- one per
    # driver -- and the eight later `cargo build`s into near-nothing. CI does
    # not need this (its nine combinations are nine matrix jobs on nine
    # runners) but a laptop very much does: without it this recipe is hours.
    export CARGO_TARGET_DIR="$out/target"
    cd "$out"
    for stack in react vue svelte; do
        for db in sqlite postgres mysql; do
            echo "== $stack / $db =="
            name="app-$stack-$db"
            "$root/target/debug/arc" new "$name" --stack "$stack" --db "$db"
            {
                echo ""
                echo "[patch.crates-io]"
                echo "arcature = { path = \"$root\" }"
            } >> "$name/Cargo.toml"
            cd "$name"
            cargo build
            # Only SQLite runs the template's smoke test: it boots the
            # application, and the other two drivers would need a server.
            if [ "$db" = sqlite ]; then cargo test; fi
            cd ..
        done
    done

# Build with the oldest supported compiler.
#
# `rust-toolchain.toml` pins `stable`, and it outranks both rustup's default
# and any `rustup override` -- so asking for the MSRV has to be louder than the
# file. `RUSTUP_TOOLCHAIN` is, and unlike `cargo +1.97.1` it is inherited by
# whatever cargo invokes in turn. CI sets the same variable on its MSRV leg:
# installing the toolchain there is not enough, because the file still wins,
# and that leg was building with stable and proving nothing.

# Type-check with the oldest supported compiler, 1.97.1.
msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    RUSTUP_TOOLCHAIN=1.97.1 cargo check --all-targets

# The nightly powerset is deliberately absent from this recipe: CI runs it on a
# schedule rather than per pull request, so `just ci` does not either. Reach for
# `just features` when you want it.

# What CI runs, in CI's order. The last word before opening a pull request.
ci: lint
    cargo build
    cargo test
    cargo build --no-default-features
    cargo build --features fullstack
    cargo build --no-default-features --features "macros,inertia,auth,validation,cache,storage-fs,mail,events,api,observe,pages,realtime,templates"
    just drivers
    just scaffold
    cargo hack check --each-feature --no-dev-deps --features db-postgres --skip db-sqlite,db-mysql --exclude-all-features
    cargo deny check
    mdbook build docs
    cargo publish --dry-run --no-verify
