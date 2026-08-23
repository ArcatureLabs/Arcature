# API tokens

An opaque bearer credential for a client that has no cookie and no session —
a CLI, a CI job, a mobile app, another service. The client sends
`Authorization: Bearer <token>` on every request. The framework turns that
back into a record, or refuses the request.

Off by default:

```toml
arcature = { version = "0.1", features = ["api-tokens", "db-postgres"] }
```

`api-tokens = ["database", "dep:sha2", "dep:subtle", "dep:zeroize"]`. It is
not part of `fullstack`. It brings a table and a migration, and an
application that only serves a browser never needs one. No new crate enters
the dependency graph: `sha2`, `subtle` and `zeroize` are already pulled in by
`session-store-db`, `crypt` and `signed-urls`, and the randomness comes from
`getrandom`, which is unconditional.

The examples below need a live database, so they are marked `ignore`: they
are neither compiled nor run. `no_run` would compile them without running
them, which is the stronger marker, but these name an application's own
account store and state type — neither of which exists in this crate — so
there is nothing here for a compiler to check them against.

## The property the design exists for

A token is two halves: a public 16-byte id and a secret 32-byte half. The row
holds the id in the clear — it is a lookup key, not a credential — and the
SHA-256 of the secret. The secret itself is never written anywhere.

From `src/tokens/migrations/postgres/0001_api_tokens.sql`:

```sql
CREATE TABLE IF NOT EXISTS arcature_api_tokens (
    id            BYTEA       PRIMARY KEY,
    secret_digest BYTEA       NOT NULL,
    tokenable_id  TEXT        NOT NULL,
    name          TEXT        NOT NULL,
    abilities     JSONB       NOT NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
)
```

There is a `secret_digest` column and no column that could hold the secret.
Disclosure of the table is therefore not disclosure of credentials: a stolen
backup, a read replica, or a reporting account with `SELECT` yields 32 bytes
of digest per token and no way to authenticate as anybody. A unit test in
`src/tokens/migrate.rs` asserts that every bundled migration contains
`secret_digest` and contains neither `secret_plaintext` nor `token TEXT`, so a
future migration that adds a plaintext column has to delete that test first.
An integration test reads the raw columns back and asserts the stored bytes
equal `digest_of(secret)` and not the secret.

## Issuing

```rust,ignore
use arcature::tokens::{Abilities, ApiTokens, NewApiToken};
use std::time::Duration;

let tokens = ApiTokens::new(pool.clone());
tokens.migrate().await?;

let issued = tokens
    .issue(
        &NewApiToken::expiring_in("user:42", "CI deploy key", Duration::from_secs(3600))
            .abilities(Abilities::of(["deploy:write"])),
    )
    .await?;

// Show this once. There is no second chance.
println!("{}", issued.plaintext().expose());
```

`ApiTokens::new` takes the application's own pool (`arcature::tokens::TokenPool`,
which is `arcature::database::Pool`). There is no `connect_lazy` twin as there
is on `DbSessionStore`: a token store is used from handlers, by which time the
pool is in hand, and a second pool would be a second slice of the database's
connection budget for no reason.

`issue` returns an `IssuedApiToken`: `token()` is the stored record,
`plaintext()` is the credential, `into_parts()` splits them. The plaintext
exists exactly once, in memory, in that return value. Nothing — not this
module, not a `SELECT`, not a backup — can produce it again. Losing it means
issuing another.

`PlaintextToken` is built to make an accidental second copy hard:

| Property | Why |
| --- | --- |
| no `Clone` | a credential that is trivially copied is a credential with an unknown number of copies |
| `Debug` prints `PlaintextToken([redacted])` | the common way a secret reaches a log file is a struct that derived `Debug` three types up |
| zeroized on drop | the plaintext does not linger in freed heap for a core dump or a later allocation to find |
| `expose()`, not `as_str()` | every call site should read as a decision |

The zeroize is best-effort, not a guarantee. Anything the caller copies the
string into — a response body, a format argument, a `String` of its own — is
outside the type's reach.

The plaintext is `arcpat_` followed by 32 hex characters of id, `_`, and 64
hex characters of secret: 104 characters in total. `TOKEN_PREFIX` is a public
const so a secret scanner — a pre-commit hook, a CI step, a log pipeline —
can recognise an Arcature token in a paste or a diff from one literal. The
prefix is not a security control; it is what makes one possible.

