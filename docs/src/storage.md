# Storage

Object and file storage over OpenDAL, behind a named-disk registry.

`Storage` is a value, not a namespace. `Storage::disk("s3")` is an instance
method on a handle you got from state — there is no static `Storage::disk`.

## Configuring

Single backend:

```rust,ignore
use arcature::storage::{Storage, StorageConfig, S3Config};

let storage = Storage::connect(StorageConfig::fs("storage/app")?).await?;
```

`Storage::connect` registers the backend as a disk named `"default"`.

Several disks:

```rust,ignore
let storage = Storage::builder()
    .disk("local", StorageConfig::fs("storage/app")?)
    .disk("s3", StorageConfig::s3(
        S3Config::new("acme-uploads")?
            .region("eu-west-1")
            .access_key_id(std::env::var("AWS_ACCESS_KEY_ID")?)
            .secret_access_key(std::env::var("AWS_SECRET_ACCESS_KEY")?),
    ))
    .default_disk("local")
    .connect()
    .await?;
```

`S3Config` redacts the access key id and the secret in its `Debug` impl.

`Application::storage(config)` wires the single-backend path at startup.

## Disks

| Call | Behaviour |
| --- | --- |
| `storage.disk("s3")` | the named disk; **panics** if it was never registered |
| `storage.try_disk("s3")` | `Option<Disk>` |
| `storage.default_disk()` | the disk named by `default_disk`, or `"default"` |
| `storage.disk_names()` | what is registered |

`disk` panics deliberately: a disk name is a deployment constant, and a typo
should stop the process at the first use rather than return an error every
handler forgets to check. `try_disk` is there when the name really is dynamic.

`Disk` is cheap to clone — the OpenDAL `Operator` inside it is `Arc`-backed.

## Paths

Every data-path method takes a `&StoragePath`, not a `&str`. Constructing one
is where the validation happens:

```rust,ignore
use arcature::storage::StoragePath;

let path = StoragePath::new("avatars/1.png")?;
storage.disk("s3").put(&path, &bytes).await?;
```

Rejected: empty keys, absolute paths (`/etc/passwd`), any `..` segment,
backslashes, ASCII control characters, and empty segments (`a//b`).

Allowed: trailing slashes, because they are meaningful as list prefixes;
Unicode of all kinds; dots inside a segment.

The check runs before any storage work does, so a traversal attempt fails at
the type boundary rather than at the backend.

## Operations

All on `Disk`:

```rust,ignore
let disk = storage.disk("local");

disk.put(&path, &bytes).await?;
let bytes: Bytes = disk.get(&path).await?;
let present: bool = disk.exists(&path).await?;
let meta = disk.stat(&path).await?;
let entries = disk.list(&prefix).await?;
disk.copy(&from, &to).await?;
disk.rename(&from, &to).await?;
disk.delete(&path).await?;
```

For large objects, `disk.reader(&path)` and `disk.writer(&path)` return the
OpenDAL `Reader` and `Writer` and stream rather than buffering.

`Disk::from_operator(operator)` is the escape hatch when you want to
configure the OpenDAL operator yourself; `disk.operator()` borrows it back.

## Public files

`arc storage:link` links `storage/app/public` into `public/storage`, the
Laravel convention, so files written to the local disk under `public/` are
served as static assets.

## What this module does not own

No object-storage protocol implementation, no S3 signing, no AWS credential
machinery, no multipart engine, no TLS. OpenDAL owns the protocol layer, the
certified rustls plus aws-lc-rs stack owns TLS, Tokio owns the runtime. The
crates are re-exported as `arcature::storage::opendal` and
`arcature::storage::bytes`.
