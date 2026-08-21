#![forbid(unsafe_code)]

//! The application binary. Everything lives in the library crate so the
//! integration tests under `tests/` can reach it.

#[arcature::main]
async fn main() -> std::io::Result<()> {
    __RUST_NAME__::run(std::env::args().skip(1)).await
}
