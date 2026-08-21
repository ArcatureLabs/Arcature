# Authentication

This module owns the integration seams, not your identity schema. There is no
framework `User` table, no roles table, no permissions system. You write the
user type; Arcature gives it sessions, hashing, extractors, and an
authorization seam.

## The user contract

```rust,ignore
use arcature::AuthUser;

pub struct User {
    pub id: uuid::Uuid,
    pub email: String,
}

impl AuthUser for User {
    type Id = uuid::Uuid;
    const SESSION_KEY: &'static str = "user_id";
    fn id(&self) -> &uuid::Uuid { &self.id }
}
```

`Id` is what goes in the session — `Uuid`, `i64`, `String`, anything
serializable. `SESSION_KEY` defaults to `"user_id"`.

Then say how to load one back:

```rust,ignore
impl UserLoader<AppState> for User {
    type Error = sea_orm::DbErr;

    async fn load_user(id: &uuid::Uuid, state: &AppState) -> Result<Option<User>, DbErr> {
        // query state.db
        Ok(None)
    }
}
```

`Ok(None)` means the session is stale and the extractor answers 401. `Err`
means the database failed, which is a different thing.

`absolute_max_age()` is the maximum session lifetime measured from the login
timestamp, regardless of activity. It defaults to 30 days. This is separate
from the session layer's sliding inactivity timeout — one bounds how long a
session can live, the other how long it can idle.

## Extractors

| Extractor | Behaviour |
| --- | --- |
| `Auth<U>` | the signed-in user, or a 401 rejection |
| `OptionalAuth<U>` | `Option<U>`, never rejects |
| `AuthManager<U>` | login, logout, session rotation |
| `Session` | the session store |
| `Flash` | flash messages |
| `CsrfToken` | the current CSRF token |

```rust,ignore
pub async fn dashboard(auth: Auth<User>) -> Result<Response> {
    Ok(text(StatusCode::OK, format!("hello {}", auth.user().email)))
}
```

## Logging in and out

```rust,ignore
pub async fn store(auth: AuthManager<User>, /* ... */) -> Result<Response> {
    let user = /* look the user up and verify the password */;
    auth.login(&user).await?;
    Ok(redirect().to("/dashboard").into_response())
}

pub async fn destroy(auth: AuthManager<User>) -> Result<Response> {
    auth.logout().await?;
    Ok(redirect().to("/").into_response())
}
```

`login` returns a `LoginBuilder`; awaiting it does the work.
`.remember(true)` extends the session's max age.

Awaiting the builder **rotates the session ID** by calling `cycle_id` before
binding the user. This is mandatory and not opt-in: the
anonymous-to-authenticated transition is exactly where a session-fixation
attack would persist, so the ID always changes. You do not need to call
`regenerate()` after `login()`. It is there for rotating outside login.

Awaiting also stamps the authentication time into the session, which is what
`absolute_max_age()` is measured against.

`logout` flushes the whole session rather than removing the user key.

## Passwords

Argon2id, from the `argon2` crate. No Arcature-written cryptography.

```rust,ignore
use arcature::auth::{PasswordConfig, PasswordHasher, PasswordHashString, PasswordSecret,
                     RehashOutcome, verify_password};

let hasher = PasswordHasher::new(PasswordConfig::default())?;
let stored = hasher.hash(b"correct horse battery staple")?;

let parsed = PasswordHashString::new(&row.password_hash)?;
verify_password(b"attempt", &parsed)?;

if matches!(hasher.needs_rehash(&parsed), RehashOutcome::Rehash) {
    // parameters changed since this hash was written; rehash on next login
}
```

`PasswordSecret` wraps a plaintext password, `PasswordHashString` a
PHC-formatted stored hash. Both are secrecy-backed: `Debug` and `Display`
never expose the secret and the buffer zeroizes on drop. No plaintext
password, signing key, or token appears in logs, error output, or a `Debug`
line anywhere in the framework.

## Sessions

Sessions are tower-sessions. Arcature owns the cookie attributes and the
signed jar:

```rust,ignore
use std::time::Duration;
use arcature::auth::{SameSite, SessionConfig, SessionKey};

let key = SessionKey::generate()?;         // or ::from_bytes(&secret)
let config = SessionConfig::new(key.as_bytes())?
    .with_cookie_name("acme_session")
    .with_same_site(SameSite::Lax)
    .with_max_age(Duration::from_secs(60 * 60 * 2))
    .with_absolute_max_age(Duration::from_secs(60 * 60 * 24 * 7));

let layer = config.into_layer(store)?;
```

`SessionConfig::dev(key)` relaxes `Secure` for plain HTTP in development.
`arc key:generate` produces a signing key.

The store is yours to choose: any `tower_sessions::SessionStore`. Arcature
does not pick one by default.

From a handler:

```rust,ignore
pub async fn handler(session: Session) -> Result<Response> {
    session.put("last_seen", 1_700_000_000i64).await?;
    let value: Option<i64> = session.get("last_seen").await?;
    let taken: Option<i64> = session.forget("last_seen").await?;
    session.regenerate().await?;
    session.flush().await?;
    Ok(no_content())
}
```

`session.raw()` borrows the underlying tower-sessions `Session`.

`Flash` writes one-shot messages read and cleared on the next request:
`flash.success(..)`, `.error(..)`, `.warning(..)`, `.info(..)`, and
`flash.messages()` to read them.

