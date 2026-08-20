//! `arc doctor` — diagnose the local environment.
//!
//! A read-only health check that reports the framework version, the Rust
//! toolchain version, and (when `DATABASE_URL` is set) whether PostgreSQL is
//! reachable. It never writes anything; failures are reported as warnings,
//! not errors, so a partial environment still produces a full report.

/// Execute the `doctor` subcommand: print the environment report.
pub fn run() -> Result<(), DoctorError> {
    let mut all_ok = true;

    report(
        "framework version",
        &format!("arcature {}", crate::FRAMEWORK_VERSION),
        true,
    );

    // Rust toolchain.
    match rust_version() {
        Ok(v) => report("rust toolchain", &v, true),
        Err(e) => {
            report("rust toolchain", &e.to_string(), false);
            all_ok = false;
        }
    }

    // Database reachability (only when DATABASE_URL is set).
    if let Ok(url) = std::env::var("DATABASE_URL") {
        match crate::database::DatabaseConfig::new(&url) {
            Ok(cfg) => {
                // Blocking call into the async connect: doctor is a short-lived
                // CLI tool, so a tiny single-threaded runtime is fine.
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|source| DoctorError::Runtime { source })?;
                match rt.block_on(crate::database::Db::connect(cfg)) {
                    Ok(db) => {
                        match rt.block_on(db.ping()) {
                            Ok(()) => report("database", "reachable", true),
                            Err(e) => {
                                report("database ping", &e.to_string(), false);
                                all_ok = false;
                            }
                        }
                        rt.block_on(db.close());
                    }
                    Err(e) => {
                        report("database connect", &e.to_string(), false);
                        all_ok = false;
                    }
                }
            }
            Err(e) => {
                report("DATABASE_URL", &e.to_string(), false);
                all_ok = false;
            }
        }
    } else {
        report("database", "DATABASE_URL unset (skipped)", true);
    }

    if all_ok {
        println!("\nall checks passed");
        Ok(())
    } else {
        println!("\nsome checks failed");
        Err(DoctorError::ChecksFailed)
    }
}

/// Fetch the `rustc` version string (e.g. `rustc 1.97.1 ...`).
fn rust_version() -> Result<String, DoctorError> {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .map_err(|source| DoctorError::Rustc { source })?;
    if !output.status.success() {
        return Err(DoctorError::RustcFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Print a labeled check line.
fn report(label: &str, value: &str, ok: bool) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("[{mark}] {label}: {value}");
}

/// An error from the `doctor` command.
#[derive(Debug)]
pub enum DoctorError {
    /// The Tokio runtime could not be built.
    Runtime { source: std::io::Error },
    /// `rustc` could not be invoked.
    Rustc { source: std::io::Error },
    /// `rustc` exited non-zero.
    RustcFailed { stderr: String },
    /// One or more checks failed.
    ChecksFailed,
}

impl std::fmt::Display for DoctorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime { source } => write!(f, "failed to build runtime: {source}"),
            Self::Rustc { source } => write!(f, "failed to invoke rustc: {source}"),
            Self::RustcFailed { stderr } => write!(f, "rustc --version failed: {stderr}"),
            Self::ChecksFailed => f.write_str("one or more environment checks failed"),
        }
    }
}

impl std::error::Error for DoctorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime { source } | Self::Rustc { source } => Some(source),
            _ => None,
        }
    }
}
