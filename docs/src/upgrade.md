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

### Fourteen new feature flags

Every one is off by default, and nothing was removed, so an existing build
compiles unchanged and links not a byte more. This list is the release: the
features are the release, and the two behaviour changes above are the only
things that happen without asking.

**Sign-in and credentials**

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `auth-flows` | `auth::flows` -- the decisions between `auth`'s seams and a login form that are wrong in ways nothing reports. An unknown address costs the same time as a wrong password; a failed attempt is throttled by address *and* caller | An application with no sign-in screen has no use for it, and it is the half of authentication where a plausible implementation leaks who has an account |
| `auth-reset` | One-time password-reset links: mailed once, redeemed once, stored as a SHA-256 digest, and issuing a new one invalidates the previous mail | Brings a table and a migration. An application whose accounts are provisioned by an administrator has no use for it |
| `auth-remember` | Rotating remember-me tokens, with the theft detection that makes a weeks-long credential defensible | Brings a table and a migration. "Stay signed in" is a product decision |
| `api-tokens` | Hashed personal access tokens -- an opaque bearer credential for a CLI, a CI job, another service. The database holds only a SHA-256 digest | Brings a table and a migration, and is independent of `auth`: an API with no passwords may still hand out a token |

**Cryptography**

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `crypt` | `crypt::Encrypter`: XChaCha20-Poly1305 over a versioned, self-describing token that refuses to return a single byte of altered ciphertext | The moment a build can produce ciphertext, somebody owns a key-rotation story |
| `signed-urls` | `crypt::UrlSigner`: a link carrying its own proof of origin and its own deadline, refused if edited by a byte | Separate from `crypt` because signing needs a MAC and encrypting needs a cipher. A one-hour download link should not pull in an AEAD |

**Request and response**

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `uploads` | `multipart/form-data` bodies, filename sanitizing, content-addressed object names, bounded readers, magic-byte content sniffing, attachment downloads | The filename, the declared content type and the byte count all come from the client. A build with no upload route has no business carrying a multipart parser |
| `views` | Compiled HTML views through Askama, plus mail bodies rendered from the same templates | Askama compiles templates to Rust at build time, so there is no expression evaluator in the request path and server-side template injection is structurally absent rather than defended against. The trade is that editing a template means rebuilding |
| `i18n` | Fluent translation catalogs, locale negotiation against a whitelist, and the active locale exposed to views and Inertia props | An application shipping one language should not carry a message parser and a plural-rule table to say so |

**Persistence**

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `session-store-db` | A sqlx-backed `SessionStore`, so sessions survive a restart instead of logging every user out on deploy | Brings a table and a migration |

**Notifications**

| Feature | What it adds | Why it is opt-in |
|---|---|---|
| `notifications` | One event, told to one person, over whichever channels apply. Implies `mail`, which is the channel it is overwhelmingly used for | A channel-less core would be a subsystem that can deliver nothing |
| `notifications-db` | The in-app inbox: one row per delivered notification | Brings a table and a migration. An application that only sends mail should not carry them |
| `notifications-broadcast` | A live push to whoever is connected now, over the `realtime` machinery | The inbox answers "what did I miss"; the broadcast answers "what just happened". Wanting one is not wanting the other |
| `notifications-queue` | Hands the mail channel to the job queue instead of the request | The only one of the four that changes *where* the work happens. It takes on running a worker, and an application without one should not be offered a method that writes rows nobody drains |

Five of these bring a table: `auth-reset`, `auth-remember`, `api-tokens`,
`notifications-db` and `session-store-db`. Each has its own idempotent
migration, applied the way you apply the job migrations. A project generated
by `arc new` on this version wires `arcature_sessions` into `--migrate`
already; the rest are yours to apply, because only you know the order they
belong in.

One thing `auth-reset` does **not** do, because it would be easy to assume it
does: spending a reset link does not sign the account's other sessions out.
Sessions are keyed by session id and are not indexed by user, so there is no
portable statement that deletes every session belonging to one subject. The
mechanism that would hold -- a credential stamp checked when a session loads
-- is a separate piece this release does not ship. If your threat model is
"the attacker already has a session and the user is resetting to evict them",
that is yours to build on top.

None of the five new database features shares a table with any other, and no
two claim the same advisory lock -- `tests/advisory_locks.rs` fails if that
stops being true, so two migrators can run concurrently without one waiting on
a lock the other holds under a different name.

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

The current version is `0.1.1`, readable at runtime as
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
