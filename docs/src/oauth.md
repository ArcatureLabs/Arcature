# OAuth

OAuth 2.0 Authorization Code with PKCE, against any provider. From a client's
point of view an authorization server is two URLs, and two URLs is all this
module asks for.

It ends at the token response. Calling a userinfo endpoint, matching the
result to a local account, starting a session — none of that is here, and the
section at the bottom says why.

## Turning it on

```toml
arcature = { version = "0.1", features = ["oauth"] }
```

`oauth = ["dep:oauth2", "dep:url"]`. It is in neither `default` nor
`fullstack`. It also needs nothing else: CI builds it with
`--no-default-features --features oauth` and runs both test binaries on that
build, so the feature is known to stand up without a database, a job runner or
a CLI underneath it.

`oauth2` owns the protocol and vendors the HTTP client it drives, reachable as
`arcature::oauth::oauth2::reqwest`. There is deliberately no direct `reqwest`
in `[dependencies]`: adding one would put a second major version of the same
client in the dependency graph, and nothing in `src/oauth/` would ever reach
it. The whole crate is re-exported as `arcature::oauth::oauth2` so downstream
code targets the version Arcature pinned rather than resolving its own.

Examples below are marked `ignore` — neither compiled nor run. They name a
session, a callback route and an account store, the first behind the `auth` feature an `oauth`-only build never compiles, the last two absent from this
crate, so there is nothing here for a compiler to check them against.

## Configuring a provider

An `Endpoints` is a pair of `&'static str`. The bundled providers are `const`
values of it, not variants of anything:

| Preset | Authorization endpoint | Token endpoint |
| --- | --- | --- |
| `GITHUB` | `https://github.com/login/oauth/authorize` | `https://github.com/login/oauth/access_token` |
| `GOOGLE` | `https://accounts.google.com/o/oauth2/v2/auth` | `https://oauth2.googleapis.com/token` |
| `DISCORD` | `https://discord.com/oauth2/authorize` | `https://discord.com/api/oauth2/token` |

```rust,ignore
use arcature::oauth::{Endpoints, OauthClient, GITHUB};

// A bundled provider.
let github = OauthClient::new(
    GITHUB,
    client_id,
    Some(client_secret),
    "https://app.example.com/auth/github/callback",
)?;

// A provider the framework has never heard of, configured identically.
const ACME_SSO: Endpoints = Endpoints {
    authorization: "https://sso.acme.example/oauth/authorize",
    token: "https://sso.acme.example/oauth/token",
};
let sso = OauthClient::new(ACME_SSO, client_id, Some(client_secret), redirect)?;
```

The rejected alternative was a `Provider` enum with a variant per vendor. It
reads better in a signature, and it makes adding a provider a framework
release: an in-house identity server could never be more than a second-class
`Provider::Custom { .. }` beside the real ones. A `const` pair costs nothing,
compares by value (`Endpoints` is `Copy`, `PartialEq` and `Eq`), and makes the
company SSO and GitHub the same kind of thing. `tests/oauth.rs` asserts that
by running the bundled presets and an invented one through identical
assertions.

Endpoints known only at run time — read from configuration, or discovered —
cannot be `&'static str`, so they take the other constructor:

```rust,ignore
let client = OauthClient::for_urls(
    &config.authorization_endpoint,
    &config.token_endpoint,
    &config.client_id,
    config.client_secret.clone(), // Option<String>
    &config.redirect_uri,
)?;
```

`client_secret` is an `Option`. `None` is a public client — a native or
single-page app with no secret to keep, relying on PKCE alone — and the
secret is then omitted from the token request rather than sent empty, which
some providers reject outright. With a secret set, the client authenticates to
the token endpoint over HTTP Basic; the round trip pins the exact header,
`Basic base64(client_id:client_secret)`.

An `OauthClient` owns its HTTP client and its endpoints and is not looked up
from anywhere. Hold it as application state.

## Transport: https, and one exception

All three URLs are parsed and transport-checked when the client is built, in
this order:

