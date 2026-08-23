# Encryption and signed URLs

Two things an application does with its own secret. `Encrypter` turns bytes
into an opaque token it can hand out and get back. `UrlSigner` mints a link
that proves the application issued it and, optionally, carries its own
deadline.

Both hang off one secret, `APP_KEY`. Neither is on by default.

## Two features, not one

They are separate features because they cost different things. Signing needs
a MAC; encrypting needs a cipher. An application that only hands out one-hour
download links has no reason to pull an AEAD into its graph, and an
application that only encrypts has no reason to carry a URL parser.

Neither `crypt` nor `signed-urls` is in `default`, and neither is in
`fullstack`. Turning one on is a decision written into the manifest: the
moment a build can produce ciphertext is the moment somebody owns a key
rotation story.

```toml
arcature = { version = "0.1", features = ["crypt", "signed-urls"] }
```

| Feature | Gives you | Pulls |
| --- | --- | --- |
| `crypt` | `Encrypter`, `EncryptError`, `DecryptError` | `chacha20poly1305`, `secrecy`, `zeroize`, `hmac`, `sha2` |
| `signed-urls` | `UrlSigner`, `Clock`, `SystemClock`, `SignedUrlError` | `secrecy`, `zeroize`, `hmac`, `sha2`, `subtle`, `percent-encoding` |

`AppKey` and `AppKeyError` are exported when **either** feature is on. The
module `arcature::crypt` does not exist when both are off; the whole surface
is also re-exported at the crate root (`arcature::Encrypter`,
`arcature::UrlSigner`).

Why each crate is there:

| Crate | Load it carries |
| --- | --- |
| `chacha20poly1305` | the AEAD behind `Encrypter`, in its **X** variant |
| `hmac` + `sha2` | subkey derivation from `APP_KEY`, and the MAC `UrlSigner` computes |
| `subtle` | `ConstantTimeEq` on the presented signature -- see below |
| `percent-encoding` | escaping and unescaping query components |
| `secrecy` + `zeroize` | key material that redacts in `Debug` and wipes on drop |

There is no `dep:subtle` under `crypt`. The only comparison that feature makes
is the AEAD's own tag check, which `chacha20poly1305` already does in constant
time.

`auth-flows` implies `signed-urls`, because an email-verification link is a
signed URL with an address binding on top. A build with `auth-flows`,
`auth-reset` or `auth-remember` already has `UrlSigner`.

## The application key

`APP_KEY` is 64 bytes, written into `.env` as 128 lowercase hexadecimal
characters. It is the same material `arcature::auth::SessionKey` holds. There
is deliberately one secret per deployment: one thing to rotate, one thing to
store, one thing to keep out of a repository.

```console
$ arc key:generate
APP_KEY written to .env

$ arc key:generate --show
APP_KEY=<128 lowercase hexadecimal characters>
```

| Behaviour | Detail |
| --- | --- |
| source | 64 bytes from the OS RNG, via `SessionKey::generate` |
| encoding | 128 lowercase hex characters, not base64 -- hex has no alphabet variants for a `.env` parser to get wrong, and length alone says whether the value is intact |
| default action | rewrite the existing `APP_KEY=` line in `.env`, or append one |
| re-running | replaces rather than appends, so a `.env` never ends up with three keys and a loader picking one |
| `--show` | prints and touches nothing, which is what a pipeline wants when the secret belongs in a secret store |
| no `.env` | an error naming the path, suggesting `--show` |
| RNG failure | an error; no key is generated |

The command is gated on the `auth` feature, not on `crypt` or `signed-urls`,
because the key type and the certified RNG behind it live in the auth module.
`auth` is in `default`, so a generated application has the command. A build
that turns default features off and asks for `crypt` alone gets `AppKey` and
no way to mint one from the CLI -- produce 64 bytes from the OS RNG elsewhere
and pass them to `AppKey::from_bytes`.

### Reading the key

```rust,ignore
use arcature::crypt::AppKey;

let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
```

`AppKey::from_bytes(&[u8])` is the other constructor and takes exactly 64
bytes.

`from_hex` trims surrounding whitespace and accepts either case. The generator
writes lowercase, but a value that has been through a secret store and back
may not have stayed that way, and rejecting an unambiguous key on its
capitalisation would be a fault report with no fault behind it.

