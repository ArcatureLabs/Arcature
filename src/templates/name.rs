use super::error::TemplateError;

/// A validated project name. One strict cross-ecosystem rule: non-empty,
/// at most 64 chars, starts with an ASCII lowercase letter, only ASCII
/// lowercase/digits/single hyphens, no trailing hyphen, no `--`.
#[derive(Debug, Clone)]
pub struct ProjectName {
    raw: String,
}

impl ProjectName {
    /// Parse a project name.
    pub fn parse(s: &str) -> Result<Self, TemplateError> {
        if s.is_empty() {
            return Err(TemplateError::InvalidName {
                reason: "name must not be empty".into(),
            });
        }
        if s.len() > 64 {
            return Err(TemplateError::InvalidName {
                reason: "name must not exceed 64 characters".into(),
            });
        }
        if !s
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase())
            .unwrap_or(false)
        {
            return Err(TemplateError::InvalidName {
                reason: "name must start with an ASCII lowercase letter".into(),
            });
        }
        if !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(TemplateError::InvalidName {
                reason: "name must contain only ASCII lowercase, digits, or hyphens".into(),
            });
        }
        if s.ends_with('-') {
            return Err(TemplateError::InvalidName {
                reason: "name must not end with a hyphen".into(),
            });
        }
        if s.contains("--") {
            return Err(TemplateError::InvalidName {
                reason: "name must not contain consecutive hyphens".into(),
            });
        }
        Ok(Self { raw: s.to_string() })
    }

    /// The raw name (e.g. `my-app`).
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// The Rust identifier form (hyphens -> underscores, e.g. `my_app`).
    pub fn rust_identifier(&self) -> String {
        self.raw.replace('-', "_")
    }
}