Failures from `issue`:

| Error | Cause |
| --- | --- |
| `ApiTokenError::Entropy` | the OS randomness source was unavailable. No fallback is attempted: a token minted from a clock or a counter is not a secret |
| `ApiTokenError::IdCollision { attempts: 8 }` | eight random 128-bit ids were all already taken, which in practice means the random source is broken. Reported rather than retried forever, because a loop would hide it |
| `ApiTokenError::Database` | the insert failed |

The insert is `ON CONFLICT (id) DO NOTHING` on PostgreSQL, `INSERT IGNORE` on
MySQL, `INSERT OR IGNORE` on SQLite. A clash arrives as zero rows affected
rather than as an error, which is what lets `issue` draw another id instead of
parsing a driver-specific constraint name out of an error string.

## The migration and the table

`tokens.migrate()` creates `arcature_api_tokens` and its indexes. It is
idempotent and safe to run from every replica at once. Call it at startup; a
store whose table is missing fails on the first request instead, which is the
same outage discovered by a user.

The migration is embedded per dialect and applied under an
`arcature_api_tokens_schema_migrations` history table. Statements are split on
a line reading `--;;` and executed one at a time, because MySQL rejects
multiple statements in a single prepared query unless the connection opted in.

| Dialect | Migration lock | Notes |
| --- | --- | --- |
| PostgreSQL | `pg_advisory_lock(71420003)` | a key of its own; `tests/advisory_locks.rs` is the registry and fails if two subsystems claim the same number |
| MySQL 8 | `GET_LOCK('arcature_api_tokens_migrate', 10)` | a lock name of its own, for the same reason |
| SQLite | none | it serialises writers itself, and every statement in the migration is `IF NOT EXISTS` or `INSERT OR IGNORE` |

The lock is taken on one acquired connection for the whole run, not one per
statement, because `pg_advisory_lock` and `GET_LOCK` are held by the session.
The unlock is best-effort: if it fails the session is already broken, and
reporting that instead of the migration's own error would hide the reason the
caller needs. On MySQL the wait is bounded at ten seconds and the result of
`GET_LOCK` is not inspected, so a migrator that waited out the timeout
proceeds anyway — which converges rather than corrupts, because every
statement in the file is idempotent.

Storage differs where the dialects differ:

| Column | PostgreSQL | MySQL 8 | SQLite |
| --- | --- | --- | --- |
| `id` | `BYTEA` | `BINARY(16)` | `BLOB` |
| `secret_digest` | `BYTEA` | `BINARY(32)` | `BLOB` |
| `tokenable_id`, `name` | `TEXT` | `VARCHAR(191)` | `TEXT` |
| `abilities` | `JSONB` | `JSON` | `TEXT` |
| `expires_at`, `created_at` | `TIMESTAMPTZ` | `DATETIME(6)` in UTC | `INTEGER` epoch milliseconds |

`sqlx::types::Json` covers all three abilities columns, so the store has one
code path. SQLite has no timestamp type: text timestamps compare correctly
only while every writer agrees on the format down to the digit, and integers
always do. The cost is that SQLite drops sub-millisecond precision on a round
trip. MySQL compares against `UTC_TIMESTAMP(6)` rather than `NOW()`, so a
connection set to a different session time zone cannot move an expiry.

Two indexes: `tokenable_id` for listing and mass revocation, `expires_at` for
the sweep.

## The extractor

`ApiAuth` reads the header, splits the credential, hashes the secret, and
compares it against the stored digest. The route body runs only if a live
token matched. The store reaches the extractor through an axum `Extension`, so
an application installs it once on the router:

```rust,ignore
use arcature::axum::{Extension, Router, http::StatusCode, routing::get};
use arcature::tokens::{ApiAuth, ApiTokens};

async fn deploy(ApiAuth(token): ApiAuth) -> Result<String, StatusCode> {
    // Authentication says who; the ability says what.
    if !token.can("deploy:write") {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(format!("deploying for {}", token.tokenable_id()))
}

fn routes(tokens: ApiTokens) -> Router {
    Router::new()
        .route("/deploy", get(deploy))
        .layer(Extension(tokens))
}
```

`ApiAuth` is a tuple struct over `ApiToken`, so destructuring it in the
argument list is the usual spelling. `ApiAuth::token()` and `ApiAuth::can()`
are there when it is bound whole.