The hex decoder is written out rather than built on `u8::from_str_radix`,
which accepts a leading sign: with it, a 128-character string of `+4` decodes
to 64 valid bytes. A decoder for a secret should accept exactly one spelling
of it, so `+4`, `-4` and `4 ` are all refused, and a non-ASCII character is
refused rather than sliced mid-codepoint.

| `AppKeyError` | Means |
| --- | --- |
| `Empty` | nothing but whitespace |
| `NotHexadecimal` | a character outside `0-9a-fA-F`, or an odd number of digits |
| `WrongLength` | decoded, but not to 64 bytes |

Every variant is about the value's shape, never its content, so an error that
reaches a log carries no secret with it. Every `Display` ends with the same
instruction: run `arc key:generate` to write a valid one into `.env`.

### Nothing uses those 64 bytes directly

`APP_KEY` is never handed to a cipher or a MAC. Each consumer gets its own
32-byte subkey:

```text
subkey(label) = HMAC-SHA256(APP_KEY, "arcature/kdf/v1" || len(label) || label)
```

`len(label)` is a big-endian `u64`. It is there so no two labels can produce
the same input: without it, `"ab"` then `"c"` and `"a"` then `"bc"` are one
string, and a future label could collide with a present one.

| Label | Consumer |
| --- | --- |
| `encrypter` | `Encrypter` |
| `url-signer` | `UrlSigner` |

Sharing one key across two algorithms is how a weakness in either becomes a
weakness in both, and how a chosen-ciphertext oracle in one becomes a forgery
in the other. Recovering the signing key does not hand anybody the ability to
decrypt.

The derivation is a single PRF call rather than a full HKDF because there is
nothing to extract from: `APP_KEY` is already 64 uniformly random bytes from
the OS RNG, which is exactly what HKDF's expand step wants as input. It is
also exactly one HMAC block wide, so it is used as an HMAC key without being
pre-hashed.

`AppKey::subkey` is `pub(crate)`. A caller who can name a label can make two
subsystems share a key, which is the mistake the derivation exists to
prevent. Labels are constants in the module.

The bytes live in a `secrecy::SecretSlice`, which zeroizes on drop, and there
is no accessor that hands the key out. `Debug` prints
`AppKey(<redacted 64-byte key>)`.

## `Encrypter`

For values an application has to hand out and get back: a token in a link, an
opaque cursor, a payload in a queue an operator can read.

```rust,ignore
use arcature::crypt::{AppKey, Encrypter};

let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
let encrypter = Encrypter::new(&key);

let token = encrypter.encrypt_string("order 4417")?;
assert!(token.starts_with("v1."));
assert_eq!(encrypter.decrypt_string(&token)?, "order 4417");
```

| Method | Signature |
| --- | --- |
| `Encrypter::new(&AppKey)` | derives the `encrypter` subkey |
| `encrypt(&[u8])` | `Result<String, EncryptError>` |
| `encrypt_string(&str)` | `Result<String, EncryptError>` |
| `decrypt(&str)` | `Result<Vec<u8>, DecryptError>` |
| `decrypt_string(&str)` | `Result<String, DecryptError>` |

`new` is cheap enough to call per request, though an application will normally
build one at startup and keep it in state. `Debug` prints
`Encrypter(XChaCha20-Poly1305, <redacted key>)`.

### Why XChaCha20-Poly1305

Every message gets a fresh **192-bit** nonce from the OS RNG. That width is
the whole reason for the X variant: at 192 bits a random draw per message is
safe for any number of messages, so there is no counter for a caller to
manage and no way for a caller to reuse one.

AES-GCM's nonce is 96 bits. Random nonces there have a birthday bound a busy
application can actually reach, and a repeat under GCM does not merely leak
plaintext -- it leaks the authentication subkey, so the attacker gains forgery
as well. The alternative to a 192-bit nonce is asking every caller to own a
counter that must never repeat across restarts, replicas and rollbacks.

The nonce comes from `getrandom`, which this crate depends on
unconditionally, and an RNG failure is `EncryptError::Rng` rather than a
fallback: a nonce from anything but the OS RNG is not a nonce.

### Why not `ring` or `aws-lc-rs`

`chacha20poly1305` is RustCrypto, pure Rust, compiled here with
`default-features = false` (`alloc` for the `Vec` output, `zeroize` to wipe
the key schedule on drop). `ring` and `aws-lc-rs` both carry C and assembly.

