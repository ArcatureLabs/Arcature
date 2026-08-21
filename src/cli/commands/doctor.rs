//! `arc doctor` — diagnose the local environment.
//!
//! A read-only health check that reports the framework version, the Rust
//! toolchain, the two things that decide how a dev rebuild *feels* — which
//! linker cargo will use, and whether an antivirus reads every freshly
//! linked binary before it is allowed to run — and (when `DATABASE_URL` is
//! set) whether the database is reachable. It never writes anything.
//!
//! Three outcomes, not two. `[FAIL]` is for something that is broken and
//! makes the exit code non-zero; `[warn]` is for something that is working
//! but costing the developer time, which is exactly what the linker and
//! antivirus checks find. A slow loop is not a broken environment, and
//! failing the command over it would make `arc doctor` useless in CI.

use std::path::{Path, PathBuf};

/// Execute the `doctor` subcommand: print the environment report.
///
/// # Errors
///
/// Returns [`DoctorError::ChecksFailed`] when any check failed, so a script
/// can branch on the exit code. Warnings do not affect it.
pub fn run() -> Result<(), DoctorError> {
    let mut all_ok = true;

    report(
        "framework version",
        &format!("arcature {}", crate::FRAMEWORK_VERSION),
        true,
    );

    // Rust toolchain. `-vV` rather than `--version`, because the host triple
    // in the same output is what the linker check needs to know which
    // `[target.*]` section applies.
    let host = match rustc_verbose() {
        Ok(output) => {
            let release = field(&output, "release").unwrap_or("unknown");
            let host = field(&output, "host").unwrap_or("unknown");
            report("rust toolchain", &format!("{release} ({host})"), true);
            Some(host.to_string())
        }
        Err(error) => {
            report("rust toolchain", &error.to_string(), false);
            all_ok = false;
            None
        }
    };

    linker_check(host.as_deref());
    scanner_check();

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

// ---------------------------------------------------------------- linker

/// Report which linker cargo will hand rustc, and warn when it is the
/// platform default.
///
/// Link is the largest single part of a Rust rebuild that changed one
/// function body, and it is the part a developer can fix in one file, so it
/// is worth a line even when nothing is wrong.
fn linker_check(host: Option<&str>) {
    let Some(host) = host else {
        // Without the host triple there is no way to know which section
        // applies, and guessing the wrong one would report a linker that is
        // not in use. Say nothing rather than something false.
        return;
    };

    let Some((path, config)) = cargo_config() else {
        warn(
            "linker",
            "platform default -- no .cargo/config.toml. Link is the largest \
             part of a rebuild; `arc new` writes a config that selects a \
             faster one.",
        );
        return;
    };

    match configured_linker(&config, host) {
        Some(linker) => report(
            "linker",
            &format!("{linker} (from {})", path.display()),
            true,
        ),
        None => warn(
            "linker",
            &format!(
                "platform default -- {} sets none for {host}",
                path.display()
            ),
        ),
    }
}

/// The `.cargo/config.toml` that applies to the current directory.
///
/// Only this directory, not the ancestor walk cargo itself performs: `arc
/// doctor` is run from a project root, and a report that quotes the file it
/// actually read is more useful than one that silently reaches into a home
/// directory.
fn cargo_config() -> Option<(PathBuf, String)> {
    // Cargo accepts both spellings and prefers the extension-less one only
    // for backwards compatibility, so look for the modern name first.
    for name in [".cargo/config.toml", ".cargo/config"] {
        let path = PathBuf::from(name);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            return Some((path, contents));
        }
    }
    None
}

/// The linker configured for `target`, if the config names one.
///
/// A line scanner rather than a TOML parser: this crate has no TOML
/// dependency, the file is normally one `arc new` wrote, and "unknown" is a
/// fair answer for a diagnostic that meets something unexpected. Nothing
/// decides anything from this value -- it is printed.
fn configured_linker(config: &str, target: &str) -> Option<String> {
    let wanted = format!("target.{target}");
    let mut in_section = false;

    for line in config.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            // `[target.'cfg(windows)']` and friends are deliberately not
            // matched: deciding whether a cfg expression applies is the
            // compiler's job, and a wrong guess here prints a linker that
            // is not in use.
            in_section = header.replace(['"', '\''], "") == wanted;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(value) = line.strip_prefix("linker")
            && let Some(name) = quoted(value)
        {
            return Some(basename(&name));
        }
        // The other way to choose one: `rustflags = ["-Clink-arg=-fuse-ld=lld"]`.
        if line.starts_with("rustflags")
            && let Some(rest) = line.split("-fuse-ld=").nth(1)
        {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// The first double-quoted string in `value`, which follows a `key =`.
fn quoted(value: &str) -> Option<String> {
    let rest = value.trim_start().strip_prefix('=')?;
    let opened = rest.find('"')? + 1;
    let closed = rest[opened..].find('"')? + opened;
    Some(rest[opened..closed].to_string())
}

/// The file name of a path, without directories or extension.
fn basename(value: &str) -> String {
    Path::new(value)
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or(value)
        .to_string()
}

// ------------------------------------------------------------- av scanner

/// The directory cargo writes build output into.
///
/// Windows only, like its one caller: the scanner check below is the only
/// thing that needs this path, and a helper compiled on a platform where
/// nothing calls it is dead code that `-D warnings` is right to reject.
#[cfg(windows)]
fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("target")
        },
        PathBuf::from,
    )
}

