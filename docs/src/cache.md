# Cache

One multiplexed Redis/Valkey connection per `Cache`. Not a pool — redis-rs's
`MultiplexedConnection` is cheap to clone, thread-safe, and multiplexes
commands over a single socket, so a second pool would buy nothing.

`Cache` is a value with methods. There is no `Cache::remember(&cache, ..)`
static form; the handle comes from state and you call it directly.

## Connecting

```rust,ignore
use std::time::Duration;
use arcature::cache::{Cache, CacheConfig, Namespace};

let config = CacheConfig::new(&std::env::var("REDIS_URL")?)?
    .response_timeout(Duration::from_secs(2))
    .namespace(Namespace::new("acme")?);

let cache = Cache::connect(config).await?;
```

`Application::cache(config)` does this at startup. `cache.ping()` checks
liveness; `cache.close()` shuts it down.

`CacheConfig` never logs the password or the full URL — its `Debug` impl is
written by hand to redact them.

`max_payload_size(Some(bytes))` caps what a single value may store.

## Namespaces

`Namespace::new("acme")` prefixes every key as `acme:key`. It rejects an
empty prefix, a prefix ending in `:`, and control characters.

`Namespace::none()` is the explicit opt-out. It is a distinct value rather
than `None` at the type level, so an unprefixed cache is a decision someone
made rather than a field nobody filled in.

## Operations

| Method | Does |
| --- | --- |
| `get::<T>(key)` | JSON value, `None` when absent |
| `set(key, &value)` | store JSON, no expiry |
| `put(key, &value, ttl)` | store JSON with a TTL |
| `get_bytes(key)` / `set_bytes(key, bytes)` | raw bytes |
| `set_bytes_with_ttl(key, bytes, ttl)` | raw bytes with a TTL |
| `forget(key)` | delete; returns how many keys went |
| `exists(key)` | presence |
| `incr(key, delta)` / `decr(key, delta)` | atomic counters |
| `expire(key, ttl)` / `ttl(key)` | expiry control |
| `set_if_absent(key, bytes)` | atomic compare-and-set |
| `set_if_absent_with_ttl(key, bytes, ttl)` | the same, with a TTL |

## Cache-aside

```rust,ignore
use std::time::Duration;

let user = cache
    .remember("user:42", Duration::from_secs(300), || async {
        load_user(42).await
    })
    .await?;
```

`remember` reads the key, and on a miss runs the loader, stores the result
with the TTL, and returns it.

The loader's error type must implement `Into<CacheError>`. That is the part
worth checking before you write the closure: a loader returning
`sqlx::Error` needs a conversion, or the closure needs to map it.

A miss is not an error. A **backend failure is**, and it does not run the
loader. Arcature does not decide fail-open semantics on your behalf: if you
want the loader to run when Redis is unreachable, handle `CacheError::Backend`
explicitly before calling `remember`. Silently degrading to the database under
load is how a cache outage becomes a database outage.

## What this module does not own

No Redis protocol reimplementation, no cache server, no connection pool, no
TLS stack, no distributed-lock subsystem, no client-side caching layer. The
redis-rs crate is re-exported as `arcature::cache::redis` when you need it
directly.