| Position | Role named in the error | Rejects |
| --- | --- | --- |
| 1 | `"authorization endpoint"` | unparseable, or plaintext off loopback |
| 2 | `"token endpoint"` | same |
| 3 | `"redirect URI"` | same |

A URL that does not parse is `OauthError::InvalidUrl { role }`. One that
parses and fails the transport check is `OauthError::InsecureTransport
{ role }`. The first failure wins, so `role` names the first bad URL, not all
of them.

The rule itself, from `require_transport_security` in
`src/oauth/provider.rs`:

| URL | Verdict |
| --- | --- |
| `https://` anything | allowed |
| `http://localhost` (ASCII case-insensitive) | allowed |
| `http://127.0.0.1`, any IPv4 loopback | allowed |
| `http://[::1]`, any IPv6 loopback | allowed |
| `http://` any other host | refused |
| `http://localhost.evil.test` | refused |
| any other scheme | refused |

So plaintext HTTP is permitted, and only when the host is loopback. That is
the one case with no network to intercept, and it is the case every local
development redirect URI needs.

There is no flag to widen it, and that absence is the decision. An application
that could switch the check off would eventually ship with it switched off,
and the switch would be found in a production config file six months later.
Development gets what it needs from the loopback exception and nothing more.
A host that merely mentions loopback is not loopback: `localhost.evil.test`
and `127.0.0.1.evil.test` are both refused, and both are pinned by tests in
`src/oauth/provider.rs` and in `tests/oauth.rs`.

`tests/oauth_round_trip.rs` runs on the exception on purpose. Its mock
provider binds `127.0.0.1:0`, so the suite needs no certificate and no
network, and behaves identically on a pull request from a fork.

The HTTP client built for the token exchange sets exactly one option:
`redirect::Policy::none()`. A token endpoint that answers `302` is a
server-side request forgery primitive, not a provider quirk to accommodate.

## The authorization redirect

```rust,ignore
use arcature::oauth::OauthClient;

pub async fn start(session: Session, client: OauthClient) -> Result<Redirect> {
    let start = client.authorize(&["read:user"])?;

    session.put("oauth.state", start.state().as_str()).await?;
    session.put("oauth.verifier", start.verifier().secret()).await?;

    Ok(Redirect::to(start.url().as_str()))
}
```

`authorize` returns an `Authorization` holding three things: the URL, the
`state`, and the PKCE verifier. The browser is handed the URL. The other two
have to survive until the callback, which means the session or somewhere like
it — they are per-attempt values, not per-user ones, and a user with two tabs
open has two of each. `into_parts()` takes the three apart by value when
borrowing them is awkward.

`Authorization`'s `Debug` prints the URL up to the end of the path and then
`?[redacted]`, because the state and the code challenge live in that query
string and a `Debug` output is exactly the thing that ends up in a log.

## PKCE (S256), and why

The challenge is built by `PkceCodeChallenge::new_random_sha256()`. The method
is `S256` and there is no way to ask for anything else.

The rejected alternative is RFC 7636's other method, `plain`, where the
challenge *is* the verifier. It exists for clients that cannot compute a
SHA-256, which is no client this framework will ever run on, and it defends
against nothing: an attacker who can read the authorization request can read
the challenge, and under `plain` the challenge is the secret. Offering the
option would only create a way to configure the protection off.

What PKCE buys is the case where the authorization code is intercepted — a
malicious app registered on the same custom URI scheme, a code leaking through
a `Referer` header, a shared-machine browser history. The code alone is not
enough to redeem it: the token endpoint wants the verifier whose SHA-256 was
committed to at the start, and only the client that started the flow has it.

`tests/oauth_round_trip.rs` is what turns that from a claim into a test. The
mock provider recomputes the challenge from the verifier the token endpoint
was handed and refuses the exchange when the two disagree, which is what a
real authorization server does. The suite therefore proves three things a
"the string appears in the URL" test cannot:

- the `code_challenge_method` the provider saw was `S256`;
- the challenge the provider saw is the base64url SHA-256 of the verifier the
  exchange later sent, and is not the plain verifier;
