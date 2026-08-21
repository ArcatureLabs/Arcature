# 0003 — Exactly one TCP port, in development as well as production

## Decision

An Arcature application listens on one TCP port. In production that is the
application server. In development it is still the application server: Vite
runs in `middlewareMode`, binds no TCP port at all, and listens on an IPC
endpoint — a Unix domain socket, or a Windows named pipe — that the Rust
process forwards to. The browser sees one origin for the page, the assets and
the HMR WebSocket.

There is deliberately no "just open port 5173" fallback. If the IPC endpoint is
not there, the dev proxy is inactive and Vite requests reach the application
router, which does not serve them. The implementation is `src/dev_proxy/`, and
the model is AdonisJS.

## Context

The conventional setup runs the frontend dev server on its own port and the
backend on another. It works, and it makes development a different system from
production in the ways that matter most:

- **Two origins.** Cookies are per-origin. `Secure`, `SameSite` and `__Host-`
  behave differently across them, so the CSRF and session configuration that
  works in development is not the configuration that runs in production, and
  the difference surfaces at deploy time.
- **CORS in development only.** Preflights, credentialed requests and header
  allow-lists get configured to make the split work, and then have to be
  remembered as development-only when the split disappears.
- **A proxy table.** Either Vite proxies `/api` to the backend or the backend
  proxies assets to Vite, and either way there is a second routing table that
  exists nowhere in production and drifts from the real one.

One port removes all three at once, and it removes them by making development
resemble production rather than by adding configuration to paper over the gap.

Vite supports this directly. In `middlewareMode` it does not create a server;
it hands back a middleware. Attaching that middleware to an IPC listener and
forwarding to it from Rust is the entire mechanism. `ARCATURE_VITE_IPC` carries
the endpoint path, read once at pipeline assembly (`dev_proxy::config`), never
per request. `dev_proxy::vite::is_vite_request` decides what to forward — `/@…`,
`/src/…`, `/node_modules/.vite/…`, and the `vite-hmr` WebSocket upgrade — as a
pure function of path and headers, so the application never sees a Vite request
and Vite never sees an application route.

## Cost

**A transport most people have not debugged.** When a TCP dev server misbehaves
you can curl it. An IPC endpoint needs different tools, and on Windows it is a
named pipe, which needs different tools again. The failure modes are honest —
connect-time `NotFound` falls through to the application 404, a mid-request
failure is a redacted 502 — but they are less familiar than a connection
refused on 5173.

**Vite must be started by the tooling.** A developer cannot run `npx vite` in
another terminal and have it work, because a TCP Vite is not what the proxy
forwards to. The IPC path is process-private and per-invocation.

The supervisor behind `arc dev` is implemented. It mints the two IPC
endpoints, starts Vite against one and the application against the other,
binds the single TCP port itself, and rebuilds only the application child so
the listener and the HMR socket survive a save. It also publishes its address
in `.arcature/dev.addr`, which is how `arc typegen` run from a second terminal
reads the graph from the running process instead of building anything.
