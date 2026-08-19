#![forbid(unsafe_code)]

pub mod controllers;
pub mod pages;
pub mod routes;

use arcature::prelude::*;

pub fn run() -> impl std::future::Future<Output = std::io::Result<()>> + Send + 'static {
    async {
        Application::new()
            .routes(routes::routes())
            .run()
            .await
            .map_err(|e| {
                eprintln!("{e}");
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
            })
    }
}
