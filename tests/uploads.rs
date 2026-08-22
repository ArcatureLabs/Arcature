//! What an upload route refuses, end to end.
//!
//! The unit tests beside each piece pin the piece: the bounds in
//! `src/http/multipart.rs`, the sanitizer in `src/storage/filename.rs`, the
//! magic-byte check in `src/storage/sniff.rs`. These pin the thing that
//! actually ships -- a real route, a real disk, a real
//! `multipart/form-data` body -- because an upload endpoint fails at the
//! seams between those pieces, not inside them.
//!
//! Every test here asks the same two questions of a hostile request: what
//! status came back, and **what is on the disk afterwards**. The second is
//! the one that matters. A 4xx with the file written anyway is not a
//! rejection; it is a rejection notice attached to a successful upload.

#![cfg(feature = "uploads")]

use std::path::Path;

use arcature::routing::{Route, Routes};
use arcature::storage::{AllowedExtensions, Disk, Storage, StorageConfig};
use arcature::validation::upload::{UploadPolicy, UploadedFile};
use arcature::{MultipartLimits, http};
use axum::Extension;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use tower::ServiceExt as _;

const BOUNDARY: &str = "XbArCaTuReTeStBoUnDaRy";

/// Sixteen bytes of a real PNG: the signature plus the start of `IHDR`.
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D', b'R',
];

/// A JPEG's leading bytes.
const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F', 0];

/// A web shell. The payload every one of these tests is really about.
const PHP: &[u8] = b"<?php system($_GET['c']); ?>";

/// One part of a multipart body.
struct Part<'a> {
    name: &'a str,
    filename: Option<&'a str>,
    bytes: &'a [u8],
}

impl<'a> Part<'a> {
    fn file(filename: &'a str, bytes: &'a [u8]) -> Self {
        Self {
            name: "file",
            filename: Some(filename),
            bytes,
        }
    }
}

