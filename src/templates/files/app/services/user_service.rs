//! The user service: business logic over the `User` model.
//! Services take a `&Db` and return domain values, never HTTP responses.

use arcature::prelude::*;
use crate::app::models::User;

pub async fn list(db: &Db) -> Result<Vec<User>> {
    User::query(db).latest().all().await
}
