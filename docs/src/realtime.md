# Realtime

WebSocket and Server-Sent Events over axum. One bounded
`tokio::sync::broadcast` channel fans a payload out to every connection the
process is holding.

The wrappers are thin on purpose. There is no Arcature realtime protocol, no
`X-Arcature-*` headers, and no client package to install: a browser talks to
this with `WebSocket` and `EventSource`, the two APIs it already has. The
rejected alternative is the Pusher/Echo shape — a framed protocol carrying
subscribe and unsubscribe messages, plus a JavaScript client to speak it. That
buys channel multiplexing over a single socket, and it costs a second wire
format to version, a package to publish and keep in step with the server, and
a debugging story where the network tab shows you frames to decode instead of
events to read. `axum::extract::ws` and `axum::response::sse` stay first-class
escape hatches; when the wrapper is the wrong shape, drop to the thing it
wraps.

Realtime is app-owned. The framework mounts no route, registers no service and
holds no global. You build a `Broadcast`, a `Registry` and a `ShutdownConfig`
once, put them in your state, and clone `WebSocketEndpoint` / `SseEndpoint`
into handlers.

## Turning it on

`realtime` is in `default`, so `cargo add arcature` already has it. The
manifest entry is the whole of what it pulls:

```toml
realtime = ["dep:tokio", "dep:futures", "dep:bytes"]
```

Three optional dependencies, and on a default build all three are already
present — `tokio` from `macros`, `bytes` and `futures` from `storage-fs`. So
on the default feature set `realtime` adds no crate to the graph, only edges.
On a `default-features = false` build that names `realtime`, it adds those
three.

What it does not pull is the WebSocket implementation. The `axum` dependency
enables `ws` unconditionally, so tungstenite compiles into every build of this
crate whether `realtime` is on or off. Turning the feature off removes
Arcature's wrappers, not the protocol underneath them.

`arcature::realtime` is the module. `Broadcast`, `SseEndpoint` and
`WebSocketEndpoint` are also re-exported at the crate root and from the
prelude.

## Broadcast and publishing

```rust,ignore
use arcature::realtime::{Broadcast, ChannelPayload};

// Capacity is a required argument. There is no default and no `Default` impl.
let broadcast = Broadcast::new(256).expect("capacity is non-zero");

let payload = ChannelPayload::from_bytes(serde_json::to_vec(&update)?);
let delivered: usize = broadcast.publish(payload)?;
```

`Broadcast::new` returns `Option<Self>` and gives `None` for a capacity of
zero. A channel that can retain nothing drops every message, so handing one
back as if it had worked would be worse than saying no. It never panics.

`ChannelPayload` is opaque bytes behind an `Arc<[u8]>`. The channel stores a
value once and clones it per receiver, so the `Arc` makes that clone a
refcount bump rather than a copy of the payload. There is no JSON helper and
no envelope: serialize it yourself, and the bytes on the wire are the bytes
you published.

`publish` has one result that surprises people:

| Situation | `publish` returns |
| --- | --- |
| `n` subscribers | `Ok(n)` |
| **no subscribers** | **`Err(ChannelError::Closed)`** |
| all `Broadcast` handles dropped | `Err(ChannelError::Closed)` |
| buffer full | `Ok(n)` — tokio overwrites the oldest retained message |

An empty channel and a dead channel are the same error, because
`tokio::sync::broadcast::Sender::send` reports failure when the receiver count
is zero and `publish` maps every send failure to `Closed`. Treat `Err(Closed)`
as "nobody was listening", not as "something broke". `tests/realtime.rs` pins
this.

`Broadcast::capacity()` returns the number you passed. Tokio rounds the ring
buffer **up** to the next power of two, and lag is measured against the
rounded size, so a `Broadcast::new(100)` reports 100 and starts lagging
receivers at 128.

`ChannelError` has a third variant, `Full`. Nothing in this crate constructs
it. Both endpoints match on it, and neither arm can be reached.

`broadcast.subscribe()` hands back a `Subscription` whose `recv()` yields the
next payload. `Subscription` also exposes its `pub rx`, the raw
`broadcast::Receiver`, so it composes with `tokio::select!`.
`Broadcast::subscriber_count()` is a separate atomic maintained by
`subscribe()` and `Subscription::drop`, not tokio's own receiver count — a
receiver you conjure by calling `rx.resubscribe()` yourself is invisible to
it.

## Subscribing over SSE

