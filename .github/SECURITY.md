# Security policy

## Supported versions

Arcature follows semantic versioning and is in `0.x`, where the minor is the
breaking field. Security fixes land on the latest `0.x` minor only: an older
minor is an older API, and backporting to one would mean maintaining a branch
whose surface has already been replaced. There is no long-term-support branch,
and one is not planned before the crate is published.

| Version | Supported |
|---|---|
| `0.1.x` | Yes -- the latest minor |
| Anything earlier | No |

The crate is **not yet published to crates.io**. Until it is, "the latest
minor" means the `main` branch of
[ArcatureLabs/Arcature](https://github.com/ArcatureLabs/Arcature).

## Reporting a vulnerability

**Do not open a public issue, pull request, or discussion.**

Report privately, by either route:

- GitHub's private vulnerability reporting: the **Security** tab of the
  repository, then **Report a vulnerability**. This is the preferred route —
  it keeps the report, the fix, and the advisory in one place.
- Email **security@arcature.dev**, or, if that bounces,
  <lhquangmink@gmail.com>.

A useful report contains:

- the affected version or commit;
- the feature flags enabled, since much of the framework is feature-gated;
- what an attacker gains, stated plainly;
- the smallest reproduction you can manage — a failing test is ideal, a curl
  command is fine;
- whether you intend to disclose publicly, and when.

## What to expect

| Stage | Target |
|---|---|
| Acknowledgement that a human has read it | 3 working days |
| Initial assessment: accepted, needs more information, or not a vulnerability | 7 working days |
| Fix released, or a written plan with a date | 30 days for high and critical, 90 days for the rest |

These are targets, not contractual guarantees; Arcature is maintained by a
small group. If a deadline slips you will be told why rather than left waiting.

Please give us the window above before disclosing publicly. Credit goes to the
reporter in the advisory and the changelog unless you ask otherwise.

## Scope

In scope: anything in this repository that runs in an application built on
Arcature — the request pipeline, CSRF, sessions, password hashing, the
validation boundary, the Inertia protocol implementation, the job queue, the
proxies, the CLI and the generated scaffold.

Out of scope, and better reported upstream:

- Vulnerabilities in dependencies. Report them to the dependency; tell us too,
  and we will bump the pin.
- Findings that require an attacker who already has code execution in the
  application process.
- Missing hardening that the documentation already names as a deliberate cost.
  Two examples: `CsrfConfig::inertia()` drops the `__Host-` cookie prefix and
  uses `SameSite=Lax`, and `SecurityHeaders` leaves HSTS and CSP off by
  default. Both are argued in the source and in `docs/decisions/`. An argument
  that the reasoning is *wrong* is welcome — as an issue, not an advisory.

## The attack surface

The section above says what is in scope. This one says where the scope
actually is: every place in the framework where a byte an attacker chose meets
something that interprets it, and what stands in front of it.

It exists to make one question answerable during review. A pull request either
adds a row here, changes a guard in an existing row, or does neither -- and
"neither" is the answer for the overwhelming majority of them. A change that
adds a row without saying so is the shape of change this table is meant to
catch.

Read the **Guard, and its default** column literally. Where it says a limit is
unset by default, that is the framework's default; the generated scaffold sets
its own value in `bootstrap/app.rs`, and the two are different numbers on
purpose. A library that imposes a body limit on every application is a library
that gets a workaround written for it.

### The request, before any handler

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| Request line, headers, HTTP/1 framing | `axum::serve`, wrapped by `pipeline::compose_service` | hyper | hyper's own header and URI limits. Arcature overrides none of them. | always |
| URL path, for route selection | `src/routing/table.rs` | `axum::Router` (matchit) | The matcher itself. No length cap of our own. | always |
| Body size | pipeline stage 12, `RequestBodyLimitLayer` | -- | `ApplicationBuilder::body_limit`. **Unset by default** -- unbounded. The scaffold sets 2 MiB. | always |
| A slow request | pipeline stage 13, `TimeoutLayer` -> 408 | -- | `ApplicationBuilder::timeout`. **Unset by default.** The scaffold sets 30 s. | always |
| A handler panic | pipeline stage 8 | -- | The payload is discarded and a generic RFC 9457 `Problem` is returned. A panic message routinely carries a path, a query fragment, or the offending value. | always |

### Extractors -- where a body becomes a type

Each of these is a newtype over the corresponding axum extractor. It runs
`validator::Validate` after deserialisation and turns a rejection into an RFC
9457 `Problem` through `src/validation/rejection.rs` rather than echoing the
input back.

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| Path parameters | `ValidatedPath<T>` | serde | `Validate`, then `from_path_rejection` | `validation` |
| Query string | `ValidatedQuery<T>` | `serde_urlencoded` | `Validate`, then `from_query_rejection` | `validation` |
| JSON body | `ValidatedJson<T>` | `serde_json` | `Validate`, then `from_json_rejection` | `validation` |
| Form body | `ValidatedForm<T>` | `serde_urlencoded` | `Validate`, then `from_form_rejection` | `validation` |

**There is no multipart extractor and no multipart parser.** `axum` is
depended on with `default-features = false` and an explicit feature list that
does not name `multipart`. Adding file upload adds a row here and widens the
surface materially -- filename, content type and length are all attacker-
chosen. That is exactly why it is worth writing the boundary down before the
feature exists.

### Cookies and tokens

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| Session cookie | `SessionConfig` into `tower_sessions::SessionManagerLayer` | the `cookie` crate, signed jar | The signing key must be exactly 64 bytes. `SessionConfig::new`: `__Host-id`, `Secure`, `HttpOnly`, `SameSite=Strict`, 14-day idle, 30-day absolute. `validate()` refuses a `__Host-` name without `Secure`. | `auth` |
| CSRF cookie and header | `src/auth/csrf.rs`, `CsrfMiddleware::call` | `Cookie::parse_encoded` | Double-submit. `CsrfToken::parse` accepts exactly 64 hex characters and nothing else. Safe methods are exempt, and so is any request carrying `Authorization: Bearer` -- a bearer request is not a browser-driven one. `CsrfConfig::inertia()` deliberately drops the `__Host-` prefix and uses `SameSite=Lax`. | `auth` |
| Password | `PasswordHasher::verify_password` | the `argon2` crate | Argon2id, m=19456 KiB / t=2 / p=1, 16-byte salt from `getrandom`. The comparison is the `argon2` crate's, which is constant-time. Plaintext lives in `secrecy::SecretSlice` and is zeroed on drop. | `auth` |
| OAuth `state` and `code` | `OauthClient::exchange` | `oauth2` 5.0 | `state` is compared **constant-time** (`src/oauth/pkce.rs`) and checked *before* the code is redeemed. PKCE S256. A token-endpoint parse failure collapses to `MalformedResponse` with the body dropped. | `oauth`, not in `default` |

The CSRF token is compared with `==`, not in constant time. That follows from
what the value is -- a double-submit token is a nonce the client is *given* and
hands back, not a secret the server checks a guess against -- but it is the
kind of thing a reader should find written down rather than discover.

### Protocol and transport surfaces

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| `X-Inertia-Partial-Data` and siblings | `InertiaRequest::parse` | a hand-written comma splitter | 8 KiB per header, 64 keys per header, dedup, UTF-8 boundary-safe truncation. `MergeIntent` accepts only `prepend` and `append`. | `inertia` |
| Props, on the way back out | `escape_script_body` | `serde_json`, then escaping | `<`, `>`, `&` and `/` are escaped to their `\u` forms -- a superset of the official Inertia escape, which covers the slash alone. Optional CSP nonce on the `<script data-page>` tag. | `inertia` |
| WebSocket upgrade and frames | `WebSocketEndpoint::handle` | `axum::extract::ws` (tungstenite) | Origin, then authorizer, then connection limit -- **all before the upgrade**. `WsLimits::conservative()`: 64 KiB message, 64 KiB frame, 20 s heartbeat, 40 s pong timeout. Inbound frames other than Close and Pong are **discarded unread**: the server parses no client payload. | `realtime` |
| SSE request | `SseEndpoint::handle` | -- (outbound only) | Origin and connection limit. No authorizer: channel authorisation is expected as a layer above. | `realtime` |
| `Origin` header | `OriginPolicy::authorize` | `to_str` plus an ASCII check | **`DenyAll` is the default.** Exact string comparison; an origin is public, so constant time is not wanted here. | `realtime` |
| Redirect target, including a hostile `Referer` | `validate_redirect_target` | -- | Rejects a target starting `//`, `http://` or `https://`. Backslash forms and non-HTTP schemes are **not** rejected, and `redirect().back()` falls back to `/` rather than erroring. | always |
| A path handed to `AppConfig::absolute_url` | `src/config/mod.rs` | -- | Joined, never substituted: leading slashes collapse, so `//host`, `///host` and `https://host` all land as path segments under `APP_URL`. | always |

### Storage and static files

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| Object key | `StoragePath::new` | hand-written validation | Rejects empty, a leading `/`, any backslash, bytes below `0x20` and `0x7F`, a `..` segment, and an empty interior segment. Every `Storage` method takes a `&StoragePath`, so the check cannot be skipped. It **rejects** rather than sanitises: a name with a space or an accent is a 400, not a rewrite. | `storage-fs`, `storage-s3` |
| Path under `public/` | `StaticFiles` | `tower_http::services::ServeDir` | ServeDir's traversal rejection. `append_index_html_on_directories(false)`. | always |
| Path under the static-page root | `Pages::serve` | `std::path::Path` components | An absolute path or any `..` component is a 403. There is **no canonicalisation**, so a symlink inside the root that points outside it is followed. | `pages` |

### Data that was attacker input earlier

The rows above are the front door. These two are the ones people forget: a
value that was validated on the way in is not validated on the way back out of
storage, and the code that reads it is often the code that trusts it most.

| Attacker input | Where it enters | Interpreted by | Guard, and its default | Feature |
|---|---|---|---|---|
| Job payload, read back from the database | `TypedHandler::handle` | `serde_json` | A deserialisation failure is `HandlerError::Malformed`: the row goes dead and is never retried, and **the serde error is dropped** so payload bytes cannot reach `last_error`. `kind` is capped at 128 bytes; a stored error at 4 KiB, on a char boundary. | `jobs` |
| Cached value | the `Cache` read path | `serde_json` | `Namespace::new` rejects an empty name, a trailing `:`, and control characters. | `cache` |

### Operator input, which is not attacker input

Listed so the distinction stays explicit. Everything here is written by
somebody who already controls the process; the guards are against mistakes and
against leaking the value, not against an adversary.

| Input | Where it enters | Guard |
|---|---|---|
| `DATABASE_URL` | `DatabaseConfig::new`, then sqlx `ConnectOptions::from_str` | A parse failure is a config error at boot. `Debug` is hand-written per driver and never prints the user, the password, or the URL. |
| SMTP DSN | `src/mail/config.rs` | The `url` crate; scheme allow-list `smtp`/`smtps`; `tls` value allow-list; an empty host is refused. |
| `APP_NAME`, `APP_URL`, `APP_ENV`, `APP_PORT` | `AppConfig::from_env` | Defaults on unset or empty; `env_parsed` falls back rather than failing. |
| `ARCATURE_VITE_IPC`, `ARCATURE_APP_IPC` | `src/dev_proxy/`, `src/application/serve_ipc.rs` | Set by `arc dev`; the socket path is process-private. A failed IPC bind refuses to start rather than falling back to TCP. |

`APP_ENV` **gates nothing**, and this table is where that stops being a slogan.
Release redaction of 5xx messages, the UAG endpoint, the log format and the
scaffold's `Secure` cookie switch all key off `cfg!(debug_assertions)` --
decided when the binary is compiled. A protection an environment variable can
switch off is one that anyone who can reach the process environment can remove
without redeploying.

### Two properties the inventory rests on

Both are stated because a future change could quietly end either, and neither
is enforced by a test.

- **No request can reach a subprocess.** Every `std::process::Command::new` in
  the crate is under `src/cli/`. Outside it the only `std::process` uses are
  `exit` on the IPC orphan path, `ExitCode` in the binary, and `id()` inside
  test modules. (`src/cli/parser.rs` has many `Command::new` calls; those are
  `clap::Command`.)
- **No SQL in the job queue is built by interpolation.** Every statement in
  `src/jobs/dialect/{postgres,mysql,sqlite}.rs` is a `const &str` with
  placeholders, and the schema is `include_str!` of a checked-in file. Every
  `format!` in `src/jobs/` produces an error message, a log line, or the worker
  id -- and the worker id reaches the database as a bind parameter, not as text
  spliced into a query.

### Endpoints that sit beside the pipeline

Two route groups are merged next to the router rather than through it, so they
carry no session, no maintenance mode, no rate limit and no access-log line.
Anything added to either inherits that.

- `/up` -- the health endpoints. Always present.
- `/_arcature/uag.json` -- requires the `uag` feature, an explicit
  `.uag_endpoint(..)` call, **and** `cfg!(debug_assertions)`. Three conditions,
  because it publishes the application graph.

### Off unless asked for

Named so that "the framework does not do X" and "the framework does not do X
by default" are not read as the same sentence. Each default is deliberate and
argued where it is defined.

| Protection | Default | Turned on by |
|---|---|---|
| `SecurityHeaders` as a whole -- `nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy` | absent | `ApplicationBuilder::security_headers` |
| HSTS | off, even with the layer on | `SecurityHeaders::with_hsts` |
| CSP | off, even with the layer on | `SecurityHeaders::with_csp` |
| Rate limiting | absent | `ApplicationBuilder::rate_limit`. `KeySource::Ip` reads `ConnectInfo`, which the framework does not install, so without it every request shares one bucket. No forwarding header is trusted as a client address unless a `KeySource::Header` names it. |
| Body limit, request timeout | unset | `ApplicationBuilder::body_limit`, `ApplicationBuilder::timeout` |

## Unsafe code, here and below

`arcature` sets `#![forbid(unsafe_code)]` in `src/lib.rs` and
`unsafe_code = "forbid"` under `[lints.rust]` in `Cargo.toml`, so the lint
covers every target of the package, tests included. That claim stops at the
crate boundary, and the crate boundary is not where the memory-safety risk is:
the framework's job is to integrate other people's code, and other people's
code is where the `unsafe` lives.

`unsafe-baseline.<host-target>.txt` at the repository root records the whole
picture, one line per crate, as measured by [`cargo-geiger`]. `just geiger`
recomputes it and diffs against the file for the current host;
`just geiger-accept` records a new answer. On a developer's machine the diff
is a review prompt -- a pull request that moves these numbers is expected to
say which dependency moved and why the change is acceptable. On Linux it is
also a gate: `.github/workflows/geiger.yml` runs the same diff whenever
`Cargo.lock` changes on `main`, and weekly as a backstop.

As recorded, over `--all-targets` with default features, the tree is 360
crates. 153 of them contain `unsafe`; 49 declare `#![forbid(unsafe_code)]`;
the remaining 158 happen to contain none but promise nothing. The counts,
as *reached by this build* out of *present in those crates*:

| | Reached | Present |
|---|---|---|
| `unsafe` functions | 342 | 1551 |
| `unsafe` expressions | 31094 | 111017 |
| `unsafe` impls | 823 | 1367 |
| `unsafe` traits | 76 | 102 |
| `unsafe` methods | 992 | 4251 |

The bulk of it is where you would expect it, and where it is about as
well-audited as Rust gets: `tokio`, `memchr`, `hashbrown`, `aho-corasick`,
`windows-sys`, `parking_lot_core`, `aws-lc-rs`, `bytes` and `lock_api`
account for roughly two-fifths of the reached expression count between them.
Arcature has not audited any of it. What it claims is narrower: the number is
written down, a change to it shows up in a diff, and nobody has to take "Rust
is memory-safe" on trust for a tree this size.

Three caveats on reading the file:

- The reading is per host target, which is why the filename carries the
  target. Near the leaves the graph is platform-specific -- the
  `x86_64-pc-windows-msvc` file contains `windows-sys`, the
  `x86_64-unknown-linux-gnu` one does not -- so a single shared file would
  report the reviewer's operating system as a change in the dependency tree.
  The numbers quoted above are the Windows reading. Linux is the target that
  matters for a deployed application and it is the one CI enforces; the
  counts differ near the leaves and the conclusion does not.

  A host with no baseline yet gets one recorded and the command fails once,
  saying so. That is the check reporting it had nothing to compare against,
  which is a different thing from the counts having moved, and it should not
  be silently accepted as either.

- `cargo-geiger` reads the `#![forbid(unsafe_code)]` *attribute* out of each
  crate root and does not read `[lints]` out of `Cargo.toml`. Under
  `--all-targets` the integration tests in `tests/` are separate crate roots
  that do not repeat the attribute, so the workspace crates are reported as
  `?` (no `unsafe` found, no `forbid` declared) rather than `:)`. The lint is
  still enforced on every one of those targets by the manifest.
- `arcature-macros` declares neither the attribute nor the lint. It contains
  no `unsafe`, and the counts confirm it, but nothing currently stops one
  from being added.

### The largest single piece of it is not Rust, and `cargo geiger` cannot see it

`cargo geiger` counts `unsafe` in Rust. The biggest memory-safety surface in
this tree is not written in Rust at all, so it appears in the table above only
as the handful of `unsafe extern` declarations that reach it.

TLS resolves to rustls, and rustls needs a cryptographic provider. This
manifest names one explicitly, twice:

- `sqlx` with `tls-rustls-aws-lc-rs` (`Cargo.toml`)
- `lettre` with `aws-lc-rs` (`Cargo.toml`)

Both crates also offer a `ring` provider, so this is a decision that was made
rather than a default that was inherited. `aws-lc-rs` builds `aws-lc-sys`,
which vendors AWS-LC -- a fork of BoringSSL. Version `0.44.0` of that crate
carries, across all the targets it supports:

| | Files | Lines |
|---|---|---|
| C | 414 | 145,513 |
| C headers | 270 | 156,427 |
| Assembly (`.S`) | 902 | 1,044,806 |
| Assembly (`.asm`, MASM/NASM) | 39 | 87,956 |

Only the subset for the host target is compiled. On
`x86_64-pc-windows-msvc`, which is what the Windows baseline was recorded on,
that subset is 254 object files linked into a 16 MB static library, and that
library is linked into every Arcature binary that talks to a database over
TLS, sends mail, or makes an outbound HTTPS request. It enters through
`database` + any `db-*` driver, through `mail`, and through `reqwest` under
`oauth` and `storage-s3` -- which is to say, through the default feature set.

**So the claim "Arcature is pure Rust" is false, and this file will not make
it.** `#![forbid(unsafe_code)]` is true of `arcature`. It says nothing about a
C library an order of magnitude larger than the framework, sitting in the path
of every TLS handshake.

Why the choice is nonetheless the defensible one, stated so that a future
reader can disagree with the reasoning rather than guess at it:

- There is no production-grade pure-Rust TLS provider. The choice is not
  between C and no C; it is between AWS-LC and `ring`, which is itself
  BoringSSL-derived C and assembly. `ring` is smaller, which is a real
  argument in its favour and the one to revisit if this is ever reopened.
- AWS-LC is continuously fuzzed, has a funded security team, ships CVE
  advisories on a schedule, and holds FIPS 140-3 validation. `ring`'s
  maintenance has been intermittent.
- rustls upstream made `aws-lc-rs` its default provider for those reasons.
  Choosing it keeps this tree on the path that receives upstream's attention.

What follows from writing it down: a CVE in AWS-LC is an Arcature security
release, not an upstream problem someone else will notice. `cargo audit` runs
daily and covers `aws-lc-sys` like any other crate, and that is the control
this section exists to point at.

[`cargo-geiger`]: https://github.com/geiger-rs/cargo-geiger

## What Arcature does not claim

Arcature writes no cryptography. Argon2id, HMAC, SHA-2 and TLS come from
RustCrypto, `cookie`, and the rustls + aws-lc-rs stack. A cryptographic flaw in
one of those belongs upstream.

Arcature is not a substitute for the reverse proxy in front of it. TLS
termination, rate limiting and connection-level request-size limits are the
front door's job, and the framework's defences assume it is doing it.

That assumption has a measured edge, and it is worth stating rather than
leaving to be discovered. The in-memory rate limiter keeps one bucket per key
and sweeps the table past 8192 entries, dropping every bucket that has
refilled to capacity -- which bounds the table only while buckets refill
faster than new keys arrive. Under a quota whose refill is slower than that,
the sweep is entitled to drop nothing, so it rescans a growing table on every
request while holding a blocking mutex. Measured at 128 connections: a fresh
key on every request costs nothing under a per-second quota and **5.2x
throughput** under a per-hour one. A per-hour quota keyed by address is the
ordinary shape of a login or password-reset throttle, so an attacker who can
present many distinct keys can degrade a service that is otherwise limiting
them correctly -- the limit holds, the throughput does not.

`RateLimit::redis(cache)` does not have this property; its backend keeps no
client-side map and lets the server expire keys. Neither does a
faster-refilling spelling of the same rate. `RateLimit`'s documentation
carries the four-row measurement, and `tests/load_profile.rs` reproduces it.
