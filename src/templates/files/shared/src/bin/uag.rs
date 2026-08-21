#![forbid(unsafe_code)]

//! Print the Unified Application Graph as JSON on stdout.
//!
//! `arc typegen`, `arc routes` and `arc build` run outside this process, so
//! they cannot call [`app::graph`](__RUST_NAME__::app::graph) directly. When a
//! dev server is running they read `/_arcature/uag.json` from it; when one is
//! not -- in CI, chiefly -- they build and run this binary instead.
//!
//! It is behind `required-features = ["uag"]` in `Cargo.toml`, which is what
//! keeps `cargo run` and `cargo build --features dev` from linking a second
//! binary on every dev-loop iteration. `dev` turns on the framework's `uag`
//! feature so the endpoint exists; this crate's own `uag` feature is what
//! adds the target, and only `arc typegen`'s fallback path asks for it.
//!
//! Nothing here is reachable from the server: the graph is derived from
//! `&'static` metadata, so this prints the same bytes the endpoint serves
//! without opening a database connection or reading `.env`.

fn main() -> std::io::Result<()> {
    use std::io::Write as _;

    let artifact = arcature::uag::build(
        &__RUST_NAME__::app::graph(),
        &__RUST_NAME__::app::page_contracts(),
    );
    // Written as bytes rather than through `println!` so the output is
    // byte-identical to what the endpoint serves: the artifact is what the
    // caller diffs in CI, and a trailing newline is the only difference a
    // formatter would introduce.
    let json = artifact.to_json().map_err(std::io::Error::other)?;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    stdout.write_all(&json)?;
    stdout.write_all(b"
")?;
    stdout.flush()
}