/// Encode `parts` as a `multipart/form-data` body.
///
/// Hand-rolled rather than taken from a client library on purpose: half of
/// these tests need a header a well-behaved client would refuse to write.
fn body(parts: &[Part<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{}\"", part.name).as_bytes(),
        );
        if let Some(filename) = part.filename {
            out.extend_from_slice(format!("; filename=\"{filename}\"").as_bytes());
        }
        // A declared type, always a lie, always ignored.
        out.extend_from_slice(b"\r\nContent-Type: image/png\r\n\r\n");
        out.extend_from_slice(part.bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

/// The handler under test: store the file and report what was kept.
///
/// It answers `"<sanitized filename>|<storage key>"`, which is the pair a
/// caller of this route would put in a database -- one to show a human, one
/// to fetch the bytes back.
// A handler's error type is a whole `Response`, which is what an axum
// handler returns; boxing it would only make the test unlike the code.
#[allow(clippy::result_large_err)]
async fn accept(
    Extension(disk): Extension<Disk>,
    file: UploadedFile,
) -> Result<String, axum::response::Response> {
    let address = file.store_under(&disk, "uploads").await.map_err(|_| {
        axum::response::IntoResponse::into_response(
            arcature::Problem::of(arcature::ProblemKind::Internal).with_detail("storage failed"),
        )
    })?;
    let key = address
        .path_under("uploads")
        .expect("the prefix the store just used is a valid key");
    Ok(format!("{}|{}", file.filename(), key))
}

/// A route at `/upload`, with a disk rooted in a directory that dies with
/// the test.
async fn route(
    policy: Option<UploadPolicy>,
    limits: Option<MultipartLimits>,
) -> (tempfile::TempDir, axum::Router, Disk) {
    let root = tempfile::tempdir().expect("a temporary directory");
    let config = StorageConfig::fs(root.path().to_string_lossy().into_owned())
        .expect("the temporary path is a valid root");
    let storage = Storage::connect(config).await.expect("the disk connects");
    let disk = storage.default_disk();

    let mut route = Route::post("/upload", accept).layer(Extension(disk.clone()));
    if let Some(policy) = policy {
        route = route.layer(policy);
    }
    if let Some(limits) = limits {
        route = route.layer(limits);
    }
    (root, Routes::new([route]).into_router(), disk)
}

/// POST `parts` to `/upload`.
async fn post(router: &axum::Router, parts: &[Part<'_>]) -> (StatusCode, String) {
    post_body(router, Body::from(body(parts))).await
}

async fn post_body(router: &axum::Router, body: Body) -> (StatusCode, String) {
    let request = Request::builder()
        .method("POST")
        .uri("/upload")
        .header(
            axum::http::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .expect("the hand-built request is well-formed");

    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("a readable body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Every regular file under `root`, as slash-joined relative paths.
fn files_under(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                let relative = path.strip_prefix(base).unwrap_or(&path);
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

#[tokio::test]
async fn a_real_image_is_stored_under_a_digest_and_nothing_else() {
    let (root, router, _disk) = route(None, None).await;

    let (status, body) = post(&router, &[Part::file("holiday.PNG", PNG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (filename, key) = body.split_once('|').expect("the handler's pair");
    assert_eq!(filename, "holiday.png");
    // The key is the digest, fanned out, and owes nothing to the name.
    assert!(key.starts_with("uploads/"), "{key}");
    assert!(key.ends_with(".png"), "{key}");
    assert!(!key.contains("holiday"), "{key}");

    let files = files_under(root.path());
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0], key);
}

#[tokio::test]
async fn a_traversal_filename_is_stripped_rather_than_followed() {
    let (root, router, _disk) = route(None, None).await;

    let (status, body) = post(&router, &[Part::file("../../etc/passwd.png", PNG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (filename, key) = body.split_once('|').expect("the handler's pair");
    // The directory part is discarded, not obeyed and not escaped.
    assert_eq!(filename, "passwd.png");
    assert!(!key.contains(".."), "{key}");
    assert!(!key.contains("etc"), "{key}");

    // And on disk: one object, inside the root, under the digest.
    let files = files_under(root.path());
    assert_eq!(files, vec![key.to_string()]);
}

#[tokio::test]
async fn a_backslash_traversal_from_a_windows_client_is_stripped_too() {
    let (root, router, _disk) = route(None, None).await;

    let (status, body) = post(
        &router,
        &[Part::file("..\\..\\Windows\\System32\\evil.png", PNG)],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (filename, _) = body.split_once('|').expect("the handler's pair");
    assert_eq!(filename, "evil.png");
    assert_eq!(files_under(root.path()).len(), 1);
}

#[tokio::test]
async fn a_null_byte_in_the_filename_is_refused_and_stores_nothing() {
    let (root, router, _disk) = route(None, None).await;

    // `a\0b.png`: the classic truncation trick against a C consumer
    // downstream, which reads the name as `a` and the type as whatever it
    // pleases.
    let filename = "a\u{0}b.png";
    let (status, _) = post(&router, &[Part::file(filename, PNG)]).await;

    assert!(status.is_client_error(), "{status}");
    assert!(files_under(root.path()).is_empty());
}

#[tokio::test]
async fn a_windows_device_name_is_refused_and_stores_nothing() {
    for filename in ["CON.png", "con.png", "NUL.png", "LPT1.png", "CON.foo.png"] {
        let (root, router, _disk) = route(None, None).await;
        let (status, body) = post(&router, &[Part::file(filename, PNG)]).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{filename}");
        assert!(
            body.contains("problem") || body.contains("errors"),
            "{body}"
        );
        assert!(files_under(root.path()).is_empty(), "{filename}");
    }
}

#[tokio::test]
async fn an_executable_extension_is_refused_and_stores_nothing() {
    for filename in [
        "shell.php",
        "shell.phtml",
        "payload.exe",
        "payload.sh",
        "index.jsp",
        "web.config",
    ] {
        let (root, router, _disk) = route(None, None).await;
        let (status, _) = post(&router, &[Part::file(filename, PNG)]).await;

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{filename}");
        assert!(files_under(root.path()).is_empty(), "{filename}");
    }
}

#[tokio::test]
async fn a_php_script_renamed_to_an_image_is_refused_and_stores_nothing() {
    for filename in ["avatar.jpg", "avatar.png", "avatar.gif", "avatar.webp"] {
        let (root, router, _disk) = route(None, None).await;
        let (status, body) = post(&router, &[Part::file(filename, PHP)]).await;

        // The extension is on the whitelist and the declared `Content-Type`
        // says `image/png`. Only the bytes disagree, and the bytes win.
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{filename}");
        assert!(!body.contains("php"), "{body}");
        assert!(files_under(root.path()).is_empty(), "{filename}");
    }
}

#[tokio::test]
async fn a_double_extension_does_not_smuggle_a_script_past_the_whitelist() {
    let (root, router, _disk) = route(None, None).await;

    // `.png` is what a consumer that splits on the last dot reads, which is
    // every consumer that matters, so this one is admitted -- with the
    // script half flattened into the stem, never a second extension.
    let (status, body) = post(&router, &[Part::file("avatar.php.png", PNG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (filename, key) = body.split_once('|').expect("the handler's pair");
    assert_eq!(filename, "avatar_php.png");
    assert!(key.ends_with(".png"), "{key}");
    assert!(!key.contains("php"), "{key}");
    assert_eq!(files_under(root.path()).len(), 1);
}

#[tokio::test]
async fn a_vietnamese_filename_survives_intact() {
    let (root, router, _disk) = route(None, None).await;

    let (status, body) = post(&router, &[Part::file("ảnh đại diện.png", PNG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (filename, key) = body.split_once('|').expect("the handler's pair");
    // Sanitizing a name is not the same as ASCII-folding it. The accents,
    // and the spaces, are still there.
    assert_eq!(filename, "ảnh đại diện.png");
    // And none of it reaches the key, which is the digest.
    assert!(key.is_ascii(), "{key}");
    assert_eq!(files_under(root.path()).len(), 1);
}

#[tokio::test]
async fn a_decomposed_and_a_composed_vietnamese_name_agree() {
    let (_root, router, _disk) = route(None, None).await;

    // The same name typed two ways: one code point, or a letter plus two
    // combining marks. They render identically, so they must compare
    // identically.
    let composed = "\u{1EA3}nh.png";
    let decomposed = "a\u{309}nh.png";
    let (_, first) = post(&router, &[Part::file(composed, PNG)]).await;
    let (_, second) = post(&router, &[Part::file(decomposed, PNG)]).await;

    assert_eq!(first, second);
}

#[tokio::test]
async fn an_oversized_body_is_refused_with_413_and_stores_nothing() {
    let (root, router, _disk) = route(
        None,
        Some(
            MultipartLimits::new()
                .with_total_bytes(64 * 1024)
                .with_field_bytes(64 * 1024),
        ),
    )
    .await;

    // Two megabytes against a sixty-four kilobyte cap.
    let mut payload = Vec::with_capacity(2 * 1024 * 1024);
    payload.extend_from_slice(PNG);
    payload.resize(2 * 1024 * 1024, b'A');

    let (status, _) = post(&router, &[Part::file("huge.png", &payload)]).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(files_under(root.path()).is_empty());
}

#[tokio::test]
async fn a_thousand_field_body_is_refused_on_the_field_count() {
    let (root, router, _disk) = route(None, Some(MultipartLimits::new().with_fields(8))).await;

    let parts: Vec<Part<'_>> = (0..1000)
        .map(|_| Part {
            name: "junk",
            filename: None,
            bytes: b"x",
        })
        .collect();

    let (status, _) = post(&router, &parts).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(files_under(root.path()).is_empty());
}

#[tokio::test]
async fn a_route_that_widens_the_whitelist_still_holds_the_bytes_to_it() {
    let (root, router, _disk) = route(
        Some(UploadPolicy::new(
            AllowedExtensions::new(["png", "jpg", "pdf"]).expect("a valid whitelist"),
        )),
        None,
    )
    .await;

    // The extension is now allowed; the bytes are still a JPEG.
    let (status, _) = post(&router, &[Part::file("scan.pdf", JPEG)]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(files_under(root.path()).is_empty());

    // The same bytes under the right name are fine.
    let (status, body) = post(&router, &[Part::file("scan.jpg", JPEG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(files_under(root.path()).len(), 1);
}

#[tokio::test]
async fn a_refusal_never_quotes_the_request_back() {
    let (_root, router, _disk) = route(None, None).await;

    let (status, body) = post(&router, &[Part::file("<script>alert(1)</script>.php", PHP)]).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Not the filename, not the markup in it, not the extension, and not
    // what the bytes turned out to be -- that last one would make the
    // endpoint a free file-type oracle.
    assert!(!body.contains("script"), "{body}");
    assert!(!body.contains("alert"), "{body}");
    assert!(!body.contains("php"), "{body}");
}

#[tokio::test]
async fn a_stored_file_is_served_back_as_an_attachment() {
    let (_root, router, disk) = route(None, None).await;

    let (status, body) = post(&router, &[Part::file("holiday.png", PNG)]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, key) = body.split_once('|').expect("the handler's pair");
    let path = arcature::StoragePath::new(key).expect("the handler returned a valid key");

    let attachment = http::Attachment::from_disk(&disk, &path)
        .await
        .expect("the object reads back");
    let response = axum::response::IntoResponse::into_response(attachment);
    let headers = response.headers();

    assert_eq!(
        headers
            .get(axum::http::header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok()),
        Some("attachment")
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
}
