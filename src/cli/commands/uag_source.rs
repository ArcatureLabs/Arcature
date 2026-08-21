//! Where the CLI gets the application graph from.
//!
//! `arc routes`, `arc typegen` and `arc build` all need the same thing: the
//! [`UagArtifact`] of the project in the current directory. The artifact is
//! assembled from `&'static` metadata inside the application, so the CLI --
//! a separate process that never links the application -- has to ask the
//! application for it.
//!
//! There are two ways to ask, and the order matters for how fast the loop
//! feels:
//!
//! 1. **A running `arc dev`.** The supervisor writes its listening address
//!    into `.arcature/dev.addr`, and the application it started serves
//!    `GET /_arcature/uag.json`. Reading it costs one loopback request and no
//!    compilation at all, which is the whole reason the endpoint exists.
//! 2. **The `uag` binary.** Without a dev server -- on CI, or in a terminal
//!    where nothing is running -- there is nothing to ask, so the project's
//!    own `--bin uag` is built and run for its stdout. It is a real link
//!    step, which is why it is the fallback and not the default: the
//!    scaffold gates that target behind its own `uag` feature precisely so
//!    the edit loop never pays for it.
//!
//! Nothing here formats a diagnostic or writes a file. It returns the
//! artifact and says which of the two answered.

use std::path::{Path, PathBuf};

use super::dev::project::{DEV_ADDRESS_FILE, SCRATCH_DIR};
use crate::uag::UagArtifact;

/// Which of the two sources answered.
///
/// Reported so the command can say it in one line. "typegen took nine
/// seconds" and "typegen took nine seconds because it had to build the uag
/// binary" are different messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// A running `arc dev`, at this address.
    DevServer(String),
    /// The project's own `uag` binary.
    Binary,
}

impl std::fmt::Display for Source {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DevServer(address) => write!(formatter, "the dev server on {address}"),
            Self::Binary => formatter.write_str("`cargo run --bin uag`"),
        }
    }
}

/// A loaded artifact and the source it came from.
pub(crate) struct Loaded {
    /// The application graph.
    pub(crate) artifact: UagArtifact,
    /// Which source answered.
    pub(crate) source: Source,
    /// The project root the artifact describes.
    pub(crate) root: PathBuf,
}

/// Find the project containing `start` and load its artifact.
///
/// # Errors
///
/// [`UagSourceError::NoProject`] when no ancestor holds a `Cargo.toml`, and
/// whatever the chosen source failed with otherwise. A dev server that is
/// listed but does not answer is *not* an error: the address file outlives a
/// `Ctrl-C` that could not clean up, so the binary is tried instead.
pub(crate) fn load(start: &Path) -> Result<Loaded, UagSourceError> {
    let root = project_root(start)?;

    if let Some(address) = dev_address(&root) {
        match fetch(&address) {
            Ok(bytes) => {
                return Ok(Loaded {
                    artifact: parse(&bytes)?,
                    source: Source::DevServer(address),
                    root,
                });
            }
            Err(error) => {
                // Said once, not swallowed: a developer who has `arc dev` open
                // in another pane should know why this took ten seconds.
                eprintln!(
                    "arc: {address} did not answer ({error}); building the uag binary instead"
                );
            }
        }
    }

    let bytes = run_binary(&root)?;
    Ok(Loaded {
        artifact: parse(&bytes)?,
        source: Source::Binary,
        root,
    })
}

/// The nearest ancestor of `start` holding a `Cargo.toml`.
///
/// Deliberately weaker than [`dev::project::Project::discover`](super::dev):
/// that one also insists on a `package.json` because it is about to run Vite,
/// and reading the route table is not.
pub(crate) fn project_root(start: &Path) -> Result<PathBuf, UagSourceError> {
    start
        .ancestors()
        .find(|dir| dir.join("Cargo.toml").is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| UagSourceError::NoProject {
            from: start.to_path_buf(),
        })
}

/// The address a running `arc dev` published, if there is one.
fn dev_address(root: &Path) -> Option<String> {
    let path = root.join(SCRATCH_DIR).join(DEV_ADDRESS_FILE);
    let address = std::fs::read_to_string(path).ok()?;
    let address = address.trim().to_owned();
    (!address.is_empty()).then_some(address)
}

/// Deserialize the artifact.
fn parse(bytes: &[u8]) -> Result<UagArtifact, UagSourceError> {
    serde_json::from_slice(bytes).map_err(|source| UagSourceError::Json { source })
}

/// The header separator, and the reason the response parsing below is four
/// lines rather than a client library.
const HEADER_END: &[u8] = b"\r\n\r\n";

/// How long to wait for the dev server. Short: it is on loopback, and a
/// stale address file should cost a moment, not a stall.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);

/// How long to wait for the answer once connected.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// `GET /_arcature/uag.json` from a running dev server, over plain TCP.
///
/// Hand-written rather than routed through hyper because the request carries
/// `Connection: close`: the server closes when it is done, so the body is
/// everything after the first blank line, and there is no chunked framing, no
/// keep-alive bookkeeping and no async runtime to start for one loopback
/// request.
fn fetch(address: &str) -> std::io::Result<Vec<u8>> {
    use std::io::{Read as _, Write as _};
    use std::net::{TcpStream, ToSocketAddrs as _};

    let resolved = address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other(format!("`{address}` resolved to no address")))?;
    let mut stream = TcpStream::connect_timeout(&resolved, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nAccept: application/json\r\n\
         User-Agent: arc\r\nConnection: close\r\n\r\n",
        path = crate::application::uag_endpoint::PATH,
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    body_of(&raw)
}