What the extractor answers:

| Condition | Response |
| --- | --- |
| no `Authorization` header | `401`, `WWW-Authenticate: Bearer`, body `Authentication required` |
| a scheme other than `bearer` (matched case-insensitively) | the same `401` |
| `Bearer` with nothing after it | the same `401` |
| a header value that is not UTF-8 | the same `401` |
| a credential that is not the shape this module mints | the same `401`, with no query made |
| unknown id, wrong secret, or expired token | the same `401` |
| the store was never installed as an `Extension` | `500`, body `API tokens are not configured` |
| the database refused the query | `503` |

Every authentication failure is the same response with the same body. A client
that can tell them apart is being told about tokens it does not hold.

The two non-`401` rows are deliberate. A missing `Extension` is a wiring
mistake, and answering `401` would send a correct client away to mint a token
that would fail in exactly the same way. A database that is down is the
server's problem, and `401` would tell an honest client its credential was
bad.

`401` and not `403` for a missing token: `401` means "authenticate and try
again" and obliges the response to name a scheme, which this one does. `403`
is the answer a route gives after `ApiToken::can` returns false, as the
example above does.

## Why SHA-256 and not Argon2

This looks like the password path and is not, and the difference decides the
algorithm.

A password hash is slow on purpose because a password is low entropy. Users
pick from a distribution an attacker can enumerate — a wordlist, a leaked
corpus, a few billion candidates — so the only defence a stolen hash has is
that each guess costs real time and memory. Argon2 buys that time. It is the
right call in `auth::password`, and it stays the right call there.

A token secret is 32 bytes straight from the OS CSPRNG: 256 bits of uniform
randomness, with no distribution to guess from. Enumerating it is not
expensive, it is impossible. Multiplying an impossible search by Argon2's cost
factor leaves it impossible. The slow hash buys nothing, because the entropy
already did the work a slow hash exists to do.

The cost, meanwhile, is real and lands on every request. A token is presented
on each API call, so verification is on the hot path in a way a login never
is. Argon2's default parameters are tens of milliseconds and tens of
megabytes, tuned so that an attacker's GPU farm is slow. Put that in front of
every request and the tuning applies to the server: a client with a valid
token and a loop becomes a memory-hard workload generator. Rate limiting does
not save it, because the work happens before the request is known to be
abusive. Choosing Argon2 here would not harden the token; it would hand anyone
holding one a denial-of-service primitive against the application.

So: a single SHA-256, with no length-extension risk in this construction (the
input is a fixed 32 bytes and the digest is never a prefix of a longer
authenticated message). The reasoning generalises — hash slowly what humans
chose, hash quickly what the CSPRNG chose — and it is why `DbSessionStore`
stores a SHA-256 of a session id as well. The full argument lives at the
hashing site, `digest_of` in `src/tokens/store.rs`.

## The comparison is constant-time

From `ApiTokens::authenticate` in `src/tokens/store.rs`:

```rust,ignore
let stored: Vec<u8> = row.try_get(0)?;
let matches: bool = presented_digest.ct_eq(stored.as_slice()).into();
```

`subtle::ConstantTimeEq` reads every byte every time. A `==` would return at
the first differing byte. A few hundred nanoseconds, averaged over enough
requests, is enough to recover a digest one byte at a time — thirty-two
rounds of two hundred and fifty-six guesses, instead of a search of 2^256.

Two more details of that path. The presented secret is hashed *before* the
query, not after, so an unknown id and a known id with a wrong secret follow
the same code and differ by one constant-time comparison. And the digest is
selected by exactly one statement: `FIND` and `AUTHENTICATE` are identical
except that the second also selects `secret_digest`, which makes every other
read structurally incapable of loading a digest into memory. An integration
test asserts that against the column names the server reports, not against the
statement text, because the statement text is what a mistake would be written
in.

What remains observable is whatever the database leaks by finding a row versus
not finding one. That residue has the same shape as the account-enumeration
oracle a login form must not have, and it is acceptable here for a reason
worth stating: an email address is public and the password behind it is often
guessable, so "this account exists" is a real step forward, whereas a token id
is 128 random bits and the secret behind it is 256 more. Learning that some id
exists costs the same 128-bit search either way.