/// The advice printed when a real-time scanner is watching `target/`.
///
/// The numbers are measured, not estimated: on the machine this was written
/// on, the first execution of a freshly linked 18 MB debug binary took 1.4
/// to 3.3 seconds, and running the identical file a second time took 9
/// milliseconds. Every `arc dev` reload pays the first number, because every
/// reload runs a binary that has just been written.
#[cfg(windows)]
fn scanner_advice(target: &Path) -> String {
    format!(
        "Microsoft Defender real-time protection is on and does not exclude \
         {}. Every `arc dev` reload runs a binary that was just linked, and \
         the first run of a new file is scanned in full -- measured at 1.4-3.3s \
         against 9ms for an already-scanned one. In an elevated PowerShell: \
         Add-MpPreference -ExclusionPath '{}'",
        target.display(),
        target.display()
    )
}

/// Warn when an antivirus will read every freshly linked binary.
#[cfg(windows)]
fn scanner_check() {
    let target = target_dir();
    match defender_status() {
        None => {
            // No Defender module, or PowerShell is unavailable. Another
            // scanner may still be running, but this cannot see it, and
            // inventing a warning for an antivirus nobody installed would
            // teach people to ignore the line.
            warn("antivirus", "could not ask Microsoft Defender (skipped)");
        }
        Some(DefenderStatus {
            real_time: false, ..
        }) => report("antivirus", "Defender real-time protection off", true),
        Some(DefenderStatus {
            exclusions: Some(exclusions),
            ..
        }) if excludes(&exclusions, &target) => {
            report("antivirus", "Defender excludes target/", true)
        }
        Some(_) => warn("antivirus", &scanner_advice(&target)),
    }
}

/// On every other platform there is nothing here to check.
///
/// Not a skipped line: a report that lists a check no operating system in
/// the room can fail is noise, and the reader has to read it every time.
#[cfg(not(windows))]
#[allow(clippy::missing_const_for_fn, reason = "the Windows arm is not const")]
fn scanner_check() {}

/// What Microsoft Defender says about itself.
#[cfg(windows)]
struct DefenderStatus {
    /// Is real-time protection on?
    real_time: bool,
    /// The exclusion list, or `None` when it could not be read -- which is
    /// the normal case, because reading it requires an elevated shell.
    exclusions: Option<String>,
}

/// Ask Defender whether it is on and what it ignores.
///
/// One PowerShell invocation for both facts. It costs a second or so, which
/// is affordable in a command whose whole purpose is to be run occasionally
/// and read carefully.
#[cfg(windows)]
fn defender_status() -> Option<DefenderStatus> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-MpComputerStatus).RealTimeProtectionEnabled; \
             try { (Get-MpPreference).ExclusionPath -join ';' } catch { '' }",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let real_time = lines.next()?.trim().eq_ignore_ascii_case("true");
    let rest = lines.collect::<Vec<_>>().join(";");

    // `Get-MpPreference` answers a non-administrator with a sentence rather
    // than an error, so an unreadable list arrives looking like one entry.
    let exclusions = if rest.trim().is_empty() || rest.contains("Must be an administrator") {
        None
    } else {
        Some(rest)
    };

    Some(DefenderStatus {
        real_time,
        exclusions,
    })
}

