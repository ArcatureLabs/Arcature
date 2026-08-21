//! Minting and probing the two IPC endpoints.
//!
//! `arc dev` creates two endpoints per invocation -- one for Vite, one for
//! the application -- and passes their paths to the two children through
//! [`crate::config::VITE_IPC_ENV`] and [`crate::config::APP_IPC_ENV`].
//! Nothing else may reach them: they carry unauthenticated development
//! traffic, and the whole one-port design rests on the browser having
//! exactly one way in.
//!
//! # Why per-process paths
//!
//! Two `arc dev` runs in two checkouts are a normal thing to want. A fixed
//! path would make the second one either fail to bind or, worse, silently
//! forward half its requests into the first one's Vite. The process id is
//! enough to separate them, and it disappears when the process does.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::dev_proxy::endpoint::IpcEndpoint;

/// How often to retry while waiting for a child to start listening.
const PROBE_INTERVAL: Duration = Duration::from_millis(25);

/// The two endpoints one `arc dev` invocation owns.
#[derive(Debug, Clone)]
pub(crate) struct Endpoints {
    /// Where Vite's `middlewareMode` HTTP server listens.
    pub(crate) vite: PathBuf,
    /// Where the application listens instead of binding a port.
    pub(crate) app: PathBuf,
}

impl Endpoints {
    /// Mint a fresh pair for this process.
    ///
    /// On Unix the sockets live in the project's own scratch directory
    /// rather than the shared temp directory: `/tmp` is world-writable and a
    /// socket path there is guessable, while the scratch directory inherits
    /// the project's permissions. On Windows the name is the pipe namespace,
    /// which has no project-relative form.
    pub(crate) fn mint(scratch: &Path) -> Self {
        let pid = std::process::id();
        #[cfg(windows)]
        {
            let _ = scratch;
            Self {
                vite: PathBuf::from(format!(r"\\.\pipe\arcature-vite-{pid}")),
                app: PathBuf::from(format!(r"\\.\pipe\arcature-app-{pid}")),
            }
        }
        #[cfg(unix)]
        {
            Self {
                vite: scratch.join(format!("vite-{pid}.sock")),
                app: scratch.join(format!("app-{pid}.sock")),
            }
        }
    }
}

/// Wait until something is listening on `path`, or give up.
///
/// Probing by connecting is the only test that means anything: on Unix a
/// socket file exists from the moment it is bound but also after the process
/// that bound it died, and on Windows there is no file to look at.
///
/// The probe connection is opened and dropped. Both children answer HTTP, so
/// a connection with no request on it costs them a closed socket.
///
/// # Errors
///
/// [`WaitError::TimedOut`] if nothing accepted within `timeout`.
pub(crate) async fn wait_until_listening(
    path: &Path,
    timeout: Duration,
    mut still_alive: impl FnMut() -> Option<String>,
) -> Result<Duration, WaitError> {
    let endpoint = IpcEndpoint::new(path.to_path_buf());
    let started = Instant::now();
    let deadline = started + timeout;

    loop {
        if let Ok(stream) = endpoint.connect().await {
            drop(stream);
            return Ok(started.elapsed());
        }
        // A child that has already exited will never start listening, and
        // waiting the full timeout for it only delays the real error.
        if let Some(reason) = still_alive() {
            return Err(WaitError::Died {
                path: path.to_path_buf(),
                reason,
            });
        }
        if Instant::now() >= deadline {
            return Err(WaitError::TimedOut {
                path: path.to_path_buf(),
                waited: started.elapsed(),
            });
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// A child never started listening.
#[derive(Debug)]
pub(crate) enum WaitError {
    /// The deadline passed with nothing accepting.
    TimedOut { path: PathBuf, waited: Duration },
    /// The child exited before it started listening.
    Died { path: PathBuf, reason: String },
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut { path, waited } => write!(
                formatter,
                "nothing is listening on {} after {:.1}s. \
                 There is no fallback to a second TCP port -- the one-port design depends on \
                 IPC working -- so this is where `arc dev` stops.",
                path.display(),
                waited.as_secs_f32()
            ),
            Self::Died { path, reason } => write!(
                formatter,
                "the process that should listen on {} exited first: {reason}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WaitError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_endpoints_are_never_the_same_path() {
        let endpoints = Endpoints::mint(Path::new("/tmp/arcature-scratch"));
        assert_ne!(endpoints.vite, endpoints.app);
    }

    #[test]
    fn an_endpoint_name_carries_this_process_id() {
        let endpoints = Endpoints::mint(Path::new("/tmp/arcature-scratch"));
        let pid = std::process::id().to_string();
        assert!(
            endpoints.vite.to_string_lossy().contains(&pid),
            "{:?} should be private to this process",
            endpoints.vite
        );
        assert!(endpoints.app.to_string_lossy().contains(&pid));
    }

    #[tokio::test]
    async fn waiting_for_an_endpoint_nobody_serves_times_out_with_advice() {
        let path = std::env::temp_dir().join("arcature-never-listens.sock");
        let error = wait_until_listening(&path, Duration::from_millis(80), || None)
            .await
            .expect_err("nothing is listening there");
        let message = error.to_string();
        assert!(message.contains("second TCP port"), "{message}");
    }

    #[tokio::test]
    async fn waiting_stops_early_when_the_child_has_already_exited() {
        let path = std::env::temp_dir().join("arcature-dead-child.sock");
        let started = Instant::now();
        let error = wait_until_listening(&path, Duration::from_secs(30), || {
            Some(String::from("exit status 101"))
        })
        .await
        .expect_err("the child is gone");
        assert!(matches!(error, WaitError::Died { .. }));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a dead child must not cost the full timeout"
        );
    }
}
