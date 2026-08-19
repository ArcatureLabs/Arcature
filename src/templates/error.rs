use std::path::PathBuf;

/// An error from the template generator.
#[derive(Debug)]
pub enum TemplateError {
    /// The destination already exists.
    ExistingTarget { path: PathBuf },
    /// The destination path is invalid.
    InvalidDestination { reason: String },
    /// The project name is invalid.
    InvalidName { reason: String },
    /// An I/O error.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The final rename (staging -> target) failed.
    Rename {
        staging: PathBuf,
        target: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExistingTarget { path } => {
                write!(f, "destination already exists: {}", path.display())
            }
            Self::InvalidDestination { reason } => {
                write!(f, "invalid destination: {reason}")
            }
            Self::InvalidName { reason } => write!(f, "invalid project name: {reason}"),
            Self::Io { path, source } => {
                write!(f, "I/O error at {}: {source}", path.display())
            }
            Self::Rename {
                staging,
                target,
                source,
            } => {
                write!(
                    f,
                    "rename {} -> {} failed: {source}",
                    staging.display(),
                    target.display()
                )
            }
        }
    }
}

impl std::error::Error for TemplateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::Rename { source, .. } => Some(source),
            _ => None,
        }
    }
}