`authenticate` returns `Ok(None)` for every authentication failure —
malformed, unknown id, wrong secret, expired — and an `Err` only when the
database fails or a row does not hold what the schema promises.

## Abilities

A token carries a set of opaque strings the application chooses. Matching is
exact.

```rust,ignore
use arcature::tokens::Abilities;

let scoped = Abilities::of(["posts:read", "posts:write"]);
assert!(scoped.contains("posts:read"));
assert!(!scoped.contains("billing:write"));

// The one wildcard, and the only one.
assert!(Abilities::all().contains("anything at all"));

// No prefix matching: `posts:*` grants `posts:*` and nothing else.
assert!(!Abilities::of(["posts:*"]).contains("posts:read"));
```

| Constructor | Result |
| --- | --- |
| `Abilities::none()` | grants nothing. This is `Default`, and the default on a `NewApiToken` |
| `Abilities::of(iter)` | grants exactly those strings |
| `Abilities::all()` | the reserved `"*"` (`Abilities::ALL`), which matches every ability |
| `.with(ability)` | one more, by value |

`"posts:*"` is a legal ability string and it grants the ability spelled
`"posts:*"`. A wildcard grammar here would mean every application's
authorization decisions depend on this module's pattern matcher agreeing with
the application's intuition, which is not a bet worth taking in an
authorization path.

The default is closed. A token minted without naming an ability can do
nothing, and `ApiToken::can` returns false for everything. The only permissive
setting is one somebody typed: `Abilities::all()`.

Abilities are not checked by the extractor. `ApiAuth` proves the token is
live; the route calls `can`. There is no ability extractor, because the check
a route needs is a line of Rust and a second extractor would be a less
readable way to write it.

## Expiry

Every token has a deadline. `NewApiToken` has no constructor that omits one,
and the column has no null state to hold one. A credential that outlives the
reason it was minted is the ordinary way a leak stays useful, so "forever" has
to be typed out as a date somebody chose.

```rust,ignore
use arcature::tokens::NewApiToken;
use chrono::{Duration, Utc};

// Spell the deadline...
let explicit = NewApiToken::new("user:42", "laptop", Utc::now() + Duration::days(30));

// ...or the time to live.
let ttl = NewApiToken::expiring_in("user:42", "CI", std::time::Duration::from_secs(3600));
```

A `ttl` too large for the calendar saturates at the furthest instant `chrono`
can represent rather than wrapping into the past, because wrapping would mint
a token that is already dead.

Every read carries `expires_at > now()`, evaluated by the database, in `FIND`,
`AUTHENTICATE` and `LIST_FOR` alike. Three consequences:

- An expired token stops working the instant it expires, whether or not any
  sweep has run.
- The clock that decides is the database server's, not the reader's. A token
  does not outlive its expiry by however far one web node's clock is fast.
- An `ApiToken` that came back from a query is live by construction.
  `expires_at()` on it is in the future as of the moment of the read.

## Revocation and sweeping

```rust,ignore
let revoked: bool = tokens.revoke(id).await?;             // one token
let count: u64 = tokens.revoke_all_for("user:42").await?; // sign out everywhere
let reclaimed: u64 = tokens.sweep_expired().await?;       // disk, not security
```

Revocation is a `DELETE`, not a flag. A revoked row that is still in the table
is a row some future query can forget to filter; a row that is gone cannot
authenticate anybody by accident. `revoke` reports whether there was a token
to revoke; `revoke_all_for` reports how many went. The second is the "the
laptop was stolen" path, and the one to call when a password changes.

`sweep_expired` deletes rows whose expiry has passed and reports how many. It
reclaims disk. It is not what makes expiry correct — the predicate in every
read already does that — so a deployment that never calls it is secure and
merely wasteful. Nothing calls it for you; see the limits below.

Reading, for a token-management screen:

```rust,ignore
for token in tokens.list_for("user:42").await? {
    println!("{} ({}) expires {}", token.name(), token.id(), token.expires_at());
}
```

`list_for` returns every live token for one subject, newest first, ties broken
by id (`ORDER BY created_at DESC, id`). `find(id)` reads one. Neither can
return a plaintext, and neither loads a digest. `ApiTokenId` is safe to show,
to log, and to accept from a revocation request — it travels in the header in
the clear next to the secret, and it is not a credential. It parses from and
prints as 32 lowercase hex characters, via `ApiTokenId::from_hex` and
`to_hex`.