- a well-formed verifier from somebody else's flow is refused, arriving as
  `OauthError::Provider { code: "invalid_grant" }`.

The test writes out its own SHA-256 and base64url rather than pulling a crate.
`sha2` belongs to the `uploads` feature and is not compiled by an `oauth`
build, and a test that shares an implementation with the code under test can
agree with its bugs. The test's arithmetic is pinned against the published
FIPS 180-4 and RFC 7636 vectors.

## The `state` parameter

| Property | Value |
| --- | --- |
| Source | `getrandom::fill`, the OS CSPRNG |
| Length | 32 bytes, `STATE_BYTES` in `src/oauth/pkce.rs` |
| Encoding | lowercase hex, so 64 characters, safe in a query string unescaped |
| On RNG failure | `OauthError::Entropy`, no fallback |
| Comparison | `OauthState::verify` -> `constant_time_eq`, same file |

`OauthState::generate` returns `Err(OauthError::Entropy)` if the OS randomness
source is unavailable. There is no fallback to a clock, a counter or a hash of
the request, because a predictable state is not a weaker state, it is no
state.

The comparison is constant time with respect to the contents of the two
values. `constant_time_eq` XOR-accumulates every byte and tests the
accumulator once at the end, and the accumulator goes through
`std::hint::black_box` before that test — without it a compiler is entitled
to notice that the accumulator can only grow and to break out of the loop
early, which is precisely the timing signal the function exists to remove.
Length is compared up front and does short-circuit; the length of a state is
visible in the query string already, so hiding it buys nothing.

The rejected alternative is `==`, which returns at the first differing byte.
Correctness alone cannot tell the two apart — both give the same answer — so
`tests/oauth.rs` asserts the property that can: the answer is identical
wherever the difference sits, checked at every one of the 32 positions
including the first, which is the one a short-circuiting comparison exits on
immediately. A wall-clock measurement of the same property sits beside it
under `#[ignore]`, because a shared or loaded CI machine makes any tolerance
wrong.

**The state is checked before the code is redeemed.** It is the first
statement in `exchange`, and a mismatch returns without touching the network.
Two tests pin the order rather than trusting it:

- `tests/oauth.rs` points a client at a token endpoint that is not listening
  and asserts the error is `StateMismatch` and not `Transport`. If the check
  ran second, the variant would be the other one.
- `tests/oauth_round_trip.rs` drives a real callback carrying a second flow's
  state, then asserts the provider's ledger recorded `token_calls == 0` — a
  forged callback is refused before the code is spent, not after.

The order matters because an authorization code is one-time. A state check
that ran after the exchange would let a CSRF callback burn a legitimate code,
and would have handed the tokens over before anybody objected.

## The callback and the exchange

```rust,ignore
use arcature::oauth::{OauthClient, OauthState, PkceVerifier};

pub async fn callback(
    session: Session,
    client: OauthClient,
    Query(params): Query<CallbackParams>, // code: String, state: String
) -> Result<Response> {
    let stored: String = session
        .forget("oauth.state")
        .await?
        .ok_or_else(|| Error::forbidden("no OAuth flow in progress"))?;
    let verifier: String = session
        .forget("oauth.verifier")
        .await?
        .ok_or_else(|| Error::forbidden("no OAuth flow in progress"))?;

    let tokens = client
        .exchange(
            &OauthState::from_stored(stored),
            &params.state,
            &params.code,
            PkceVerifier::from_secret(verifier),
        )
        .await?;

    // `tokens.access_token()` is a bearer credential. Send it; do not put it
    // in a log line or an error message.
    Ok(sign_in(profile_for(&tokens).await?).await?)
}
```

`exchange` takes the stored state by reference and the verifier by value. The
verifier is consumed, so the same one cannot be reused for a second exchange
by accident. Take both out of the session rather than reading them, which is
what `forget` does here: a flow finishes once, and leaving the values behind
leaves a live verifier sitting in the session for whatever arrives next.

What a successful exchange returns:

