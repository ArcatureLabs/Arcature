use arcature::prelude::*;
use crate::pages::home::HomePage;

pub async fn index() -> Result<Response> {
    Ok(text(StatusCode::OK, "Hello from __PROJECT_NAME__!"))
}
