//! What it costs to turn props into an Inertia response.
//!
//! Every page a browser sees pays one of exactly two costs. A first visit
//! gets the root HTML document with the page object embedded in a
//! `data-page` attribute, which means the props are serialised and then
//! HTML-escaped. Every visit after that gets the page object as JSON, which
//! skips the document entirely. The gap between the two rows is the price of
//! the document, and it is charged once per full page load rather than once
//! per navigation -- which is the whole argument for the protocol.
//!
//! One iteration is one `Inertia::render` call, so the reported time is
//! seconds per rendered page. Nothing is sent anywhere: the response value
//! is built and dropped.
//!
//! The `serialize-props-only` row is `serde_json::to_value` on the same
//! props, so the serialisation floor can be subtracted from either render.
//!
//! Props come in two sizes because the two costs scale differently: the
//! document overhead is fixed, the serialisation is not, and a benchmark
//! that only measured a three-field page would make the fixed part look
//! like the whole story.

use std::hint::black_box;

use arcature::inertia::Inertia;
use arcature::{InertiaConfig, default_root_document};
use axum::body::Body;
use axum::extract::FromRequestParts as _;
use axum::http::Request;
use axum::http::request::Parts;
use criterion::{Criterion, criterion_group, criterion_main};
use serde::Serialize;

/// How many rows the large-props page carries. A list page's worth.
const ROWS: usize = 100;

/// The props of a small page: what a dashboard header sends.
#[derive(Serialize)]
struct SmallProps {
    message: &'static str,
    app_name: &'static str,
    arcature_version: &'static str,
}

/// One row of the large page.
#[derive(Serialize)]
struct Row {
    id: u64,
    title: String,
    slug: String,
    published: bool,
    excerpt: String,
}

/// The props of an index page: a list, plus the fields around it.
#[derive(Serialize)]
struct LargeProps {
    app_name: &'static str,
    total: usize,
    rows: Vec<Row>,
}

fn small_props() -> SmallProps {
    SmallProps {
        message: "Welcome to the benchmark",
        app_name: "Bench",
        arcature_version: env!("CARGO_PKG_VERSION"),
    }
}

fn large_props() -> LargeProps {
    LargeProps {
        app_name: "Bench",
        total: ROWS,
        rows: (0..ROWS)
            .map(|n| Row {
                id: n as u64,
                title: format!("Post number {n}"),
                slug: format!("post-number-{n}"),
                published: n % 3 != 0,
                excerpt: "A sentence of body copy, long enough to be worth \
                          serialising and short enough to read."
                    .to_owned(),
            })
            .collect(),
    }
}

/// Build the extractor the way the Inertia layer would have.
///
/// The layer inserts the config into request extensions and the extractor
/// parses the request context out of the headers; doing that here, once, is
/// what keeps the measured section to the render itself.
async fn extractor(inertia_visit: bool) -> Inertia {
    let mut builder = Request::builder().uri("/posts");
    if inertia_visit {
        builder = builder.header("x-inertia", "true");
    }
    let request = builder
        .body(Body::empty())
        .expect("the request is well-formed");
    let (mut parts, _body): (Parts, _) = request.into_parts();
    parts.extensions.insert(
        InertiaConfig::new("bench", default_root_document("Bench"))
            .expect("the asset version is non-empty"),
    );
    Inertia::from_request_parts(&mut parts, &())
        .await
        .unwrap_or_else(|_| panic!("the config is in extensions, so extraction succeeds"))
}

fn render(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime is available");

    let first_visit = runtime.block_on(extractor(false));
    let inertia_visit = runtime.block_on(extractor(true));

    let mut group = c.benchmark_group("inertia/render");

    group.bench_function("serialize-props-only/small", |b| {
        b.iter(|| black_box(serde_json::to_value(small_props())));
    });
    group.bench_function("serialize-props-only/large", |b| {
        let props = large_props();
        b.iter(|| black_box(serde_json::to_value(&props)));
    });

    group.bench_function("first-visit-html/small", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(first_visit.render("home", small_props()).await) });
    });
    group.bench_function("inertia-visit-json/small", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(inertia_visit.render("home", small_props()).await) });
    });

    group.bench_function("first-visit-html/large", |b| {
        let props = large_props();
        b.to_async(&runtime)
            .iter(|| async { black_box(first_visit.render("posts/Index", &props).await) });
    });
    group.bench_function("inertia-visit-json/large", |b| {
        let props = large_props();
        b.to_async(&runtime)
            .iter(|| async { black_box(inertia_visit.render("posts/Index", &props).await) });
    });

    group.finish();
}

criterion_group!(benches, render);
criterion_main!(benches);