| `TokenSet` accessor | Type | What the round trip observed |
| --- | --- | --- |
| `access_token()` | `&str` | the provider's `access_token` member |
| `refresh_token()` | `Option<&str>` | `Some`, and not equal to the access token |
| `token_type()` | `&str` | `"bearer"` — the provider sent `Bearer`, and this path lowercases |
| `expires_in()` | `Option<Duration>` | `Some(3600s)`, from `expires_in` |
| `scopes()` | `&[String]` | `["read:user"]` after `["read:user", "profile"]` was asked for |

That last row is the reason the accessor exists at all. Narrowing the granted
scopes is the provider's prerogative, so the answer has to be read out of the
response rather than echoed back from the request.

`TokenSet::new(access_token, token_type)` builds one directly, for tests and
for applications that obtained tokens some other way and want the same
redaction. It stores what it is given and lowercases nothing.

## Fetching user info

The module does not do this, and that is the deliberate half of the two-URL
model. An OAuth 2.0 authorization server is an authorization endpoint and a
token endpoint; a userinfo endpoint belongs to a *resource server*, and its
path, its JSON shape and its field names differ per provider — `sub` here,
`id` there, `login` versus `username` versus `preferred_username`. A framework
type that covered them would be a per-provider parser, which is the provider
enum this module already declined, wearing a different hat.

So the leg after `exchange` is an ordinary authenticated HTTP request, with
the access token as a bearer credential:

```rust,ignore
use arcature::oauth::oauth2::reqwest;

let profile: serde_json::Value = reqwest::Client::new()
    .get("https://api.github.com/user")
    .bearer_auth(tokens.access_token())
    .send()
    .await?
    .json()
    .await?;
```

`arcature::oauth::oauth2::reqwest` is the client `oauth2` already vendors, so
reaching for it adds nothing to the dependency graph. An application that
already has an HTTP client should use that one instead.

The round trip makes this call for a reason beyond illustration. Everything
before it compares strings against strings, and an access token parsed out of
the `refresh_token` member is still a string that survives every assertion. A
resource server is the only thing that can tell the two apart, so the test
stands one up, presents `tokens.access_token()` to it, and asserts on the
provider's side that the credential it received was `Bearer <access token>` —
a refresh token must never be the credential sent to a resource server.

## Errors

`OauthError` is the one error type. Every variant is built from a fixed
`&'static str` or from a provider-supplied error *code*, never from a response
body:

| Variant | Carries | Raised by | Retry? |
| --- | --- | --- | --- |
| `InvalidUrl { role }` | the role, a `&'static str` | construction | no, it is a config bug |
| `InsecureTransport { role }` | the role | construction | no, same |
| `Entropy` | nothing | `authorize` | no, not recoverable by retrying |
| `StateMismatch` | nothing | `exchange`, before the network | no, start the flow again |
| `Transport` | nothing | `exchange`; also a client that fails to build | yes, this is the retryable one |
| `Provider { code }` | the provider's `error` member | `exchange` | depends on the code |
| `MalformedResponse` | nothing | `exchange` | no |

`Provider { code }` carries `invalid_grant`, `invalid_client`,
`unsupported_grant_type` and the rest of RFC 6749's fixed vocabulary. It does
not carry the `error_description` beside it, which is free-form text the
provider wrote.

**A token-endpoint response that does not parse becomes `MalformedResponse`,
and the body is dropped on the floor.** Upstream, `RequestTokenError::Parse`
holds the raw bytes the provider sent; both it and `RequestTokenError::Other`
collapse to `MalformedResponse` with nothing attached. The reason is the case
that looks harmless: a malformed *success* response still contains an access
token, so a variant that carried the body for diagnostics would put
credentials into every log line that formatted the error.

The cost of that is real and worth stating. Debugging a provider that answers
in a shape this implementation does not understand means reproducing the
request, because the error will not tell you what it said.

