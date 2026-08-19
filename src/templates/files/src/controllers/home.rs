use arcature::prelude::*;

pub async fn index() -> Result<Response> {
    Ok(text(StatusCode::OK, "Hello from __PROJECT_NAME__!"))
}
