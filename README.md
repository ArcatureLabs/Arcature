# Arcature

An opinionated full-stack Rust web framework. One package, batteries included.

`cargo add arcature` is enough for the canonical generated application: HTTP
routing, native Inertia.js, database, auth, validation, cache, storage, mail,
jobs, events, realtime, observability, and the `arc` CLI.

## Quick start

Generate a new application:

```sh
cargo install arcature --features cli
arc new my-app
cd my-app
cargo run
```

Or add Arcature to an existing project:

```sh
cargo add arcature
```

Then write your first route:

```rust
use arcature::prelude::*;

#[arcature::main]
async fn main() -> Result<()> {
    Application::new()
        .routes(Routes::new([Route::get("/", index).name("home")]))
        .run()
        .await
}

async fn index() -> Result<Response> {
    Ok(text(StatusCode::OK, "hello"))
}
```

## What Arcature is

Arcature integrates proven wheels (Axum, Tower, Tokio, SeaORM, SQLx, Inertia.js,
OpenDAL, lettre, tracing) and owns the developer experience: the application
lifecycle, conventions, integration, and a coherent vocabulary. The raw Axum,
Tower, SeaORM, and SQLx escape hatches stay available for when the framework's
opinions run out.

One package. One release unit. One version. Features exist only to reduce
compile surface, never to turn the framework into a self-assembly kit. The
default feature set is batteries-included: it compiles the canonical generated
application with no extra flags.

## Architecture

| Subsystem | Dependency | Feature |
|---|---|---|
| HTTP routing | Axum 0.8, Tower 0.5 | always-on |
| Async runtime | Tokio 1.53 | `macros` |
| Native Inertia.js v3 | Inertia.js protocol | `inertia` |
| Database | SeaORM 2.0, SQLx 0.9 (one PgPool) | `database` |
| Auth | argon2, tower-sessions, secrecy | `auth` |
| Validation | validator 0.21 | `validation` |
| Cache | Redis/Valkey (redis 1.5) | `cache` |
| Storage | OpenDAL 0.58 (fs, S3) | `storage-fs`, `storage-s3` |
| Mail | lettre 0.11 (rustls) | `mail` |
| Jobs | PostgreSQL SKIP LOCKED queue | `jobs` |
| Events | In-process typed dispatch | `events` |
| Realtime | WebSocket + SSE over axum | `realtime` |
| Observability | tracing, request IDs | `observe` |
| Static pages | Static file serving | `pages` |
| CLI | `arc new`, `arc version` | `cli` |

### Versioning

Arcature uses YBF (Year.Breaking.Fix): `YEAR.BREAKING.FIX`.

- The `YEAR` increments with the calendar year.
- The `BREAKING` generation increments on a breaking change.
- The `FIX` increments on a compatible fix.

Current version: `2026.3.0`. MSRV: `1.97.1`. License: `Apache-2.0`.

## Developer experience

### Models

```rust
use arcature::prelude::*;

#[model(table = "users")]
pub struct User {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub email: String,
    pub name: String,
}

// Query facade: User::query(&db).where_eq(...).latest().paginate(20)
let users = User::query(&db).all().await?;
```

### Requests with validation

```rust
#[request]
pub struct StoreUserRequest {
    #[validate(required, email)]
    pub email: String,
    #[validate(length(min = 1, max = 120))]
    pub name: String,
}

pub async fn store(input: Validated<StoreUserRequest>) -> Result<Response> {
    let req = input.into_inner();
    // ...
}
```

### Controllers

```rust
#[controller]
impl UsersController {
    pub async fn index(db: Db) -> Result<Response> {
        let users = User::query(&db).all().await?;
        Ok(json(StatusCode::OK, &users))
    }

    pub async fn show(id: ValidatedPath<i32>, db: Db) -> Result<Response> {
        let user = User::find_by_pk(&db, *id).await?;
        Ok(json(StatusCode::OK, &user))
    }
}
```

### Inertia pages

```rust
pub async fn index(db: Db) -> Result<Response> {
    let users = User::query(&db).all().await?;
    inertia!("users/index", { users })
}
```

### Auth

```rust
pub async fn login(
    auth: AuthManager<User>,
    input: Validated<LoginRequest>,
) -> Result<Response> {
    let req = input.into_inner();
    let user = User::find_by_email(&auth.db(), &req.email).await?;
    auth.login(user).await?;
    Ok(redirect("/dashboard"))
}

pub async fn dashboard(current: Current<User>) -> Result<Response> {
    inertia!("dashboard", { current })
}
```

### Jobs

```rust
#[derive(Job, Serialize, Deserialize)]
#[job(attempts = 5)]
pub struct SendWelcomeEmail {
    pub user_id: i64,
}

// Enqueue:
jobs.enqueue(&JobRequest::new(&SendWelcomeEmail::JOB, &payload)?).await?;

// Register the handler:
registry.add(&SendWelcomeEmail::JOB, |job: SendWelcomeEmail| async move {
    // send the email...
    Ok(())
})?;
```

### Events

```rust
#[derive(Event, Serialize, Deserialize)]
pub struct UserRegistered {
    pub user_id: i64,
    pub email: String,
}

let dispatcher = Dispatcher::new()
    .register(|event: UserRegistered| async move {
        // send welcome email...
        Ok(())
    });

dispatcher.dispatch(&UserRegistered { user_id: 1, email: "a@b.com".into() }).await?;
```

### Cache

```rust
let users = Cache::remember(&cache, "users:all", Duration::from_secs(300), || async {
    User::query(&db).all().await
}).await?;
```

### Storage

```rust
Storage::disk("s3").put("avatars/1.png", &bytes).await?;
let data = Storage::disk("local").get("avatars/1.png").await?;
```

### Mail

```rust
let email = Email::new()
    .from("noreply@example.com")?
    .to("user@example.com")?
    .subject("Welcome")?
    .html("<h1>Welcome!</h1>")?;

Mail::to("user@example.com").send(&mailer, email).await?;
```

## Features

The default features are batteries-included:

```toml
[dependencies]
arcature = "2026.3.0"
```

To reduce compile surface, disable default features and opt in:

```toml
[dependencies]
arcature = { version = "2026.3.0", default-features = false, features = ["macros", "inertia", "database"] }
```

## License

Apache-2.0