The three failure modes of `exchange` stay distinguishable because an
application may retry one of them and must not retry the others, and
`tests/oauth_round_trip.rs` covers each: a replayed code arrives as
`Provider { code: "invalid_grant" }`, a token endpoint that is not listening
as `Transport`, and a callback from another flow as `StateMismatch`.
`an_oauth_error_never_carries_a_response_body` in `tests/oauth.rs` renders
the five runtime variants — `StateMismatch`, `Entropy`, `Transport`, `MalformedResponse` and `Provider` — and asserts none of them mentions `access_token` and none runs
past 200 characters.

## What is never logged

| Type | `Debug` renders | `Display` |
| --- | --- | --- |
| `PkceVerifier` | `PkceVerifier([redacted])` | none |
| `OauthState` | `OauthState([redacted])` | none |
| `TokenSet` | `TokenSet([redacted])` | none |
| `Authorization` | the URL through the path, then `?[redacted]`, plus the two redacted fields | none |
| `OauthClient` | `OauthClient { .. }` | none |
| `Endpoints` | derived, in full — it holds two public URLs | none |
| `OauthError` | derived | yes, and it carries no body |

None of the secret-bearing types implements `Display`, so none of them can
reach a log line through ordinary formatting. Reading a secret out means
calling `secret()`, `as_str()` or `access_token()` by name, which is the point
where a reviewer sees the decision. `OauthClient`'s `Debug` is hand-written
rather than derived because the client holds a `ClientSecret`, and `oauth2`'s
own redaction is not something this crate should rely on transitively.

Five tests in `tests/oauth.rs` pin this by formatting a real value and
asserting the secret is absent from the output.

Separately, under the `observe` feature, the JSON log layer drops the value of
any field whose name contains one of the fragments in
`arcature::observe::redact::DENY_LIST` — `token`, `verifier`, `secret`, `auth`,
`credential` and the rest — with `-` and `.` folded to `_` first. That is a
second net under the first, not a replacement for it: it matches on field
names, so it catches a field called `oauth.access-token` and does not catch a
secret interpolated into a message string.

## What this module does not do

**No provider registry, and no discovery.** There are three `const`
`Endpoints` and no way to look one up by name — no enum, no `FromStr`, no
table keyed by a string from a config file. There is also no OpenID Connect
discovery: nothing reads `/.well-known/openid-configuration`. Fetch it
yourself if you want it, and hand the two URLs to `for_urls`.

**No token storage.** The `oauth` feature brings no table, no migration and no
model, and the module never touches a session. `authorize` hands you the state
and the verifier, `exchange` hands you a `TokenSet`; where those live between
the two requests, and whether the access token is kept after the flow at all,
is the application's decision. It is also why `oauth` needs no `database`.

**No refresh loop, and no refresh method.** `exchange` is the only thing on
`OauthClient` that talks to a token endpoint. There is no background task
watching `expires_in`, no interceptor that retries a `401` with a refreshed
credential, and no `refresh()`. `TokenSet::refresh_token()` hands you the
string; driving the refresh grant with it goes through the re-exported
`oauth2`, which is exactly what
`a_refreshed_token_set_carries_the_new_access_token` does. A refresh loop
needs somewhere to write the new token back to, and the paragraph above is the
reason there is no such place.

**No OpenID Connect.** No `id_token` on `TokenSet`, no JWT parsing, no
signature verification, no nonce. An `id_token` member in a token response is
ignored. Verifying one is a JWS implementation plus a key-set fetcher, and
neither belongs behind a feature whose stated job is two URLs.

**No revocation and no introspection.** RFC 7009 and RFC 7662 are two more
endpoints, and `Endpoints` holds two.

**No routes, no extractor, no middleware.** Nothing in `src/oauth/` imports
`axum`. There is no callback handler to mount, no `Application::oauth(..)`
wiring, and no `arc make:` generator. The two handlers in this chapter are
what an application writes.

**No timeout on the token exchange.** The HTTP client is built with one option
set — the redirect policy — so the request inherits whatever the vendored
`reqwest` defaults to. An application that needs a bounded exchange should
wrap the `exchange` future in `tokio::time::timeout`.