This is not a claim that no C runs in the process. A default build already
links `aws-lc-rs`: `sqlx` selects `tls-rustls-aws-lc-rs` and `lettre` selects
`aws-lc-rs`, both for TLS. The narrower decision is about which bytes reach
it. A token pulled out of a query string or a cookie is attacker-authored
input arriving at a parser, and this module's threat model is that such bytes
are handled by pure Rust with `unsafe_code = "forbid"` above them.

### The token format

```text
v1.<base64url( nonce || ciphertext || tag )>
```

| Part | Size |
| --- | --- |
| version tag | the literal `v1.` |
| nonce | 24 bytes |
| tag | 16 bytes (Poly1305) |
| ciphertext | the plaintext's length |

So a token is `ceil((n + 40) * 4 / 3) + 3` characters for an `n`-byte
plaintext. The body is unpadded base64url, so a token is safe in a URL path, a
query value, a cookie and a JSON string with no further escaping.

Associated data is the fixed string `arcature/crypt/v1`. It is not secret and
it is not the message; it is a statement the tag covers. Binding the version
means a `v1` token cannot be relabelled as a `v2` one, so a future version
that weakens something cannot be reached by editing four characters of an
existing token.

Encryption is randomised. The same plaintext encrypted twice gives two
different tokens, by design. An empty plaintext round-trips.

### It fails closed

A token whose bytes have changed returns no plaintext at all. Which error
depends on where: the version tag is stripped before anything else, so
altering it gives `DecryptError::UnknownVersion`, and a body that is not
base64url gives `Malformed`. Everything the cipher actually sees -- nonce,
ciphertext, tag -- gives `DecryptError::Authentication`. There
is no partial result and no "decrypted but unverified" path, because a caller
holding attacker-chosen bytes that look like plaintext is the failure mode an
AEAD exists to prevent.

| `EncryptError` | Means |
| --- | --- |
| `Rng` | the OS RNG failed; nothing was encrypted |
| `Oversized` | the plaintext exceeds what the cipher can address; not reachable on a 64-bit target |

| `DecryptError` | Means |
| --- | --- |
| `UnknownVersion` | no version tag this build reads -- not an Arcature token, or minted by a newer release |
| `Malformed` | not unpadded base64url, or shorter than a nonce plus a tag (40 bytes) |
| `Authentication` | the tag did not match: altered, or encrypted under a different key |
| `NotUtf8` | authenticated, but the plaintext is not UTF-8. Only from `decrypt_string`, and not reachable by an attacker -- it means `encrypt` was given bytes and `decrypt_string` was used to read them back |

The variants distinguish shapes of failure so an application can tell a stale
link from an attack in its own logs. None of them is a near-miss: every one
means no plaintext was produced.

### Not a password store, not a database column

Passwords go through `arcature::auth::PasswordHasher`. Hashing is one-way and
encryption is not, so a stolen `APP_KEY` turns an encrypted password table
into a plaintext one.

Encrypting a column you then want to query is also a trap. Two encryptions of
one value differ, by design, so `WHERE email = ?` finds nothing.

## `UrlSigner`

A signed URL is the answer to "let this one person fetch this one thing,
without giving them an account and without leaving the door open". The link is
self-contained: nothing is written down when it is issued and nothing is
looked up when it is presented, so it costs a MAC to make and a MAC to check.

```rust,ignore
use std::time::Duration;
use arcature::config::AppConfig;
use arcature::crypt::{AppKey, UrlSigner};

let key = AppKey::from_hex(&arcature::config::env_required("APP_KEY")?)?;
let config = AppConfig::new().url("https://example.com");
let signer = UrlSigner::new(&key, &config);

// Expires in an hour.
let url = signer.sign_temporary("/invoices/9", &[("as", "pdf")], Duration::from_secs(3600))?;
// https://example.com/invoices/9?as=pdf&expires=<unix seconds>&signature=v1.<base64url>

signer.verify(&url)?;
```

| Method | Behaviour |
| --- | --- |
| `UrlSigner::new(&AppKey, &AppConfig)` | derives the `url-signer` subkey, roots links at `config.base_url()` |
| `.with_clock(Arc<dyn Clock>)` | replaces the clock expiry is measured against |
| `.sign(path, params)` | a link with **no deadline** |
| `.sign_temporary(path, params, valid_for)` | a link that stops verifying `valid_for` from now |
| `.verify(url)` | `Result<(), SignedUrlError>` |

`params` is `&[(&str, &str)]`. `Debug` prints the base URL and `<redacted>` in
place of the key.

### Defaults

