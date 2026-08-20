//! The request payload for updating a user.

use arcature::prelude::*;
use serde::Deserialize;

#[request]
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateUserRequest {
    #[validate(length(min = 1, max = 255))]
    pub name: Option<String>,
    #[validate(email)]
    pub email: Option<String>,
}
