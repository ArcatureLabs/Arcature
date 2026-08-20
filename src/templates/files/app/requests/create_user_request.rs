//! The request payload for creating a user.

use arcature::prelude::*;
use serde::Deserialize;

#[request]
#[derive(Debug, Clone, Deserialize)]
pub struct CreateUserRequest {
    #[validate(length(min = 1, max = 255))]
    pub name: String,
    #[validate(email)]
    pub email: String,
}
