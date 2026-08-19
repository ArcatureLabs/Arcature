//! Tests for the template generator.

#![cfg(feature = "templates")]

use arcature::templates::{generate, ProjectName, TemplateError};

#[test]
fn project_name_parse_valid() {
    let name = ProjectName::parse("my-app").unwrap();
    assert_eq!(name.raw(), "my-app");
    assert_eq!(name.rust_identifier(), "my_app");
}

#[test]
fn project_name_parse_simple() {
    let name = ProjectName::parse("blog").unwrap();
    assert_eq!(name.raw(), "blog");
    assert_eq!(name.rust_identifier(), "blog");
}

#[test]
fn project_name_rejects_empty() {
    assert!(matches!(
        ProjectName::parse(""),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn project_name_rejects_uppercase_start() {
    assert!(matches!(
        ProjectName::parse("MyApp"),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn project_name_rejects_trailing_hyphen() {
    assert!(matches!(
        ProjectName::parse("my-app-"),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn project_name_rejects_consecutive_hyphens() {
    assert!(matches!(
        ProjectName::parse("my--app"),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn project_name_rejects_too_long() {
    assert!(matches!(
        ProjectName::parse(&"a".repeat(65)),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn generate_creates_a_project_tree() {
    let temp = tempfile_dir();
    let target = temp.join("test-app");

    generate(&target).unwrap();

    assert!(target.exists());
    assert!(target.join("Cargo.toml").exists());
    assert!(target.join("src/main.rs").exists());
    assert!(target.join("src/lib.rs").exists());
    assert!(target.join("src/controllers/mod.rs").exists());
    assert!(target.join("src/pages/mod.rs").exists());
    assert!(target.join("src/routes/mod.rs").exists());
}

#[test]
fn generate_rejects_existing_target() {
    let temp = tempfile_dir();
    let target = temp.join("existing-app");
    std::fs::create_dir_all(&target).unwrap();

    assert!(matches!(
        generate(&target),
        Err(TemplateError::ExistingTarget { .. })
    ));
}

#[test]
fn generated_cargo_toml_has_substituted_name() {
    let temp = tempfile_dir();
    let target = temp.join("sub-app");
    generate(&target).unwrap();

    let cargo_toml = std::fs::read_to_string(target.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("name = \"sub_app\""));
    // The placeholder should be gone.
    assert!(!cargo_toml.contains("__RUST_NAME__"));
    assert!(!cargo_toml.contains("__ARCATURE_VERSION__"));
}

#[test]
fn generated_main_rs_has_substituted_name() {
    let temp = tempfile_dir();
    let target = temp.join("gen-app");
    generate(&target).unwrap();

    let main_rs = std::fs::read_to_string(target.join("src/main.rs")).unwrap();
    assert!(main_rs.contains("gen_app::run()"));
    assert!(!main_rs.contains("__RUST_NAME__"));
}

/// Create a unique temporary directory for a test.
fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "arcature-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
