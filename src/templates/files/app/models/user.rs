//! The `User` SeaORM model.
//! Add fields below; mark the primary key with `#[sea_orm(primary_key)]`.

use arcature::prelude::*;

#[model(table = "users")]
pub struct User {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub name: String,
    pub email: String,
}
