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
