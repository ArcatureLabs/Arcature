use super::error::TemplateError;
use super::name::ProjectName;

/// Render a template by substituting the placeholder tokens.
pub fn render(content: &str, name: &ProjectName, arcature_version: &str) -> String {
    let rust_name = name.rust_identifier();
    let project_name = name.raw();
    content
        .replace("__RUST_NAME__", &rust_name)
        .replace("__PROJECT_NAME__", project_name)
        .replace("__ARCATURE_VERSION__", arcature_version)
}
