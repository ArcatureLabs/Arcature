# 0002 — The CSRF cookie is `XSRF-TOKEN`, not `__Host-csrf`

## Decision

`CsrfConfig::new()` — the default — issues a `__Host-csrf` cookie and expects
`x-csrf-token`, with `Secure` and `SameSite=Strict`. That is the strong
configuration and it stays the default.

`CsrfConfig::inertia()` issues `XSRF-TOKEN` and expects `x-xsrf-token`, with
`Secure` and `SameSite=Lax`. Inertia applications use this one. Both live in
`src/auth/csrf.rs`, and this record repeats the reasoning already in
`CsrfConfig::inertia`'s doc comment rather than replacing it.

## Context

Inertia's client is axios. axios reads a cookie named exactly `XSRF-TOKEN` and
sends it back in a header named exactly `X-XSRF-TOKEN`. Those names are not
configurable per request in any way that survives Inertia's own use of the
instance; they are what the client does.

Against `CsrfConfig::new()`, a stock Inertia form post is rejected with 403. The
application's options are to write JavaScript that reads the token and
reconfigures axios, or to change the names on the server. The first option is a
client-side shim — precisely the kind of framework-owned JavaScript this project
does not publish ([0001](0001-no-npm-package.md)) — and an application that
writes it once will write it in every project.

The server is where the names are cheap to change, so the server moves to meet
the client. This is the whole of "zero-config CSRF for Inertia": a name change,
not a mechanism change. The double-submit check, the safe-method exemption, the
bearer-token exemption, and the constant-time comparison are identical between
the two configurations.

## Cost

Two cookie attributes weaken, deliberately.

**No `__Host-` prefix.** The prefix mandates `Secure`, forbids `Domain`, and
pins `Path=/`, which is what stops a sibling subdomain from overwriting the
cookie. axios will not look for a prefixed name, so it goes. The exposure is an
attacker who already controls a subdomain of the site overwriting `XSRF-TOKEN`
with a value they know — a session-fixation-shaped attack on the nonce, not a
way to read the real one. It matters only after a subdomain is already lost.

**`SameSite=Lax` rather than `Strict`.** Strict withholds the cookie on every
cross-site navigation, including an OAuth callback and a link from email, so
the first page load after one arrives without a token and the next form post
fails. Lax sends it on top-level GET navigations only. That is exactly the case
Strict breaks, and it is not a case CSRF exploits: a forged unsafe request still
arrives without the cookie.

An application that would rather keep `CsrfConfig::new()` and configure axios
itself can. The auth chapter of the guide shows what that takes.
