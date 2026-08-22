# Upgrading

## Upgrading from 0.1.0 to 0.1.1

Nothing to do. `0.1.1` removes nothing and changes no public signature, so if
your manifest asks for `0.1` you already accept it:

```toml
[dependencies]
arcature = { version = "0.1", features = ["fullstack"] }
```

`cargo update -p arcature` takes it. Read the rest of this section only if you
want the new subsystems, or if you render pages through a `PageContract`, or
if you rate-limit by IP.

### Two behaviours change

Both are listed in the changelog, and both are the kind of change that does
not appear in a compiler error.

**A page rendered through a `PageContract` now titles itself.** Where the
`<title>` used to be the application title on every route, a page reached
through `render_page` now derives one from its contract name. This only
reaches applications using a `#[page]` contract *and* one of the stock root
documents; a hand-written `Fn(ScriptBody) -> String` root document ignores the
head, as it always has, and `Inertia::render` is unaffected. If you were
relying on one title everywhere, set the head explicitly with
`Inertia::with_head`.

**The IP rate limiter now keys on the caller.** `KeySource::Ip` previously had
no client address to read and put every request into a single shared bucket,
which meant a limiter configured per-IP was in practice a global one. It now
resolves a real address. Expect the limiter to start doing what its
configuration always said: if a limit was set per-IP and tuned against the
global behaviour, the effective ceiling is now that limit times your caller
count, so re-check the number. Addresses from `X-Forwarded-For` are trusted
only from peers you list with `ApplicationBuilder::trusted_proxies`, and the
default list is empty -- behind a reverse proxy, the address is your proxy's
until you say otherwise.

### Two new feature flags

Both are off by default. Neither changes an existing build.

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `uploads` | `multipart/form-data` bodies, filename sanitizing, content-addressed object names, bounded readers, magic-byte content sniffing, attachment downloads | An upload endpoint is the largest attacker-authored surface an application has. A build with no upload route should not carry a multipart parser. |
| `session-store-db` | A sqlx-backed `SessionStore`, so sessions survive a restart instead of logging every user out on deploy | It needs a database and a migration. An application that has neither should not be made to have them. |

`session-store-db` needs its table created before first use. Apply
`arcature_sessions` the way you apply the job migrations; a project generated
by `arc new` on this version already wires it into `--migrate`.

### Following `main` instead

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

These are the parts most likely to move before `1.0`, so that nobody builds
on them by accident:

- **`AppConfig`** carries `APP_NAME`, `APP_URL`, `APP_ENV` and `APP_PORT`.
  `APP_PORT`, `APP_URL` and `APP_NAME` are read by the framework as of
  `0.1.1`; `APP_ENV` is deliberately read by nothing, because a protection
  an operator can switch off with an environment variable is not one.
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
