#![forbid(unsafe_code)]

pub mod app;
pub mod routes;

use arcature::prelude::*;

pub async fn run() -> std::io::Result<()> {
    Application::new()
        .routes(routes::routes())
        .build()
        .run()
        .await
        .map_err(|e| {
            eprintln!("{e}");
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })
}
