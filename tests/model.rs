//! `#[model]` has to *compile*.
//!
//! The macro's unit tests inspect the token stream, which cannot tell whether
//! SeaORM accepts the result -- and SeaORM is the hard part: `DeriveEntityModel`
//! insists on a struct named `Model` and generates a family of sibling types
//! around it. This file is the check that the module-and-re-export expansion
//! actually satisfies it.

#![cfg(all(feature = "macros", feature = "database"))]

use arcature::prelude::*;

/// A row of `users`.
#[model(table = "users")]
pub struct User {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub email: String,
    pub display_name: Option<String>,
}

#[model(table = "blog_posts")]
pub struct BlogPost {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub title: String,
}

#[test]
fn the_model_struct_is_a_seaorm_model() {
    fn assert_model<M: arcature::database::sea_orm::ModelTrait>() {}
    assert_model::<User>();
    assert_model::<BlogPost>();
}

#[test]
fn the_companion_entity_is_a_seaorm_entity() {
    use arcature::database::sea_orm::EntityTrait;
    fn assert_entity<E: EntityTrait>() {}
    assert_entity::<UserEntity>();
    assert_entity::<BlogPostEntity>();
}

#[test]
fn the_entity_is_bound_to_the_declared_table() {
    use arcature::database::sea_orm::EntityName;
    assert_eq!(UserEntity.table_name(), "users");
    assert_eq!(BlogPostEntity.table_name(), "blog_posts");
}

#[test]
fn the_row_type_carries_the_query_facade() {
    // `Model::query` takes a `&Db`, which needs a live connection to build,
    // so this checks the seam by name rather than by calling it: if `User`
    // did not implement `Model`, naming the method would not compile.
    fn assert_queryable<M: Model>() {}
    assert_queryable::<User>();
    assert_queryable::<BlogPost>();
}

#[test]
fn the_active_model_and_column_types_are_reachable() {
    use arcature::database::sea_orm::{ActiveValue, IdenStatic, Iterable};

    let active = UserActiveModel {
        id: ActiveValue::NotSet,
        email: ActiveValue::Set("a@example.com".to_owned()),
        display_name: ActiveValue::Set(None),
    };
    assert!(matches!(active.email, ActiveValue::Set(_)));

    let columns: Vec<String> = UserColumn::iter().map(|c| c.as_str().to_owned()).collect();
    assert_eq!(columns, ["id", "email", "display_name"]);
}

#[test]
fn a_model_serializes_for_transport() {
    let user = User {
        id: 7,
        email: "a@example.com".to_owned(),
        display_name: None,
    };
    let json = serde_json::to_value(&user).expect("a model serializes");
    assert_eq!(json["id"], 7);
    assert_eq!(json["email"], "a@example.com");
}
