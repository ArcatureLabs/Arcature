//! The pages the supervisor serves when the application cannot.
//!
//! A development supervisor that answers nothing while the backend is down
//! has only moved the connection error somewhere the browser reports worse.
//! These three responses are the honest alternatives: the request is being
//! held, the build failed, or the backend died mid-request. Each one says
//! which, and each one gets itself out of the way without a manual reload.
//!
//! # Why they refresh themselves
//!
//! A rebuilt backend cannot reach a page that is not running Vite's HMR
//! client -- and a page showing a compile error is exactly such a page. A
//! `<meta http-equiv="refresh">` is the smallest thing that recovers on its
//! own, needs no script, and works in whatever the developer has open.

use std::time::Duration;

use crate::axum::body::Body;
use crate::axum::http::{Response, StatusCode, header};

/// How long the browser waits before retrying the holding page.
const RETRY_AFTER: u64 = 1;

/// How long a compile-error page waits before checking whether the code
/// compiles now. Longer than [`RETRY_AFTER`]: a failing build is usually
/// followed by more typing, and a page that flashes every second while you
/// read the error is worse than the error.
const ERROR_RETRY_AFTER: u64 = 2;

/// The request outlived the hold deadline and the backend is still building.
///
/// `503` with `Retry-After` is what the status code is for, and it keeps
/// scripted callers (`curl` in a loop, a health check) reporting a real
/// response rather than a refused connection.
#[must_use]
pub fn building(waited: Duration) -> Response<Body> {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        RETRY_AFTER,
        "Building",
        &format!(
            "<p>The application is rebuilding. This request waited \
             {:.1}s and will be answered as soon as the new binary is up.</p>",
            waited.as_secs_f32()
        ),
    )
}

/// The build failed. Show the compiler's own words.
///
/// The terminal already has them, but the terminal is not where the
/// developer is looking when the page goes blank.
#[must_use]
pub fn compile_error(diagnostics: &str) -> Response<Body> {
    page(
        StatusCode::INTERNAL_SERVER_ERROR,
        ERROR_RETRY_AFTER,
        "Compile error",
        &format!(
            "<p>The application did not compile. This page reloads itself \
             when it does.</p><pre>{}</pre>",
            escape(diagnostics)
        ),
    )
}

/// The backend accepted the connection and then went away.
///
/// Distinct from [`building`]: the supervisor thought the application was
/// up, so this is a crash rather than a rebuild, and saying "building" would
/// send the reader looking for a build that is not running.
#[must_use]
pub fn backend_gone(reason: &str) -> Response<Body> {
    page(
        StatusCode::BAD_GATEWAY,
        ERROR_RETRY_AFTER,
        "Backend unavailable",
        &format!(
            "<p>The application process stopped answering.</p><pre>{}</pre>",
            escape(reason)
        ),
    )
}

/// Render one of the three pages.
///
/// Deliberately plain: no external stylesheet, no script, nothing that could
/// itself need the server that is down.
fn page(status: StatusCode, retry_after: u64, title: &str, body: &str) -> Response<Body> {
    let html = format!(
        "<!doctype html>\n\
         <html lang=\"en\"><head>\
         <meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"{retry_after}\">\
         <title>{title} -- arc dev</title>\
         <style>body{{font:14px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;\
         margin:3rem auto;max-width:60rem;padding:0 1.5rem}}\
         h1{{font-size:1rem;font-weight:600}}\
         pre{{white-space:pre-wrap;overflow-x:auto;padding:1rem;\
         border:1px solid currentColor;border-radius:4px}}</style>\
         </head><body><h1>{title}</h1>{body}\
         <p><small>arc dev</small></p></body></html>\n"
    );

    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::RETRY_AFTER, retry_after.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(html))
        .unwrap_or_else(|_| {
            // Every header above is well-formed by construction, so this is
            // unreachable -- but a supervisor that panics while reporting a
            // failure has turned a recoverable state into a dead port.
            let mut fallback = Response::new(Body::from("arc dev: the application is unavailable"));
            *fallback.status_mut() = status;
            fallback
        })
}

/// Escape text for inclusion in HTML.
///
/// The compiler's diagnostics contain `<`, `>` and `&` constantly (generic
/// parameters, references, `&mut`), and an unescaped `<Foo>` silently eats
/// the rest of the line.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + raw.len() / 8);
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_parameters_in_a_diagnostic_do_not_eat_the_rest_of_the_line() {
        let escaped = escape("expected `Vec<String>`, found `&str`");
        assert!(!escaped.contains('<'), "{escaped}");
        assert!(escaped.contains("&lt;String&gt;"), "{escaped}");
        assert!(escaped.contains("&amp;str"), "{escaped}");
    }

    #[test]
    fn the_holding_page_asks_the_browser_to_come_back() {
        let response = building(Duration::from_secs(5));
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("1")
        );
    }

    #[test]
    fn the_holding_page_is_never_cached() {
        // A cached "building" page would outlive the build that caused it.
        for response in [
            building(Duration::from_secs(1)),
            compile_error("error[E0425]"),
            backend_gone("closed"),
        ] {
            assert_eq!(
                response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok()),
                Some("no-store")
            );
        }
    }

    #[test]
    fn a_compile_error_page_reports_a_failure_not_a_wait() {
        let response = compile_error("error[E0308]: mismatched types");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn a_dead_backend_is_a_gateway_failure_not_a_build() {
        let response = backend_gone("ipc send failed");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn every_page_recovers_without_a_manual_reload() {
        for response in [
            building(Duration::from_secs(1)),
            compile_error("error"),
            backend_gone("closed"),
        ] {
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("text/html; charset=utf-8")
            );
        }
    }
}
