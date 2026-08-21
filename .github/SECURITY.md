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

## What Arcature does not claim

Arcature writes no cryptography. Argon2id, HMAC, SHA-2 and TLS come from
RustCrypto, `cookie`, and the rustls + aws-lc-rs stack. A cryptographic flaw in
one of those belongs upstream.

Arcature is not a substitute for the reverse proxy in front of it. TLS
termination, rate limiting and connection-level request-size limits are the
front door's job, and the framework's defences assume it is doing it.