```rust,ignore
use arcature::realtime::{OriginPolicy, Registry, SseEndpoint, SseLimits,
                         ShutdownConfig, VerifiedOrigin};

let sse = SseEndpoint::new(
    broadcast.clone(),
    OriginPolicy::allow_exact(VerifiedOrigin::from_trusted("https://app.example.com")),
    registry.clone(),
    SseLimits::conservative(),
    shutdown.clone(),
);

// In a handler:
sse.clone().handle(headers, channel_id).await
```

Admission is origin, then the connection limit. There is no authorizer on this
path: `SseEndpoint::new` takes one `Broadcast` and every admitted request
subscribes to it. The `channel_id` argument to `handle` is accepted and
ignored. If SSE needs per-channel authorization, it belongs in a layer above
the handler — an SSE request is a plain `GET`, so ordinary middleware works on
it in a way it cannot for an upgrade.

The first event on the stream is a `retry:` preamble carrying
`SseLimits::retry_ms`, which tells `EventSource` how long to wait before
reconnecting. After that, each published payload becomes one `data:` event.

The payload is converted with `str::from_utf8(..).unwrap_or("")`. A payload
that is not valid UTF-8 does not error, is not logged, and is not dropped: it
becomes an event with an empty data field, which browsers ignore. Publish text
to a channel with SSE subscribers on it.

Keep-alive is a `:keep-alive` comment every `keep_alive_interval`. It exists so
proxies and load balancers with an idle-read timeout do not cut a quiet stream.

## Subscribing over WebSocket

```rust,ignore
use arcature::realtime::{Authorizer, Broadcast, WebSocketEndpoint, WsLimits};

#[derive(Clone)]
struct DocumentAccess { /* ... */ }

impl Authorizer for DocumentAccess {
    fn authorize(
        &self,
        headers: &HeaderMap,
        channel_id: &str,
    ) -> impl Future<Output = Option<Broadcast>> + Send {
        let this = self.clone();
        let channel_id = channel_id.to_owned();
        async move { this.channel_if_permitted(&channel_id).await }
    }
}

let ws = WebSocketEndpoint::new(
    DocumentAccess::new(state),
    origin_policy,
    registry.clone(),
    WsLimits::conservative(),
    shutdown.clone(),
);
```

`Authorizer` returns `Option<Broadcast>`, not `bool`. That is the design
decision in this module. Authorizing a connection and choosing its channel are
the same call, so there is no code path where a request names a channel and
gets subscribed to it without something having handed that `Broadcast` back. A
channel name never implicitly authorizes, because a name is not what selects
the channel. The rejected alternative — a predicate plus a name-to-channel
lookup — puts the two halves in separate places, and then the lookup has to be
trusted to have been guarded.

`AllowAll` implements the trait by admitting everything to one fixed channel.
It is there for tests.

Admission is origin, then the authorizer, then the connection limit, and all
three run **before** the upgrade. A refused request gets a clean HTTP status on
an unupgraded connection rather than a socket that opens and then closes.

Once upgraded, the loop:

- sends each published payload as a **binary** frame, not text
- answers nothing the client sends. `Close` ends the loop and `Pong` refreshes
  the liveness clock. Every other inbound frame is matched and discarded. The
  server parses no client payload, so there is no request surface here.
- pings every `heartbeat_interval` and closes when the last pong is older than
  `heartbeat_timeout`

The pong check is `elapsed > heartbeat_timeout`, evaluated only when the
heartbeat fires, and the clock starts at connection open rather than at the
first pong. With the defaults that means a client that never pongs is closed at
the 60-second tick: at 20s and 40s the elapsed time is not yet *greater than*
40s, so both of those send another ping.

Note the asymmetry with SSE. The same `publish` call reaches a WebSocket client
as opaque binary and an SSE client as a UTF-8 `data:` line. A payload that is
valid UTF-8 arrives intact on both.

## The connection registry and its cap

`Registry` is an `Arc`-backed counter plus a drain signal. It is the only thing
that knows how many realtime connections a process is holding, and it is shared
between the WebSocket and SSE endpoints.

The cap is not stored on the `Registry`. It is passed to `Registry::acquire` by
the caller, and both endpoints pass `ShutdownConfig::max_connections()`. So the
number lives on the shutdown config, and two endpoints built with two different
`ShutdownConfig`s enforce two different numbers against one shared count.