| Thing | Default |
| --- | --- |
| base URL | `AppConfig::base_url()`, which is `http://localhost:3000` on a fresh `AppConfig::new()` and drops any trailing slash |
| clock | `SystemClock`, the wall clock |
| deadline from `sign` | **none at all** -- a URL signed with `sign` verifies forever |
| deadline from `sign_temporary` | whatever you pass; nothing caps it |
| MAC | HMAC-SHA256, untruncated, all 32 bytes |

The base URL comes from `APP_URL` rather than from a request's `Host` header
because a signed link is usually built with no request in scope -- it goes in
an email, or in a job's output -- and behind a reverse proxy that header is
not authoritative anyway.

`sign` is there for a link whose validity is a property of the target rather
than of time: an unsubscribe link, say, which stops working when the
subscription does. Prefer `sign_temporary` whenever a deadline makes sense. A
permanent signed URL is a bearer token with no end date.

### The expiry is inside the signature

`sign_temporary` writes the deadline into the `expires` query parameter as a
Unix timestamp in seconds, and the MAC covers it along with the path and every
other parameter. Moving the expiry forward changes the signed material, so the
edited link returns `Mismatch`. An expiry outside the signature would make the
whole feature decorative.

A URL is valid up to **and including** its expiry second, and invalid from the
next one. With a signer frozen at second 1000 and a 60-second window: 1059
verifies, 1060 verifies, 1061 is `Expired`.

`Clock` is a trait (`fn now_unix(&self) -> u64`, `Send + Sync + 'static`) for
one reason: a test for "the link stops working after an hour" written against
the real clock either sleeps for an hour or proves nothing.

```rust,ignore
use std::sync::Arc;
use std::time::Duration;
use arcature::crypt::{Clock, SignedUrlError, UrlSigner};

struct Frozen(u64);
impl Clock for Frozen {
    fn now_unix(&self) -> u64 { self.0 }
}

let at = |second| UrlSigner::new(&key, &config).with_clock(Arc::new(Frozen(second)));

let url = at(100).sign_temporary("/download/42", &[], Duration::from_secs(30))?;
assert_eq!(at(130).verify(&url), Ok(()));
assert_eq!(at(131).verify(&url), Err(SignedUrlError::Expired));
```

`SystemClock` reads a pre-epoch wall clock as `0`, and **that fails open, not
closed**. The expiry check is `if self.clock.now_unix() > expires_at`, so a
clock reporting `0` is never past any deadline and every expired link is
accepted. A machine whose clock has fallen behind the epoch honours links
forever rather than refusing them.

