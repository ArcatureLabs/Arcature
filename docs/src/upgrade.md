# Upgrading

## One release so far

`arcature 0.1.0` and `arcature-macros 0.1.0` are on crates.io. There is
nothing to upgrade *from* yet, so this chapter is the scheme rather than a
history: read it before the second release, not after.

```toml
[dependencies]
arcature = { version = "0.1", features = ["fullstack"] }
```

`main` moves ahead of the release and breaks without notice. To follow it,
depend by git reference and pin a revision -- a branch reference will move
under you.

```toml
[dependencies]
arcature = { git = "https://github.com/ArcatureLabs/Arcature", rev = "...", features = ["fullstack"] }
```

## The version scheme: semantic versioning

Arcature is versioned `MAJOR.MINOR.PATCH`, starting at `0.1.0`.

| Field | Increments when |
|---|---|
| `MAJOR` | Something breaks: a removed API, a changed signature, a changed default behaviour. Stays `0` until the API is frozen. |
| `MINOR` | A compatible addition -- and, while `MAJOR` is `0`, a breaking change too. |
| `PATCH` | Something is fixed compatibly. |

The current version is `0.1.0`, readable at runtime as
`arcature::FRAMEWORK_VERSION`.

The row that matters is the middle one. Cargo treats the leftmost non-zero
field as the major, so under `0.x` the minor *is* the breaking field, and the
usual requirement already accounts for it:

```toml
arcature = "0.1"
```

resolves `0.1.1` and `0.1.9` but refuses `0.2.0`. An exact pin buys nothing
here, and costs you the patches. Raise the minor deliberately, with the
changelog open.

What `0.x` is telling you is that the public API is not frozen: any minor
release before `1.0` may remove or reshape something. The parts most likely
to move are listed further down this page. Once `1.0` is tagged, the breaking
field moves back to the major and `arcature = "1"` becomes the safe
requirement.

## Where breaking changes are recorded

[`CHANGELOG.md`](https://github.com/ArcatureLabs/Arcature/blob/main/CHANGELOG.md)
in Keep a Changelog format. Every breaking release gets a `### Removed` or
`### Changed` entry naming the API and the replacement. If a change requires
work in an application, the entry says what the work is.

## The macro crate moves with the framework

`arcature-macros` is versioned in lockstep with `arcature` and is not a
separate upgrade decision. `arcature` depends on an exact version of it, and
the release workflow publishes `arcature-macros` first for that reason. Do not
depend on `arcature-macros` directly.

## What is likely to break

These are the parts most likely to move before a first release, so that
nobody builds on them by accident:

- **`AppConfig`** parses four environment variables that nothing reads.
- **`arcature::test_kit`**, **`uag`** and **`oauth`** are the youngest
  subsystems and the least exercised by real applications, so their surface is
  the most likely to move.

Anything still unbuilt is marked "Not yet implemented" in the chapter that
would otherwise document it. Nothing in this guide shows an example that does
not compile today.

## Upgrading the toolchain

`rust-toolchain.toml` pins `stable` with `rustfmt` and `clippy`. The MSRV is
`1.97.1` and CI builds on both it and current stable, so an MSRV bump is a
visible change to that file and to the CI matrix, not a silent consequence of
using a new language feature. Arcature uses edition 2024.
