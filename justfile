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

# Run the test suite.
test:
    cargo test

# The database tests read `ARCATURE_TEST_DB_URL`, never `DATABASE_URL`: a
# developer's working `DATABASE_URL` is exactly the value that must not reach
# a suite which writes to what it finds. Unset, they skip, so `just test`
# stays green with no server running; set, they run. CI additionally sets
# `ARCATURE_REQUIRE_TEST_DB=1`, which turns the skip back into a failure, so a
# leg whose database never started cannot skip its way to green.
#
# The driver is a build-wide choice, so this is one build per dialect rather
# than one build with three. Point each URL at a database whose name starts
# with `arcature_test_`; the harness refuses anything else, because these
# tests write to what they are given.

# Run the suite against whichever databases are configured, one build per driver.
db-test:
    #!/usr/bin/env bash
    set -euo pipefail
    for pair in "db-postgres:${ARCATURE_TEST_DB_URL_POSTGRES:-}" \
                "db-mysql:${ARCATURE_TEST_DB_URL_MYSQL:-}" \
                "db-sqlite:${ARCATURE_TEST_DB_URL_SQLITE:-}"; do
        driver="${pair%%:*}"
        url="${pair#*:}"
        if [ -z "$url" ]; then
            echo "== $driver: skipped (set ARCATURE_TEST_DB_URL_$(echo "${driver#db-}" | tr a-z A-Z))"
            continue
        fi
        echo "== $driver =="
        ARCATURE_TEST_DB_URL="$url" ARCATURE_REQUIRE_TEST_DB=1 \
            cargo test --no-default-features --features "jobs,session-store-db,test-kit,$driver" --lib --test test_kit
    done

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
#   --depth 2                the crate has 41 features. An uncapped powerset
#                            is a six-figure number of builds -- not slow,
#                            unrunnable, and the recipe never returned.
#                            Depth 2 is 696, and pairwise is where
#                            feature-interaction bugs actually live: a
#                            feature that fails alone is caught by
#                            --each-feature above, and one that fails only
#                            in a specific trio is rare enough not to be
#                            worth three orders of magnitude. Raise it to 3
#                            (7,277 builds) when chasing one.
#
# The two counts are measured, never arithmetic. cargo-hack drops a
# combination in which one feature already enables another, so the powerset is
# far smaller than 2^n and the pruning is not something to work out on paper:
#   cargo hack build --feature-powerset --depth 2 --features db-postgres \
#     --skip database,db-sqlite,db-mysql --exclude-all-features --keep-going \
#     --print-command-list | grep -c .
# enumerates without compiling and answers in seconds.

# In CI the two lines below are separate jobs: `--each-feature` runs on every
# pull request, the powerset runs on a nightly schedule. 696 builds is too much
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
    feats=api,api-docs,api-tokens,auth,auth-flows,auth-remember,auth-reset,cache,cli,crypt,database,dev-proxy,dx,events,i18n,inertia,jobs,macros,mail
    feats=$feats,notifications,notifications-broadcast,notifications-db,notifications-queue,oauth,observe,otel,pages,realtime
    feats=$feats,session-store-db,signed-urls,storage-fs,storage-s3,templates,test-kit,uag,uploads,validation,views
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

# The unsafe-code inventory of the dependency tree.
#
# `#![forbid(unsafe_code)]` at the top of `src/lib.rs` is a claim about one
# crate. It says nothing about the several hundred underneath it, and that is
# where the `unsafe` actually is -- in the allocator, in tokio's reactor, in
# the TLS stack. Refusing to write `unsafe` is not the same as not depending
# on it, and a security policy that runs the two together is telling itself a
# story.
#
# `cargo geiger` counts what is there: unsafe functions, expressions, impls,
# traits and methods per crate, each as `used/total` -- what this build's
# feature set actually reaches, over what the package contains.
# `unsafe-baseline.<host-target>.txt` is the last accepted answer for a given
# target, and this recipe diffs a fresh report against the one for this host.
#
# A difference is not a failure. It is a question for the pull request that
# caused it: a dependency bump that brings a new `unsafe` block into the graph
# should be seen by a person rather than averaged into a total. When the answer
# is "yes, that is fine", `just geiger-accept` records the new number.
#
# Needs `cargo install cargo-geiger`. Note that geiger builds the whole graph
# with its own compiler wrapper, so the first run is a cold build and does not
# share `target/` with anything else here.

# Diff the dependency tree's unsafe-code counts against the recorded baseline
# for this host target.
#
# The baseline is named after the target because the reading is target-shaped:
# near the leaves the graph is platform-specific, so a Windows report and a
# Linux report differ by dozens of crates that have nothing to do with anyone's
# change. One file per target lets each host compare like with like, and lets
# CI hold a baseline for the target the application actually deploys to
# without overwriting the one a developer records locally.
#
# The report lands in `unsafe-report.txt` (gitignored) and is kept only when
# the diff fails. Regenerating it is another cold build, so the run that just
# told you something moved should not also throw away the evidence -- accepting
# the change is `just geiger-accept`, and CI uploads the same file as an
# artifact for exactly this reason.
#
# A missing baseline for this target is not a failure of the check; it is the
# check having nothing to compare against. It records one and says so.
geiger:
    #!/usr/bin/env bash
    set -euo pipefail
    baseline="unsafe-baseline.$(rustc -vV | sed -n 's/^host: //p').txt"
    just _geiger-report unsafe-report.txt
    if [ ! -f "$baseline" ]; then
        # Copy rather than move. The two files are identical here, but a CI
        # run bootstrapping a new target hands its evidence back by uploading
        # unsafe-report.txt -- moving it leaves that run with nothing to
        # upload, so the one run that records a target's first baseline would
        # be the one that cannot give it to you.
        cp unsafe-report.txt "$baseline"
        echo "no baseline for this target yet -- recorded $baseline" >&2
        echo "review it and commit it; the diff starts meaning something after that" >&2
        exit 1
    fi
    if diff -u "$baseline" unsafe-report.txt; then
        rm -f unsafe-report.txt
        echo "unsafe counts match $baseline"
    else
        echo "counts moved against $baseline -- fresh report kept at unsafe-report.txt" >&2
        exit 1
    fi

# Record the current unsafe-code counts as the accepted baseline for this host.
geiger-accept:
    #!/usr/bin/env bash
    set -euo pipefail
    baseline="unsafe-baseline.$(rustc -vV | sed -n 's/^host: //p').txt"
    if [ -f unsafe-report.txt ]; then
        mv unsafe-report.txt "$baseline"
    else
        just _geiger-report "$baseline"
    fi
    echo "recorded $baseline"

# The report itself. One place, so the baseline and the diff cannot drift
# apart by using different flags.
#
# `--forbid-only` is deliberately not used: it answers "does this crate write
# `#![forbid(unsafe_code)]`", which most crates do not bother to, and that is
# a different and much weaker question than "how much unsafe is in here".
_geiger-report out:
    #!/usr/bin/env bash
    set -uo pipefail
    # Fetch first, or cargo-geiger 0.13.0 panics inside its vendored cargo:
    # `assertion failed: self.pending_ids.insert(id)`, reached only from the
    # code path that *downloads* a package it could not match. This lockfile
    # has four such packages -- rkyv, rkyv_derive, borsh and borsh-derive,
    # optional dependencies of rust_decimal that no feature turns on, so they
    # sit in Cargo.lock and in no build graph. A warm registry cache skips
    # that path entirely, which is why the bug is invisible on a machine that
    # has built this tree before and fatal on a fresh CI runner. `cargo fetch`
    # puts every lockfile entry on disk, including those four, so the report
    # downloads nothing. On a warm cache it costs about a second.
    cargo fetch --locked

    # `--color never` because this output is a file to be diffed, not a
    # terminal to be read. cargo-geiger colours the `!` and `:)` markers when
    # it believes something is watching, and a GitHub runner qualifies: the
    # first Linux report carried 831 escape sequences and the Windows one
    # none, which would have rendered every cross-host comparison as though
    # the entire tree had changed.
    cargo geiger --all-targets --output-format Ascii --color never > "{{ out }}"
    status=$?

    # cargo-geiger exits non-zero when it finishes the scan but could not read
    # some file in the tree: READMEs, JSON, `.gitkeep`, ICU `.rs.data` blobs.
    # It counted 283 of those on Linux and not one of them is a fact about
    # `unsafe`. The exit code therefore conflates "the scan failed" with "this
    # tree contains files that are not Rust", and only the first of those is a
    # reason to stop.
    #
    # So the report is validated rather than the status. A run that produced
    # the column header and a totals line did the work it was asked to do,
    # whatever it exited with; a run that did not is a real failure and still
    # fails, carrying the status with it. Checking the artifact is also the
    # stronger test -- `|| true` would have accepted the panicking run that
    # wrote an empty file.
    if grep -q '^Functions  Expressions' "{{ out }}" \
        && tail -n 5 "{{ out }}" | grep -qE '[0-9]+/[0-9]+'; then
        if [ "$status" -ne 0 ]; then
            echo "note: cargo-geiger exited $status after a complete scan;" >&2
            echo "      that is its answer to unreadable non-Rust files, not to unsafe" >&2
        fi
        exit 0
    fi
    echo "cargo-geiger exited $status without producing a usable report" >&2
    exit 1

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
