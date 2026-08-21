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

## What Arcature does not claim

Arcature writes no cryptography. Argon2id, HMAC, SHA-2 and TLS come from
RustCrypto, `cookie`, and the rustls + aws-lc-rs stack. A cryptographic flaw in
one of those belongs upstream.

Arcature is not a substitute for the reverse proxy in front of it. TLS
termination, rate limiting and connection-level request-size limits are the
front door's job, and the framework's defences assume it is doing it.