`acquire` refuses when `current >= max` — at the cap, not past it. A cap of 100
admits 100 connections. It returns a `ConnectionGuard`, and dropping the guard
decrements the count and wakes any drain waiter.

**A WebSocket connection holds its guard; an SSE stream does not.**
`run_connection` takes the guard as an argument and holds it for the life of the
socket. `SseEndpoint::handle` acquires a guard to make the limit check, but
never moves it into the stream it returns, so the guard drops when the response
is built — before the first byte is written. The source comment at that line
says the guard is held for the lifetime of the stream; the code does not do
that. Two consequences: the cap bounds concurrent SSE *admissions* rather than
concurrent SSE streams, and the drain below cannot see SSE streams at all.

## What a client sees when it lags

A subscriber that falls further behind than the (rounded-up) channel capacity
has messages overwritten under it. Tokio reports this once, as
`RecvError::Lagged(n)`, and then resumes the receiver at the oldest message
still retained — the connection is not closed and the subscription is not
broken.

What reaches the client:

| Transport | On lag | Missed-message count |
| --- | --- | --- |
| SSE | a `:lagged` comment line | not sent |
| WebSocket | nothing at all — the loop continues | not sent |

`Subscription::recv` maps `RecvError::Lagged(n)` onto a unit
`ChannelError::Lagged`, so the count of dropped messages is discarded before
either endpoint could report it. An SSE comment is invisible to `EventSource`:
no listener fires for it, and a browser client cannot observe the lag through
that API. A WebSocket client gets no signal whatsoever.

The practical consequence is that a lagging client silently has a hole in its
stream. If the messages matter, they need a durable source the client can
reconcile against — the pattern `notifications-broadcast` uses, where the live
push is an optimisation over an inbox row that is still there.

## Graceful drain

```rust,ignore
use std::time::Duration;
use arcature::realtime::{self, ShutdownConfig};

let shutdown = ShutdownConfig::new(1_000); // max_connections

// On shutdown:
realtime::drain(&registry, &shutdown, Duration::from_secs(10)).await?;
```

`realtime::drain` is two steps: `shutdown.begin_drain()`, then
`registry.drain(bound)`. `begin_drain` is idempotent — it flips an `AtomicBool`
with `swap` and only notifies waiters on the transition, so calling it twice
from two signal paths is safe. `ShutdownConfig` carries both the connection cap
and the drain flag; it is `Arc`-backed and cheap to clone into every endpoint.

`registry.drain(bound)` returns `Ok(())` immediately if the live count is
already zero. Otherwise it waits for zero within `bound` and then re-reads the
count, returning `Err(RealtimeError::Shutdown { remaining })` if any connections
are still live. The waiter is woken by each guard drop and also re-checks every
100ms, so a missed notification costs latency rather than a hang.

How each transport responds:

| Transport | Response to `begin_drain` |
| --- | --- |
| WebSocket | `drain_notified()` is a `select!` arm, so the loop sends a `Close` frame and exits at its first pass |
| SSE | the flag is read once per event, before the wait for the next payload |

A new connection during a drain is **not** refused. Neither `handle` checks
`is_draining()` during admission, so an arriving WebSocket is upgraded and then
immediately closed, and an arriving SSE request gets its `retry:` preamble
followed by end-of-stream — which, with the default 3-second retry hint, means
`EventSource` reconnects into the same outcome three seconds later. Stop
accepting realtime requests at the router or the load balancer if that
reconnect loop matters.

## Defaults

Everything the two `conservative()` constructors set, and the two values that
have no default at all.

| Setting | Default | Source |
| --- | --- | --- |
| Channel capacity | **none** — required argument to `Broadcast::new`; `None` for 0 | `Broadcast::new` |
| Connection limit | **none** — required argument to `ShutdownConfig::new` | `ShutdownConfig::new` |
| Origin policy | `DenyAll` (the `Default` impl) | `OriginPolicy` |
| SSE retry hint | 3000 ms | `SseLimits::conservative()` |
| SSE keep-alive interval | 15 s, sent as a `:keep-alive` comment | `SseLimits::conservative()` |
| WS max message size | 65536 bytes (64 KiB) | `WsLimits::conservative()` |
| WS max frame size | 65536 bytes (64 KiB) | `WsLimits::conservative()` |
| WS heartbeat interval | 20 s | `WsLimits::conservative()` |
| WS pong timeout | 40 s (strict `>`, checked on heartbeat ticks) | `WsLimits::conservative()` |

