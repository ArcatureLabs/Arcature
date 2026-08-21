//! Smoke tests: the route table is wired.
//!
//! These run without a database, a Redis, or a bound port. They catch the
//! wiring regressions that would otherwise only show up on the first request
//! of a manual run.

use __RUST_NAME__::routes::{APP_ROUTES, app_routes};

#[test]
fn the_home_route_resolves_by_name() {
    let routes = app_routes();
    assert_eq!(routes.url_for("home", &[]).unwrap(), "/");
}

#[test]
fn every_route_name_is_unique() {
    let mut names: Vec<&str> = APP_ROUTES
        .iter()
        .map(|route| route.name)
        .filter(|name| !name.is_empty())
        .collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "duplicate route name in {names:?}");
}
