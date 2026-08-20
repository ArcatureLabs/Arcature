//! The `User` resource: the DTO returned to the frontend.
//! Decouples the model from the JSON the client sees.

use crate::app::models::User;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct UserResource {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl From<User> for UserResource {
    fn from(u: User) -> Self {
        Self { id: u.id, name: u.name, email: u.email }
    }
}