## Independent of `auth`, on purpose

`api-tokens` implies `database` and nothing else. It does not imply `auth`.

An API with no passwords and no sessions may still hand out a token. Making it
compile a password hasher and a session layer to get one would be a packaging
decision pretending to be a security one. `database` is required because a
revocable credential has to live somewhere revocable.

The two sides do not know about each other. `tokenable_id` is a `String` in
the application's own spelling — a user id, a tenant id, a service name — so
this module never has an opinion about the shape of an application's primary
key, and never joins to a users table.

## CSRF steps aside for a bearer request

`CsrfLayer` exempts any unsafe request carrying an `Authorization: Bearer`
header: no double-submit check, and no CSRF cookie injected on the way out.
The decision is made by `is_bearer_request` in `src/auth/csrf.rs`, which takes
the bytes up to the first ASCII whitespace and compares them
case-insensitively against `bearer`. `Basic` is not exempt, and a request with
no `Authorization` header is not exempt.

The reason is that a bearer request is not browser-driven. Double-submit
defends a cookie-authenticated browser against a forged cross-site request; a
request that authenticates with a header the application handed to a CLI is
not that request, and there is no cookie for a forged one to ride on.

Two notes about the shape of the exemption. It is keyed on the scheme alone,
so it is granted before any token is validated — the request is then still
rejected by `ApiAuth` unless it carries a live token, so the exemption skips
CSRF, not authentication. And a route protected only by a session cookie gains
nothing from a client that also sends a bearer header; a route that means to
accept tokens should read `ApiAuth`.

## What this does not do

**No OAuth.** No authorization server, no grant types, no consent screen, no
client registration, no refresh endpoint, no discovery document, and no
`scope` parameter. Abilities are the application's own strings and mean
whatever the application decides. The separate `oauth` feature is an OAuth 2
*client* — Authorization Code with PKCE against somebody else's provider —
and shares no code with this module.

**No refresh tokens, and no rotation.** The store's only writes are `issue`,
`revoke`, `revoke_all_for` and `sweep_expired`. There is no method that
modifies a row, so a token's expiry cannot be extended and its abilities
cannot be edited. Both mean issuing a new token and revoking the old one.

**No usage tracking.** The table has no `last_used_at` and no counter, and
`authenticate` performs no write. A token screen cannot show "last used three
days ago".

**No caching, and no rate limiting.** Every authenticated request is one
`SELECT`. Nothing counts or throttles failed presentations the way
`auth::flows` throttles failed logins. The 256-bit secret is the whole defence
against guessing, which is the same reason the digest is fast.

**No layer, and no framework wiring.** `ApiAuth` is a per-route extractor.
There is no `ApiTokenLayer` that protects a router subtree, and no
`Application::tokens(..)` helper of the kind `Application::storage(config)`
is. Install the store with `Extension` yourself.

**No history.** Revocation is a `DELETE` and every read filters on expiry, so
there is no way to list expired or revoked tokens, and no audit record of who
revoked what.

**No scheduled sweep.** `sweep_expired` runs when the application calls it.
Nothing in the framework calls it, and there is no `arc` subcommand for tokens
the way there is `arc queue work`. Wire it into a [jobs](jobs.md) schedule if
you want one.

**No transaction variants.** Unlike the job queue's `enqueue_tx` and
`migrate_tx`, every method here runs on the pool. A token cannot be issued
inside a transaction you already hold.

**No serde.** `ApiToken` derives `Clone` and `Debug` only — not even
`PartialEq`, so two tokens cannot be compared with `==`. `ApiTokenId` and
`Abilities` add comparison traits. None of the three derives `Serialize`. Rendering a token list into an Inertia
prop or a JSON body means a struct of the application's own.

**No relationship to a user row.** `tokenable_id` has no foreign key and no
cascade in any dialect, so deleting a user does not delete their tokens. Call
`revoke_all_for`.

**No limits on quantity, names, or length.** A subject may hold any number of
tokens, two of them may share a name, and `list_for` returns all of them in
one `Vec` with no pagination. Nothing validates the length of `tokenable_id`
or `name`; on MySQL both columns are `VARCHAR(191)` and longer values do not
fit.

**No transport check.** Nothing here verifies that the request arrived over
TLS. A bearer token is a password in a header, and the reverse-proxy front
door owns TLS termination, as it does for the rest of the framework.