## Authorization

Authorization is never automatic and never implied. Validation proves a
request is well-formed; `Bound<T>` proves a row exists; neither says the user
may act on it.

```rust,ignore
pub struct LinkPolicy;

impl arcature::Policy<Link> for LinkPolicy {
    type User = User;
    fn check(user: &User, action: &str, link: &Link) -> bool {
        match action {
            "view" => true,
            "update" => user.id == link.user_id,
            _ => false,
        }
    }
}
```

Call it through `Auth::authorize`:

```rust,ignore
pub async fn update(auth: Auth<User>, link: Bound<Link>) -> Result<Response> {
    let link = link.into_inner();
    auth.authorize::<Link, LinkPolicy>("update", &link)?;
    Ok(no_content())
}
```

Both type parameters are required. `authorize` is generic over the model `M`
and the policy `P`, and Rust has no partial turbofish, so
`authorize::<LinkPolicy>(..)` does not compile — the model comes first. (The
doc comments on `Auth::authorize` and on the `Policy` trait show the
one-parameter form; they are wrong.)

`false` becomes `AuthzError::Forbidden`.

## CSRF

`CsrfLayer` enforces a naive double-submit token. Not signed, not
session-bound: the server issues a random nonce in a cookie, the client echoes
it in a header, and the server compares the two. The strength is in the
cookie attributes, not in a signature.

What is exempt, by design:

- Safe methods: `GET`, `HEAD`, `OPTIONS`, `TRACE`. These get a fresh cookie
  if the request did not carry one.
- Bearer-token requests. An unsafe request carrying `Authorization: Bearer …`
  is forwarded without the check and without a CSRF cookie. A bearer token is
  not sent automatically by the browser, so there is nothing to forge.

Unsafe non-bearer methods — `POST`, `PUT`, `PATCH`, `DELETE` — must present a
matching cookie and header, or the request is rejected with 403.

### Three presets

| Preset | Cookie | Header | Secure | SameSite |
| --- | --- | --- | --- | --- |
| `CsrfConfig::new()` | `__Host-csrf` | `x-csrf-token` | yes | Strict |
| `CsrfConfig::dev()` | `arcature-csrf` | `x-csrf-token` | no | Strict |
| `CsrfConfig::inertia()` | `XSRF-TOKEN` | `x-xsrf-token` | yes | Lax |

`new()` is the strongest. The `__Host-` prefix mandates `Secure`, forbids
`Domain`, and pins the path to `/` (RFC 6265bis), so a sibling subdomain
cannot overwrite the cookie. `SameSite=Strict` keeps it off every cross-site
request. `HttpOnly` is false on purpose: JavaScript has to read the cookie to
put it in the header, and the header is the proof the page is same-origin.

### Why an Inertia application uses `inertia()`

Inertia's client is axios. Axios reads a cookie named `XSRF-TOKEN` and echoes
it in `X-XSRF-TOKEN`. Both are hard-coded and neither is configurable without
writing application JavaScript.

Against `CsrfConfig::new()`, an Inertia form is rejected with 403 until the
application ships a shim that reads the token and reconfigures axios. That
shim is exactly the framework-owned client package Arcature does not publish
(see [ADR 0001](decisions.md)), so the server moves to meet the client
instead.

Two attributes weaken, deliberately:

- **No `__Host-` prefix**, because axios will not look for one. A sibling
  subdomain able to set cookies on the parent domain can then overwrite
  `XSRF-TOKEN`. That is a session-fixation-shaped attack on the nonce, not a
  way to read it, and it requires the attacker to already control a subdomain
  of your site.
- **`SameSite=Lax` rather than `Strict`.** Strict withholds the cookie on any
  cross-site navigation — an OAuth callback, a link from an email — so the
  first page load after one arrives with no token at all. Lax sends it on
  top-level GET navigations, which is the case Strict breaks and not one CSRF
  exploits: a forged unsafe request is still cookie-less.

The full reasoning, including the cost, is [ADR 0002](decisions.md).

### Keeping `new()` and configuring axios yourself

If you would rather not weaken those two attributes, keep `CsrfConfig::new()`
and tell axios where to look. This is application JavaScript, in your own
codebase, not a framework package:

```js
import axios from "axios";

axios.defaults.xsrfCookieName = "__Host-csrf";
axios.defaults.xsrfHeaderName = "X-CSRF-Token";
```

Both defaults are writable, so no interceptor is needed. The trade is one
file you maintain against two cookie attributes you keep.

### Overriding individual attributes

```rust,ignore
let config = CsrfConfig::new()
    .with_cookie_name("__Host-app-csrf")
    .with_header_name("x-app-csrf")
    .with_same_site(SameSite::Lax)
    .with_secure(true)?;
```

`with_cookie_name` auto-enables `Secure` when the name starts with `__Host-`,
and `with_secure(false)` returns an error if the cookie name carries that
prefix. The invalid combination is not representable.

### What it does not defend against

Not XSS: same-origin script can read the cookie and send the header. Not
anything the reverse proxy owns — TLS termination, rate limiting, request-size
limits. It defends against forged cross-site unsafe requests from an
authenticated browser, which is the attack it is named after.

## What this module does not own

Your user model, roles, permissions, or account schema. Cryptography — Argon2,
HMAC, SHA-2 and TLS come from RustCrypto, `cookie`, and the certified rustls
plus aws-lc-rs path. A default session store.
