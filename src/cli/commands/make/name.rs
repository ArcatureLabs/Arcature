//! Turning what the developer typed into the two spellings a generator needs.
//!
//! `arc make:controller users/show` has to become a *path* (`users/` plus a
//! snake_case file stem) and a *type* (`ShowController`). Both come from the
//! same string, and getting them from two places is how they drift, so this
//! module is the only place either is derived.
//!
//! # Why not `templates::ProjectName`
//!
//! [`crate::templates::ProjectName`] validates a *project* name: strict
//! kebab-case, ASCII lowercase first character, no path separators. It rejects
//! `User` and `users/show` by design, and it exposes only `raw()` and
//! `rust_identifier()` -- neither of which is PascalCase. Reusing it here
//! would mean loosening the rules that make it useful for `arc new`. The two
//! answer different questions, so they stay separate types.

use std::fmt;

/// A generator name, split into the directory it nests under and the item
/// itself.
///
/// Built from anything the developer plausibly types: `user`, `User`,
/// `users/show`, `users\show`, `users::show`, `send-welcome-email`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactName {
    /// The directory segments the artifact nests under, already snake_case.
    /// Empty for a bare name.
    segments: Vec<String>,
    /// The final segment in snake_case, with no kind suffix applied.
    stem: String,
}

impl ArtifactName {
    /// Parse what the developer typed.
    ///
    /// # Errors
    ///
    /// Returns [`NameError`] for an empty name or a segment that is not a
    /// plain identifier. The character rule doubles as the path-traversal
    /// guard: `.` and `..` are not identifiers, so `../../etc/passwd` never
    /// becomes a path.
    pub fn parse(input: &str) -> Result<Self, NameError> {
        // `::` first, so a Rust-style path collapses to the same separators
        // as a filesystem-style one before splitting.
        let normalized = input.replace("::", "/").replace('\\', "/");
        let raw: Vec<&str> = normalized
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();

        let Some((last, parents)) = raw.split_last() else {
            return Err(NameError::Empty);
        };

        let mut segments = Vec::with_capacity(parents.len());
        for parent in parents {
            segments.push(to_snake_case(validate(parent)?));
        }

        Ok(Self {
            segments,
            stem: to_snake_case(validate(last)?),
        })
    }

    /// The directory segments the artifact nests under.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// The snake_case file stem, with `suffix` appended when it is not
    /// already there.
    ///
    /// `arc make:controller user` and `arc make:controller UserController`
    /// both land on `user_controller`: a developer who spells the suffix out
    /// should not get `user_controller_controller`.
    #[must_use]
    pub fn file_stem(&self, suffix: &str) -> String {
        let base = self.base(suffix);
        if suffix.is_empty() {
            base.to_string()
        } else {
            format!("{base}_{}", to_snake_case(suffix))
        }
    }

    /// The PascalCase type name, with `suffix` appended when it is not
    /// already there.
    #[must_use]
    pub fn type_name(&self, suffix: &str) -> String {
        format!("{}{}", to_pascal_case(self.base(suffix)), pascal(suffix))
    }

    /// The stem with any already-typed `suffix` removed.
    fn base<'a>(&'a self, suffix: &str) -> &'a str {
        if suffix.is_empty() {
            return &self.stem;
        }
        let snake_suffix = format!("_{}", to_snake_case(suffix));
        match self.stem.strip_suffix(&snake_suffix) {
            // `arc make:controller controller` must not strip itself to
            // nothing, so a stem that is *only* the suffix stays whole.
            Some(rest) if !rest.is_empty() => rest,
            _ => &self.stem,
        }
    }

    /// The full dotted path as the developer typed it, normalized -- the
    /// spelling that goes into a page contract or a command name.
    #[must_use]
    pub fn slash_path(&self) -> String {
        let mut parts = self.segments.clone();
        parts.push(self.stem.clone());
        parts.join("/")
    }
}

impl fmt::Display for ArtifactName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.slash_path())
    }
}

/// Reject anything that is not a plain identifier segment.
fn validate(segment: &str) -> Result<&str, NameError> {
    let first_is_letter = segment
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic());
    let rest_is_clean = segment
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');

    if first_is_letter && rest_is_clean {
        Ok(segment)
    } else {
        Err(NameError::BadSegment {
            segment: segment.to_owned(),
        })
    }
}

/// Convert to `snake_case`, treating `-`, `_`, and case boundaries as breaks.
///
/// Handles the acronym case (`HTTPServer` -> `http_server`) by breaking
/// before the last capital of a run when a lowercase letter follows it.
pub fn to_snake_case(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len() + 4);

    for (index, &current) in chars.iter().enumerate() {
        if current == '-' || current == '_' || current == ' ' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            continue;
        }
        if current.is_ascii_uppercase() && index > 0 {
            let previous = chars[index - 1];
            let starts_word = previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase()
                    && chars.get(index + 1).is_some_and(char::is_ascii_lowercase));
            if starts_word && !out.ends_with('_') {
                out.push('_');
            }
        }
        out.push(current.to_ascii_lowercase());
    }

    out.trim_matches('_').to_string()
}

