//! Tests for the template generator.

#![cfg(feature = "templates")]

use arcature::templates::{ProjectName, TemplateError, generate};

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
    // dotfiles
    assert!(target.join(".env").exists());
    assert!(target.join(".env.example").exists());
    assert!(target.join(".gitignore").exists());
    // Rust entry points
    assert!(target.join("src/main.rs").exists());
    assert!(target.join("src/lib.rs").exists());
    // bootstrap/ structure
    assert!(target.join("bootstrap/mod.rs").exists());
    assert!(target.join("bootstrap/app.rs").exists());
    assert!(target.join("bootstrap/state.rs").exists());
    // config/ structure
    assert!(target.join("config/mod.rs").exists());
    // database/ structure
    assert!(target.join("database/mod.rs").exists());
    assert!(target.join("database/migrations/mod.rs").exists());
    // routes/ structure
    assert!(target.join("routes/mod.rs").exists());
    // app/ structure (folders + welcome page; business code is user-supplied)
    assert!(target.join("app/mod.rs").exists());
    assert!(target.join("app/controllers/mod.rs").exists());
    assert!(target.join("app/controllers/home_controller.rs").exists());
    assert!(target.join("app/models/mod.rs").exists());
    assert!(target.join("app/services/mod.rs").exists());
    assert!(target.join("app/requests/mod.rs").exists());
    assert!(target.join("app/policies/mod.rs").exists());
    assert!(target.join("app/resources/mod.rs").exists());
    // resources/ structure (frontend)
    assert!(target.join("resources/js/app.tsx").exists());
    assert!(target.join("resources/js/pages/home.tsx").exists());
    assert!(target.join("resources/js/layouts/default.tsx").exists());
    assert!(target.join("resources/js/components/.gitkeep").exists());
    assert!(target.join("resources/css/app.css").exists());
    // public/ structure
    assert!(target.join("public/robots.txt").exists());
    assert!(target.join("public/.gitkeep").exists());
    // storage/ structure
    assert!(target.join("storage/logs/.gitkeep").exists());
    assert!(target.join("storage/uploads/.gitkeep").exists());
    assert!(target.join("storage/framework/.gitkeep").exists());
    // tests/ structure
    assert!(target.join("tests/smoke.rs").exists());
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