/// Split an HTTP/1.1 response into status and body, insisting on a `200`.
///
/// Separated from [`fetch`] so the parsing is testable without a socket.
fn body_of(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    let split = raw
        .windows(HEADER_END.len())
        .position(|window| window == HEADER_END)
        .ok_or_else(|| std::io::Error::other("the response had no header terminator"))?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.lines().next().unwrap_or_default().trim().to_owned();
    if !status.contains(" 200") {
        // A `404` here means the application was built without the endpoint,
        // which is the interesting case: say what the answer was, rather than
        // reporting a parse failure over an HTML error page.
        return Err(std::io::Error::other(format!(
            "the dev server answered `{status}`, so it is not serving {}. Check that \
             the project's `dev` feature enables `arcature/uag` and that bootstrap/app.rs \
             calls `.uag_endpoint(..)`.",
            crate::application::uag_endpoint::PATH
        )));
    }
    Ok(raw[split + HEADER_END.len()..].to_vec())
}

/// Build and run the project's `uag` binary for its stdout.
///
/// `--features uag` names the scaffold's own feature, not `arcature/uag`: the
/// binary target carries `required-features = ["uag"]` so that
/// `cargo build --features dev`, which `arc dev` runs on every save, does not
/// link it.
fn run_binary(root: &Path) -> Result<Vec<u8>, UagSourceError> {
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--features", "uag", "--bin", "uag"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        // Inherited: compiler diagnostics belong to the developer, and holding
        // them back until the command fails would hide a long build's reason.
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|source| UagSourceError::Cargo { source })?;

    if !output.status.success() {
        return Err(UagSourceError::Binary {
            status: output.status,
        });
    }
    if output.stdout.is_empty() {
        return Err(UagSourceError::Empty);
    }
    Ok(output.stdout)
}

/// A failure obtaining the application graph.
#[derive(Debug)]
pub(crate) enum UagSourceError {
    /// No ancestor of the working directory is a crate.
    NoProject {
        /// Where the search started.
        from: PathBuf,
    },
    /// `cargo` could not be run at all.
    Cargo {
        /// The spawn failure.
        source: std::io::Error,
    },
    /// The `uag` binary did not succeed.
    Binary {
        /// Its exit status.
        status: std::process::ExitStatus,
    },
    /// The `uag` binary succeeded but printed nothing.
    Empty,
    /// The bytes were not a [`UagArtifact`].
    Json {
        /// The deserialization failure.
        source: serde_json::Error,
    },
}

impl std::fmt::Display for UagSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProject { from } => write!(
                formatter,
                "no Cargo.toml in {} or any parent directory, so there is no \
                 application graph to read",
                from.display()
            ),
            Self::Cargo { source } => write!(
                formatter,
                "could not run cargo to build the uag binary: {source}"
            ),
            Self::Binary { status } => write!(
                formatter,
                "`cargo run --features uag --bin uag` exited with {status}. A project \
                 generated before this command existed has no such target: add \
                 src/bin/uag.rs and the matching [[bin]] section, or start `arc dev` \
                 and let the endpoint answer instead."
            ),
            Self::Empty => formatter.write_str(
                "the uag binary printed nothing; it is supposed to write the artifact \
                 JSON to stdout",
            ),
            Self::Json { source } => write!(
                formatter,
                "the application graph could not be read: {source}. The usual cause is \
                 an application built against a different version of arcature than this \
                 `arc` -- rebuild both from the same one."
            ),
        }
    }
}

impl std::error::Error for UagSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cargo { source } => Some(source),
            Self::Json { source } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "arcature-uag-source-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    #[test]
    fn a_directory_with_no_crate_above_it_has_no_graph_to_read() {
        let dir = scratch("no-crate");
        let error = project_root(&dir).expect_err("there is no Cargo.toml here");
        assert!(error.to_string().contains("Cargo.toml"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dev_address_is_read_from_the_scratch_directory() {
        let dir = scratch("dev-addr");
        let arcature = dir.join(crate::cli::commands::dev::project::SCRATCH_DIR);
        std::fs::create_dir_all(&arcature).expect("scratch");
        std::fs::write(arcature.join(DEV_ADDRESS_FILE), "127.0.0.1:4173\n").expect("write");
        assert_eq!(dev_address(&dir).as_deref(), Some("127.0.0.1:4173"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_address_file_is_the_same_as_no_dev_server() {
        let dir = scratch("empty-addr");
        let arcature = dir.join(crate::cli::commands::dev::project::SCRATCH_DIR);
        std::fs::create_dir_all(&arcature).expect("scratch");
        std::fs::write(arcature.join(DEV_ADDRESS_FILE), "   \n").expect("write");
        assert_eq!(dev_address(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_two_hundred_response_yields_everything_after_the_blank_line() {
        let raw = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"a\":1}";
        assert_eq!(body_of(raw).expect("a 200 has a body"), b"{\"a\":1}");
    }

    #[test]
    fn a_not_found_names_the_status_instead_of_returning_the_error_page() {
        let raw = b"HTTP/1.1 404 Not Found\r\ncontent-type: text/html\r\n\r\n<html></html>";
        let error = body_of(raw).expect_err("a 404 is not an artifact");
        assert!(error.to_string().contains("404"), "{error}");
        assert!(
            error.to_string().contains("/_arcature/uag.json"),
            "the message should name the endpoint that is missing: {error}"
        );
    }

    #[test]
    fn a_response_with_no_header_terminator_is_refused() {
        assert!(body_of(b"HTTP/1.1 200 OK").is_err());
    }

    #[test]
    fn a_source_names_itself_so_a_slow_run_can_be_attributed() {
        assert!(
            Source::DevServer("127.0.0.1:3000".into())
                .to_string()
                .contains("127.0.0.1:3000")
        );
        assert!(Source::Binary.to_string().contains("uag"));
    }
}
