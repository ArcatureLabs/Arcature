//! The template file catalog. Each file is embedded with `include_str!` and
//! tagged with its relative output path.

/// A template file: its destination path (relative to the project root) and
/// its content.
pub struct TemplateFile {
    pub path: &'static str,
    pub content: &'static str,
}

/// All template files, in order.
pub fn files() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "Cargo.toml",
            content: include_str!("files/Cargo.toml"),
        },
        TemplateFile {
            path: "src/main.rs",
            content: include_str!("files/src/main.rs"),
        },
        TemplateFile {
            path: "src/lib.rs",
            content: include_str!("files/src/lib.rs"),
        },
        TemplateFile {
            path: "src/controllers/mod.rs",
            content: include_str!("files/src/controllers/mod.rs"),
        },
        TemplateFile {
            path: "src/controllers/home.rs",
            content: include_str!("files/src/controllers/home.rs"),
        },
        TemplateFile {
            path: "src/pages/mod.rs",
            content: include_str!("files/src/pages/mod.rs"),
        },
        TemplateFile {
            path: "src/pages/home.rs",
            content: include_str!("files/src/pages/home.rs"),
        },
        TemplateFile {
            path: "src/routes/mod.rs",
            content: include_str!("files/src/routes/mod.rs"),
        },
    ]
}