This is a real if unlikely failure mode -- it needs a system clock set before
1970, which in practice means a dead RTC battery or a deliberately wound-back
container. It is written down rather than fixed because the fix is a decision
about what an application should do when it cannot tell the time, and that is
not the signer's call to make. If it matters to you, supply your own
[`Clock`](https://docs.rs/arcature/latest/arcature/crypt/trait.Clock.html)
that refuses rather than returning a sentinel.

### Constant-time comparison

`verify` compares the presented MAC with the expected one using
`subtle::ConstantTimeEq`, never `==`:

```rust,ignore
if !bool::from(signature.as_slice().ct_eq(&expected)) {
    return Err(SignedUrlError::Mismatch);
}
```

A byte-by-byte comparison that returns at the first difference is a timing
oracle. An attacker who can measure it recovers a valid signature one byte at
a time -- roughly 8,000 guesses instead of 2^256. `ConstantTimeEq` reads every
byte every time, and it is a dependency rather than a five-line loop precisely
because its job is to be the thing the optimiser is not allowed to turn back
into an early return. There is no `==` on a signature anywhere in the crate.

The signature is checked **before** the expiry, so a tampered URL reports
tampering whatever its deadline claims. Reading the expiry only after the MAC
matched is what makes the parsed timestamp trustworthy.

### What is signed, and how it is canonicalised

The MAC input is, in order: the domain separator `arcature/signed-url/v1`; the
path, length-prefixed; the parameter count as a big-endian `u64`; then each
parameter's name and value, length-prefixed, sorted by name and then by value.

Length prefixes are load-bearing. Without them, `a=bc` and `ab=c` feed the MAC
one identical string, and an attacker who can move a character across the
boundary has a forgery for free.

Sorting is what lets a reordered query still verify. Parameters are compared
after percent-decoding, so a link a mail client or a redirect has re-escaped
still verifies; a link that has been *edited* does not.

On the way out, everything outside RFC 3986's unreserved set
(`A-Za-z0-9-._~`) is percent-encoded -- including `&`, `=`, `+`, `%`, `#`,
space and every non-ASCII byte -- so a parameter value can never introduce a
parameter. A space becomes `%20` and never `+`: `+` is an
`application/x-www-form-urlencoded` convention, not a URL one, and a verifier
that undid it would decode a literal `+` in a signed value into something that
was never signed.

A path is canonicalised to exactly one leading slash, so `"reports/7"` and
`"/reports/7"` sign the same thing.

### What `verify` accepts and rejects

`url` may be absolute, as `sign` returns it, or the path-and-query a request
carries.

| Input | Result |
| --- | --- |
| the absolute URL as minted | verifies |
| the same, stripped to `/path?query` | verifies |
| the same with `#anything` appended | verifies -- a fragment is never sent to a server, so it is not signed and not checked |
| the query reordered | verifies |
| any parameter edited | `Mismatch` |
| `https://example.com.evil/...` | `ForeignOrigin`, decided before any MAC is computed |
| a relative URL not starting with `/` | `ForeignOrigin` |
| no `signature` parameter | `MissingSignature` |
| two `signature` parameters | `Malformed` |
| a signature with no `v1.` tag | `UnknownSignatureVersion` |
| a signature that is not base64url | `Malformed` |
| an unreadable percent-escape | `Malformed` |
| genuine, past its deadline | `Expired` |
| genuine, no `expires` at all | verifies, at any time |

The origin is not in the MAC because it does not have to be: the URL must sit
under the configured `APP_URL`, which is checked first. The prefix check
carries an explicit guard -- the remainder must be empty or start with `/` or
`?` -- because without it `https://example.com` is a prefix of
`https://example.com.evil/x`. The comparison is a literal string prefix, so it
is sensitive to scheme, host case and port.

Two reserved names cannot be passed as parameters, because a caller that could
set them could contradict the signer:

| Signing input | Result |
| --- | --- |
| a parameter named `signature` or `expires` | `ReservedParameter` |
| a `path` containing `?` or `#` | `QueryInPath` -- pass the query as `params` so it is canonicalised and signed rather than appended unsigned |

`Mismatch` and `Expired` are worth logging apart. The first is somebody editing
a link; the second is somebody using a link too late. An application that
treats them alike cannot tell an attack from a slow reader.

## The base64url decoder is written in-crate

Both formats encode with an unpadded base64url (RFC 4648 section 5) written
inside the crate rather than pulled from a dependency. It is `pub(crate)`:
there is one implementation, not two, because a second decoder is a second
place for a padding bug that makes two spellings of one token both valid.

Two properties are wanted that a general-purpose encoder does not promise.

The alphabet has to stay stable forever, because it is baked into every token
the module has ever issued and a token outlives the release that minted it.

The decoder has to be strict. It rejects:

| Input | Why |
| --- | --- |
| padding (`Zg==`) | the encoder never writes it |
| any character outside `A-Za-z0-9-_` | including the standard alphabet's `+` and `/` |
| a length congruent to 1 modulo 4 (`Z`, `Zm9vY`) | no byte string encodes to that length |
| non-canonical trailing bits (`Zh`, `Zm9`) | the unused low bits of a final group are padding the encoder wrote as zero |

The result is that a byte string has exactly **one** spelling. `Zg` decodes to
`0x66`; `Zh` carries the same byte in its top eight bits and is refused. A lax
decoder gives an attacker a family of distinct strings that decode alike,
which is how a token revoked by string comparison comes back to life under
another name.

Sixty lines of table lookup is a smaller thing to own than a dependency whose
behaviour on malformed input is a version-to-version decision. The same
reasoning produced the hand-written hex in `arc key:generate` and in
`AppKey::from_hex`.

## Everything carries a version

| Value | Version tag |
| --- | --- |
| an encrypted token | the leading `v1.` |
| a URL signature | `v1.` inside the `signature` parameter value, not a parameter of its own |
| the key schedule | `arcature/kdf/v1`, the derivation domain |

Replacing an algorithm later is therefore additive: the new reader keeps the
old branch, tokens and links already in flight keep working, and nothing has
to be re-issued during a deploy. A format with no version can only ever be
changed by breaking every holder of an outstanding token at once.

The signature version rides inside the parameter value so a future format is a
change to one string a verifier already parses, rather than a new parameter
every old verifier ignores.

Changing the derivation domain would be a different matter: it changes every
subkey, and so invalidates every token and every signature in flight. A `v2`
there would be introduced alongside `v1`, not in place of it.

## What is not here

No middleware and no extractor. `verify` is a call a handler makes; nothing in
the routing layer checks a signature for you, and there is no
`SignedUrl`-shaped extractor that rejects before your code runs.

No key store, no key registry, no key identifier in either format. No JWT, no
PASETO, no public-key signature: both formats are this crate's own and both
are symmetric.

No storage of any kind. Signing writes no row and verification reads none.

## Limits

### Key rotation is the application's problem

There is exactly one `APP_KEY`, one subkey per label, and no key identifier
anywhere in a token or a signature. A verifier tries one key, and that key is
whatever the process was started with.

What that means concretely, the day `APP_KEY` changes:

| Outstanding thing | Outcome after rotation |
| --- | --- |
| every encrypted token | `DecryptError::Authentication` -- indistinguishable, by design, from a forgery |
| every signed URL | `SignedUrlError::Mismatch` |
| every signed session cookie | invalid, because `SessionKey` is the same material |

Nothing in the module reads a second key, so there is no overlap window to
configure. If you need one, it is yours to build: keep the old `AppKey`
alongside the new one, try the new one first, fall back to the old, and stop
falling back once the longest deadline you ever minted has passed. The module
gives you the pieces for that -- `AppKey::from_bytes`, and an `Encrypter` or
`UrlSigner` per key -- and none of the policy.

The corollary is that a deadline you mint is a commitment. A permanent signed
URL from `sign` outlives every rotation plan you have.

### A signed link is replayable until it expires

A signed URL is a bearer token in a query string. Presenting it is the whole
of proving you may have it, and presenting it twice works exactly as well as
presenting it once. Nothing is recorded at signing time and nothing is
consulted at verification time -- that statelessness is what makes the link
cost one MAC, and it is also what makes it replayable.

It will end up in browser history, in `Referer` headers, in access logs and in
whatever archived the email. Within its window, anybody who reads it in any of
those places can use it.

Two things follow.

Give a link the shortest lifetime the use allows, and do not sign an action a
replay would make worse. A download link is a reasonable thing to sign; "close
this account" is not.

If a link must be single-use, the application spends it. Record something when
the link is redeemed -- a nonce parameter marked used, a row's state moved on,
a version column bumped -- and check that record after `verify` returns `Ok`.
`verify` answers "did we issue this, and is it still in date". It cannot
answer "has this already been used", because it never learns that anything
was.

For the email-verification case the framework does part of this for you: the
`auth-flows` link binds the address it was minted for, so a link is refused
once the account's address changes. That is a binding, not a spend counter.

### An encrypted token has no deadline and no context

`Encrypter` is not `UrlSigner`. A token has no expiry field, and nothing in
the module ever refuses one for being old. A token minted today decrypts in a
year under the same key. If a deadline matters, put a timestamp in the
plaintext and check it after decrypting.

The associated data is the constant `arcature/crypt/v1`. There is no parameter
for caller-supplied context, so a token minted at one call site decrypts
cleanly at another. If two purposes in one application must not accept each
other's tokens, put the purpose in the plaintext and check it after
decrypting.

### The path is signed as literal bytes

`sign` rejects `?` and `#` in a path and otherwise passes it through
unchanged: the path is not percent-encoded on the way out, and `verify` does
not percent-decode it on the way in. Both sides MAC the same literal string,
with leading slashes collapsed to one.

So anything that rewrites the path in transit -- a client that escapes a
space, a proxy that resolves a `..` segment, a router that normalises a
trailing slash -- produces `Mismatch`. That fails in the safe direction, but
it fails. Build signed paths out of characters that survive a round trip, and
put anything else in a parameter, where it is escaped and unescaped for you.

## What this module does not own

No cipher implementation, no MAC implementation, no hash. `chacha20poly1305`
owns the AEAD, `hmac` and `sha2` own the MAC, `subtle` owns the constant-time
comparison, `percent-encoding` owns the escaping, `secrecy` and `zeroize` own
the key handling. All of them are pure Rust with no C and no assembly, and the
crate compiles under `unsafe_code = "forbid"`.

What the module owns is the composition: the key schedule, the two token
formats, the canonical form the MAC is computed over, and the strict decoder
that gives every token exactly one spelling.