/// Does any entry in a `;`-separated exclusion list cover `path`?
///
/// Prefix matching on a normalised, case-folded form, because Windows paths
/// compare that way and an exclusion is inherited by everything beneath it.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "only the Windows check calls it")
)]
fn excludes(exclusions: &str, path: &Path) -> bool {
    let normalise = |value: &str| {
        value
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    let path = normalise(&path.display().to_string());

    exclusions
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(normalise)
        .any(|entry| {
            // `\` on the end of the prefix so that an exclusion of
            // `C:\work\app` does not also claim `C:\work\application`.
            path == entry || path.starts_with(&format!("{entry}\\"))
        })
}

// ---------------------------------------------------------------- output

/// Fetch `rustc -vV`, which reports the release *and* the host triple.
fn rustc_verbose() -> Result<String, DoctorError> {
    let output = std::process::Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|source| DoctorError::Rustc { source })?;
    if !output.status.success() {
        return Err(DoctorError::RustcFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One `name: value` field out of `rustc -vV` output.
fn field<'a>(output: &'a str, name: &str) -> Option<&'a str> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}: ")))
        .map(str::trim)
}

/// Print a labeled check line.
fn report(label: &str, value: &str, ok: bool) {
    let mark = if ok { "ok" } else { "FAIL" };
    println!("[{mark}] {label}: {value}");
}

/// Print a line that is worth saying without being a failure.
fn warn(label: &str, value: &str) {
    println!("[warn] {label}: {value}");
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
            Self::RustcFailed { stderr } => write!(f, "rustc -vV failed: {stderr}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "x86_64-pc-windows-msvc";

    #[test]
    fn the_host_triple_and_release_come_out_of_the_verbose_version() {
        let output = "rustc 1.97.1 (abcdef 2026-01-01)\nbinary: rustc\n\
                      host: x86_64-pc-windows-msvc\nrelease: 1.97.1\n";

        assert_eq!(field(output, "host"), Some(HOST));
        assert_eq!(field(output, "release"), Some("1.97.1"));
        assert_eq!(field(output, "nothing-like-this"), None);
    }

    #[test]
    fn a_linker_is_read_out_of_the_section_for_this_host() {
        let config = "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n\n\
                      [target.x86_64-pc-windows-msvc]\nlinker = \"rust-lld.exe\"\n";

        assert_eq!(configured_linker(config, HOST).as_deref(), Some("rust-lld"));
    }

    #[test]
    fn a_linker_set_only_for_another_target_is_not_this_target_s_linker() {
        // The reason the section has to be tracked at all: a config that
        // configures Linux and says nothing about Windows must report
        // nothing, not `clang`.
        let config = "[target.x86_64-unknown-linux-gnu]\nlinker = \"clang\"\n";

        assert_eq!(configured_linker(config, HOST), None);
    }

    #[test]
    fn the_linker_can_also_be_chosen_through_rustflags() {
        let config = "[target.x86_64-pc-windows-msvc]\n\
                      rustflags = [\"-Clink-arg=-fuse-ld=lld\"]\n";

        assert_eq!(configured_linker(config, HOST).as_deref(), Some("lld"));
    }

    #[test]
    fn a_commented_out_linker_is_not_in_use() {
        let config = "[target.x86_64-pc-windows-msvc]\n# linker = \"rust-lld\"\n";

        assert_eq!(configured_linker(config, HOST), None);
    }

    #[test]
    fn a_cfg_section_is_left_to_the_compiler_rather_than_guessed_at() {
        let config = "[target.'cfg(windows)']\nlinker = \"rust-lld\"\n";

        assert_eq!(configured_linker(config, HOST), None);
    }

    #[test]
    fn an_exclusion_covers_everything_underneath_it() {
        assert!(excludes(
            "C:\\Users\\dev\\work",
            Path::new("C:\\Users\\dev\\work\\app\\target")
        ));
    }

    #[test]
    fn the_directory_itself_counts_as_excluded() {
        assert!(excludes("C:\\app\\target", Path::new("C:\\app\\target")));
    }

    #[test]
    fn a_prefix_that_is_not_a_whole_directory_name_does_not_count() {
        // `C:\work\app` must not be read as covering `C:\work\application`,
        // which a plain string prefix test would do -- and the developer
        // would be told they are fine while every reload keeps paying.
        assert!(!excludes(
            "C:\\work\\app",
            Path::new("C:\\work\\application\\target")
        ));
    }

    #[test]
    fn matching_ignores_case_and_slash_direction() {
        assert!(excludes("c:/APP/", Path::new("C:\\app\\target")));
    }

    #[test]
    fn an_empty_list_excludes_nothing() {
        assert!(!excludes("", Path::new("C:\\app\\target")));
        assert!(!excludes(" ; ; ", Path::new("C:\\app\\target")));
    }

    #[test]
    fn the_advice_names_the_directory_and_the_command_to_run() {
        let advice = scanner_advice(Path::new("C:\\app\\target"));

        assert!(
            advice.contains("Add-MpPreference -ExclusionPath"),
            "{advice}"
        );
        assert!(advice.contains("C:\\app\\target"), "{advice}");
    }
}
