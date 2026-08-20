//! The authorization policy for the `User` model.

use arcature::prelude::*;
use crate::app::models::User;

pub struct UserPolicy;

impl Policy<User> for UserPolicy {
    type User = User;

    fn check(user: &Self::User, action: &str, resource: &User) -> bool {
        match action {
            "view" | "update" | "delete" => user.id == resource.id,
            _ => false,
        }
    }
}
