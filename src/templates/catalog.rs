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
        // --- Root manifests ---
        TemplateFile {
            path: "Cargo.toml",
            content: include_str!("files/Cargo.toml"),
        },
        // --- Rust entry points ---
        TemplateFile {
            path: "src/main.rs",
            content: include_str!("files/src/main.rs"),
        },
        TemplateFile {
            path: "src/lib.rs",
            content: include_str!("files/src/lib.rs"),
        },
        TemplateFile {
            path: "src/routes/mod.rs",
            content: include_str!("files/src/routes/mod.rs"),
        },
        // --- app/ (backend application layer) ---
        TemplateFile {
            path: "app/mod.rs",
            content: include_str!("files/app/mod.rs"),
        },
        TemplateFile {
            path: "app/controllers/mod.rs",
            content: include_str!("files/app/controllers/mod.rs"),
        },
        TemplateFile {
            path: "app/controllers/home_controller.rs",
            content: include_str!("files/app/controllers/home_controller.rs"),
        },
        TemplateFile {
            path: "app/models/mod.rs",
            content: include_str!("files/app/models/mod.rs"),
        },
        TemplateFile {
            path: "app/models/user.rs",
            content: include_str!("files/app/models/user.rs"),
        },
        TemplateFile {
            path: "app/services/mod.rs",
            content: include_str!("files/app/services/mod.rs"),
        },
        TemplateFile {
            path: "app/services/user_service.rs",
            content: include_str!("files/app/services/user_service.rs"),
        },
        TemplateFile {
            path: "app/requests/mod.rs",
            content: include_str!("files/app/requests/mod.rs"),
        },
        TemplateFile {
            path: "app/requests/create_user_request.rs",
            content: include_str!("files/app/requests/create_user_request.rs"),
        },
        TemplateFile {
            path: "app/requests/update_user_request.rs",
            content: include_str!("files/app/requests/update_user_request.rs"),
        },
        TemplateFile {
            path: "app/policies/mod.rs",
            content: include_str!("files/app/policies/mod.rs"),
        },
        TemplateFile {
            path: "app/policies/user_policy.rs",
            content: include_str!("files/app/policies/user_policy.rs"),
        },
        TemplateFile {
            path: "app/resources/mod.rs",
            content: include_str!("files/app/resources/mod.rs"),
        },
        TemplateFile {
            path: "app/resources/user_resource.rs",
            content: include_str!("files/app/resources/user_resource.rs"),
        },
        // --- resources/ (frontend) ---
        TemplateFile {
            path: "resources/js/app.tsx",
            content: include_str!("files/resources/js/app.tsx"),
        },
        TemplateFile {
            path: "resources/js/pages/home.tsx",
            content: include_str!("files/resources/js/pages/home.tsx"),
        },
        TemplateFile {
            path: "resources/js/layouts/default.tsx",
            content: include_str!("files/resources/js/layouts/default.tsx"),
        },
        TemplateFile {
            path: "resources/js/components/.gitkeep",
            content: include_str!("files/resources/js/components/.gitkeep"),
        },
        TemplateFile {
            path: "resources/css/app.css",
            content: include_str!("files/resources/css/app.css"),
        },
    ]
}