/// Convert to `PascalCase` by way of snake_case, so every input spelling
/// funnels through one set of word boundaries.
pub fn to_pascal_case(input: &str) -> String {
    pascal(&to_snake_case(input))
}

/// Capitalize each `_`-separated word and join them.
fn pascal(snake: &str) -> String {
    snake
        .split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A naive English pluralizer, used only for a model's default table name.
///
/// It is wrong for irregular nouns, and that is acceptable: the table name
/// lands in a generated `#[model(table = "...")]` attribute the developer
/// reads and edits on the spot. Guessing well for the common case beats
/// pulling in an inflection dependency.
pub fn pluralize(word: &str) -> String {
    const SIBILANT_ENDINGS: [&str; 5] = ["s", "x", "z", "ch", "sh"];

    if let Some(stem) = word.strip_suffix('y')
        && !stem.ends_with(['a', 'e', 'i', 'o', 'u'])
    {
        return format!("{stem}ies");
    }
    if SIBILANT_ENDINGS.iter().any(|ending| word.ends_with(ending)) {
        return format!("{word}es");
    }
    format!("{word}s")
}

/// An error from parsing a generator name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// The name was empty, or only separators.
    Empty,
    /// A path segment was not a plain identifier.
    BadSegment { segment: String },
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str(
                "the name is empty; pass something like `user`, `User`, or `users/show`",
            ),
            Self::BadSegment { segment } => write!(
                formatter,
                "{segment:?} is not a valid name segment; use letters, digits, `-`, and `_`, \
                 starting with a letter, and separate nested names with `/`"
            ),
        }
    }
}

impl std::error::Error for NameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowercase_name_and_its_pascal_spelling_normalize_alike() {
        let lower = ArtifactName::parse("user").expect("valid");
        let pascal = ArtifactName::parse("User").expect("valid");
        assert_eq!(lower, pascal);
        assert_eq!(lower.file_stem("Controller"), "user_controller");
        assert_eq!(lower.type_name("Controller"), "UserController");
    }

    #[test]
    fn a_nested_name_splits_into_directories_and_a_stem() {
        let name = ArtifactName::parse("users/show").expect("valid");
        assert_eq!(name.segments(), ["users"]);
        assert_eq!(name.file_stem("Controller"), "show_controller");
        assert_eq!(name.type_name("Controller"), "ShowController");
        assert_eq!(name.slash_path(), "users/show");
    }

    #[test]
    fn backslashes_and_rust_paths_are_the_same_separator() {
        let expected = ArtifactName::parse("admin/users/show").expect("valid");
        assert_eq!(
            ArtifactName::parse("admin\\users\\show").expect("valid"),
            expected
        );
        assert_eq!(
            ArtifactName::parse("admin::users::show").expect("valid"),
            expected
        );
    }

    #[test]
    fn a_name_that_already_carries_its_suffix_is_not_doubled() {
        let name = ArtifactName::parse("UserController").expect("valid");
        assert_eq!(name.file_stem("Controller"), "user_controller");
        assert_eq!(name.type_name("Controller"), "UserController");
    }

    #[test]
    fn a_name_that_is_only_the_suffix_keeps_itself() {
        let name = ArtifactName::parse("controller").expect("valid");
        assert_eq!(name.type_name("Controller"), "ControllerController");
    }

    #[test]
    fn an_empty_suffix_leaves_the_name_alone() {
        let name = ArtifactName::parse("SendWelcomeEmail").expect("valid");
        assert_eq!(name.file_stem(""), "send_welcome_email");
        assert_eq!(name.type_name(""), "SendWelcomeEmail");
    }

    #[test]
    fn a_name_with_path_traversal_is_refused() {
        assert!(matches!(
            ArtifactName::parse("../../etc/passwd"),
            Err(NameError::BadSegment { .. })
        ));
        assert!(matches!(
            ArtifactName::parse("users/../secrets"),
            Err(NameError::BadSegment { .. })
        ));
    }

    #[test]
    fn an_empty_name_is_refused() {
        assert_eq!(ArtifactName::parse(""), Err(NameError::Empty));
        assert_eq!(ArtifactName::parse("///"), Err(NameError::Empty));
    }

    #[test]
    fn a_segment_that_starts_with_a_digit_is_refused() {
        assert!(ArtifactName::parse("2fa").is_err());
    }

    #[test]
    fn an_acronym_run_breaks_before_the_word_it_starts() {
        assert_eq!(to_snake_case("HTTPServer"), "http_server");
        assert_eq!(to_snake_case("OAuthToken"), "o_auth_token");
        assert_eq!(to_snake_case("send-welcome-email"), "send_welcome_email");
        assert_eq!(to_pascal_case("send_welcome_email"), "SendWelcomeEmail");
    }

    #[test]
    fn the_pluralizer_covers_the_common_english_endings() {
        assert_eq!(pluralize("user"), "users");
        assert_eq!(pluralize("category"), "categories");
        assert_eq!(pluralize("day"), "days");
        assert_eq!(pluralize("address"), "addresses");
        assert_eq!(pluralize("box"), "boxes");
        assert_eq!(pluralize("match"), "matches");
    }
}
