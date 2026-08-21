# 0004 — Layer order is a written contract

## Decision

The order in which Tower layers wrap the router is fixed in one place —
`src/application/pipeline.rs` — and documented there as a numbered table with a
justification per row. It does not follow the order the builder methods were
called in. `.inertia()` before `.csrf()` and `.csrf()` before `.inertia()`
produce the same pipeline.

`ApplicationBuilder` collects layers into named slots. `Pipeline::apply`
imposes the order, applying stages inside-out because `Router::layer` wraps
whatever is already on the router. Two stages — the dev proxy and the
pre-routing proxy — wrap the router as a service rather than as a `Router`, so
`compose_service` handles those; that split is why the module has two functions
instead of one.

## Context

Layer order is not a style question. It determines behaviour, and it determines
it in ways that are invisible until the day they matter:

- Security headers outside the body limit and the timeout means a 413 and a 408
  carry `nosniff` and a framing policy. Inside, they do not — and a 413 is a
  document a browser renders.
- The access log outside the panic catcher means a 500 is logged. Inside, the
  panic is caught and the line never written.
- The session outside CSRF means the token can be looked up. The other way
  round, every unsafe request is rejected.
- Inertia innermost of the framework layers means a CSRF rejection or a timeout
  is returned as itself, rather than dressed up as an Inertia response the
  client will try to render as a page.

If the order came from builder call order, every one of those is a property of
how somebody happened to write their `main`. The behaviour would be correct in
the scaffold and quietly wrong the first time someone reordered the chain to
group related calls together — a refactor with no visible risk that silently
removes headers from error responses.

Fixing the order also makes it reviewable. A table with one reason per row is a
thing a person can read and disagree with. A behaviour that emerges from call
order is not written down anywhere and can only be discovered by experiment.

Several rows are deliberate deviations from the obvious arrangement, and each
says so in the table: the response-shaping and observability stages sit
*outside* the body limit and the timeout, not inside; maintenance sits outside
the session, so a `503` does not depend on the store being maintained and a
form POST during the window gets the `503` rather than a CSRF rejection; the
health endpoints are *merged beside* the router rather than layered over it, so
a liveness probe every few seconds costs no session load and no log line.

The ordering assertions in `tests/application.rs` were checked by mutation —
moving Inertia, CSRF or the timeout to the other side of the user layers fails
them.

## Cost

**The order is not configurable.** An application that genuinely needs its own
layer between the session and CSRF cannot express that. `.layer()` installs
user layers at one position — innermost, wrapping the router directly, where
the request has already been limited, timed and authenticated — and that is the
only position on offer. Anything else means dropping to axum and assembling the
router by hand, which is supported but is no longer this pipeline.

**One more place to keep true.** The table is prose beside code, and prose can
rot. It is mitigated by the tests and by the table living in the same file as
the function it describes, not by anything structural. A change to `apply` that
does not update the table is a defect, and only a reviewer will catch it.
