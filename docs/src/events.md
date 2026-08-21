# Events

In-process typed event dispatch. Not a message bus, not durable, not
cross-process: if the process dies mid-dispatch, the remaining listeners do
not run. For work that must survive a restart, use [Jobs](jobs.md) — a
listener that enqueues a job is the usual bridge.

## Declaring an event

```rust,ignore
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, arcature::Event)]
pub struct UserRegistered {
    pub user_id: u64,
    pub email: String,
}
```

The derive generates `impl DxComponent` with `NAME = "UserRegistered"` and the
empty `impl Event`. `Serialize` and `Deserialize` are yours to add: dispatch
erases the type through `serde_json::Value`, serializing once and
deserializing per listener.

That is a deliberate choice over `TypeId` plus `Any`. The dispatch key is a
`&'static str` name, which means the same mechanism describes itself to
tooling and no downcast can silently miss. It costs one round-trip through
JSON per listener.

## Listeners

```rust,ignore
use arcature::events::{DispatchError, Dispatcher};

let dispatcher = Dispatcher::new()
    .register(|event: UserRegistered| async move {
        println!("welcome {}", event.email);
        Ok(())
    });
```

`register` consumes and returns the dispatcher, so registration chains.
Several listeners may share an event type; they run in registration order.

`#[listener(UserRegistered)]` marks a free function as a listener and emits a
`LISTENER_BINDING` static beside it for inspection:

```rust,ignore
#[arcature::listener(UserRegistered)]
pub async fn send_welcome_email(
    event: UserRegistered,
) -> Result<(), arcature::events::DispatchError> {
    let _ = event;
    Ok(())
}

assert_eq!(LISTENER_BINDING.event, "UserRegistered");
assert_eq!(LISTENER_BINDING.listener, "send_welcome_email");
```

The function is emitted unchanged and stays directly callable. The macro does
not register it — you still pass it to `Dispatcher::register`. The binding
const is metadata for the Unified Application Graph, not a registry that
wires itself.

## Dispatching

```rust,ignore
dispatcher
    .dispatch(&UserRegistered { user_id: 1, email: "a@b.com".into() })
    .await?;
```

Listeners run sequentially in registration order. **A listener failure does
not stop the others**: the error is logged, every listener still runs, and
`dispatch` returns the first error afterwards. Dispatching an event with no
listeners is a no-op that returns `Ok(())`.

Sequential, not concurrent, and in-process, so a slow listener delays the
request that dispatched the event. Listeners should be short; anything with
latency belongs in a job.

## Errors

`DispatchError` has three variants. `Serialize(String)` carries the serde
message. `Listener(String)` carries whatever the listener chose to expose.
`Deserialize` carries **no message at all**, deliberately: a serde
deserialization error can echo the payload it choked on, and an event payload
is exactly the kind of thing that should not end up in a log line.

## Testing

`Dispatcher::recording()` records dispatched event names:

```rust,ignore
let dispatcher = Dispatcher::recording();
dispatcher.dispatch(&UserRegistered { user_id: 1, email: "a@b.com".into() }).await?;
assert!(dispatcher.was_dispatched("UserRegistered"));
```

`dispatched_events()` lists them and `listener_count(name)` reports how many
listeners an event has. In a non-recording dispatcher `was_dispatched` always
returns `false`, so recording is opt-in rather than something production pays
for.