`SseLimits`, `WsLimits` and `ShutdownConfig` have no `Default` impl. Ask for
`conservative()` or fill in the fields; the numbers above are not applied to
anything you did not construct. `Registry::default()` exists and is an empty
registry — it carries no cap of its own.

Statuses a refused connection gets, from `admission_status`:

| Refusal | Status |
| --- | --- |
| Origin denied | `403 Forbidden` |
| Authorizer returned `None` (WebSocket only) | `403 Forbidden` |
| At or above the connection limit | `503 Service Unavailable` |

The mapping is an explicit function rather than an `IntoResponse` impl, so a
refusal returns a status and no body. A denied origin and a failed authorization
are deliberately the same status: the difference between "you are not from here"
and "you may not have this channel" is information the caller does not need.

`OriginPolicy` defaults to `DenyAll`, and a policy with an allow-list denies a
request that carries no `Origin` header at all.
`VerifiedOrigin::from_header` rejects a non-ASCII value; `from_trusted`
validates nothing, which is the point of the name. Matching is exact string
equality — an origin is public, so constant-time comparison is not wanted here.

## Limits

**Fan-out reaches one process, and there is no switch.** `Broadcast` wraps a
`tokio::sync::broadcast` channel, which is a channel between tasks inside one
process. A message published on instance A reaches only the WebSocket and SSE
subscribers connected to instance A. Nothing errors and nothing warns:
subscribers on instance B never see it, because the channel delivered correctly
to everyone it can see and it cannot see the other process. With two instances
and clients spread evenly, roughly half of every broadcast is missing from a
given client's view. This is the one limit here with no configuration switch —
sessions share through `session-store-db` and rate limiting shares through
`RateLimit::redis`, and realtime has no equivalent. [Deployment](deployment.md)
lists the three honest ways to live with it: run one instance, pin realtime
upgrades to one instance, or write the bridge by hand. A Redis pub/sub bridge is
the obvious general answer and is deliberately not written, because its delivery
semantics, ordering and back-pressure would have to be decided rather than
inherited.

**Publishing from a job worker reaches nobody.** It is the same limit with a
sharper edge: a worker is a different process, and it holds none of the web
process's sockets.

**An idle SSE stream does not notice a drain.** The `is_draining()` check runs
once per event, immediately before the wait for the next payload. A stream
parked in that wait when the drain begins stays parked. It ends after delivering
one more payload, or when the last `Broadcast` handle drops and the channel
closes. The keep-alive tick does not help: axum emits the keep-alive comment
when the inner stream is pending and does not re-enter it, so from the client's
side the connection keeps looking healthy while the server is trying to shut
down. Combined with the SSE guard release described above, that means
`realtime::drain` can return `Ok(())` with SSE streams still open — the registry
count it waits on never included them.

**The WebSocket path copies each payload per subscriber.** The `Arc<[u8]>` keeps
the channel's own fan-out cheap, but the send does
`Bytes::from(payload.as_bytes().to_vec())`, which allocates and copies once per
connection per message.

**Three public variants are unreachable from inside this module.**
`ChannelError::Full`, `RealtimeError::Protocol { hint }` (and every
`ProtocolHint`) and `RealtimeError::Channel(_)` are declared, matched or mapped
to a status, and never constructed by any code in the crate. They are usable by
an application that constructs them; do not write a handler that waits for the
framework to hand one over.

## What this module does not do

No message history and no replay. A subscriber receives what is published after
it subscribes, and a lagging subscriber's missed messages are gone. If a client
needs to catch up after a reconnect, that comes from your database, not from
here — there is no `Last-Event-ID` handling and no `id:` field on the events.

No presence, no rooms, no channel registry. There is no list of who is
connected, no join or leave notification, and no server-side directory mapping
names to channels: a `Broadcast` is a value your application holds, and the
`Authorizer` is the only thing that turns a name into one.

No client-to-server messages. The WebSocket loop discards every inbound frame
except `Close` and `Pong`. This is one-directional fan-out, and a client that
needs to tell the server something should post to a route.

No routes and no automatic wiring. The framework mounts nothing at `/ws` or
`/events`, registers no service, and reads no configuration. Every value in this
chapter is one your application constructs and stores.

No cross-process bridge, as above. No ordering guarantee across channels —
ordering holds within one `Broadcast`, which is all a single ring buffer can
promise. No backpressure on the publisher: `publish` never blocks and never
fails because a subscriber is slow; the slow subscriber lags instead.
