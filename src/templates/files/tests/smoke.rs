//! Smoke test: the route table compiles and the home route is registered.

//! A minimal end-to-end check that `routes()` builds and the `home` named
//! route resolves to `/`. Catches wiring regressions in the bootstrap layer.

use __RUST_NAME__::routes;

#[test]
fn home_route_is_registered() {
    let routes = routes::routes();
    assert_eq!(routes.url_for("home", &[]).unwrap(), "/");
}
