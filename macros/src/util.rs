//! Shared helpers for the Arcature proc-macros.
//!
//! One responsibility: cross-macro utilities (the snake-case conversion used
//! by `#[derive(Job)]` to default a job kind from its struct name). Kept apart
//! from the individual macro files so each macro file holds only its own
//! expansion.

/// Convert a PascalCase name to snake_case.
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 8);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_ascii_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert a snake_case name to PascalCase.
///
/// Used by `#[middleware]` to derive the generated middleware type's name
/// from the annotated function's name (`require_auth` -> `RequireAuth`).
pub fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize = true;
    for ch in s.chars() {
        if ch == '_' {
            capitalize = true;
        } else if capitalize {
            result.extend(ch.to_uppercase());
            capitalize = false;
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{to_pascal_case, to_snake_case};

    #[test]
    fn snake_case_splits_on_capitals() {
        assert_eq!(
            to_snake_case("SendVerificationEmail"),
            "send_verification_email"
        );
        assert_eq!(to_snake_case("User"), "user");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
    }

    #[test]
    fn pascal_case_capitalizes_each_word() {
        assert_eq!(to_pascal_case("require_auth"), "RequireAuth");
        assert_eq!(to_pascal_case("auth"), "Auth");
    }

    #[test]
    fn pascal_case_leaves_an_already_pascal_name_alone() {
        assert_eq!(to_pascal_case("RequireAuth"), "RequireAuth");
    }
}
