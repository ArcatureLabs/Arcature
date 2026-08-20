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
        // --- Dotfiles (env + git) ---
        TemplateFile {
            path: ".env",
            content: include_str!("files/.env"),
        },
        TemplateFile {
            path: ".env.example",
            content: include_str!("files/.env.example"),
        },
        TemplateFile {
            path: ".gitignore",
            content: include_str!("files/.gitignore"),
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
        // --- bootstrap/ (compose the Application) ---
        TemplateFile {
            path: "bootstrap/mod.rs",
            content: include_str!("files/bootstrap/mod.rs"),
        },
        TemplateFile {
            path: "bootstrap/app.rs",
            content: include_str!("files/bootstrap/app.rs"),
        },
        TemplateFile {
            path: "bootstrap/state.rs",
            content: include_str!("files/bootstrap/state.rs"),
        },
        // --- config/ (typed Config from env) ---
        TemplateFile {
            path: "config/mod.rs",
            content: include_str!("files/config/mod.rs"),
        },
        // --- database/ (SeaORM migrations) ---
        TemplateFile {
            path: "database/mod.rs",
            content: include_str!("files/database/mod.rs"),
        },
        TemplateFile {
            path: "database/migrations/mod.rs",
            content: include_str!("files/database/migrations/mod.rs"),
        },
        // --- routes/ (route registration) ---
        TemplateFile {
            path: "routes/mod.rs",
            content: include_str!("files/routes/mod.rs"),
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
            path: "app/services/mod.rs",
            content: include_str!("files/app/services/mod.rs"),
        },
        TemplateFile {
            path: "app/requests/mod.rs",
            content: include_str!("files/app/requests/mod.rs"),
        },
        TemplateFile {
            path: "app/policies/mod.rs",
            content: include_str!("files/app/policies/mod.rs"),
        },
        TemplateFile {
            path: "app/resources/mod.rs",
            content: include_str!("files/app/resources/mod.rs"),
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
        // --- public/ (static assets) ---
        TemplateFile {
            path: "public/robots.txt",
            content: include_str!("files/public/robots.txt"),
        },
        TemplateFile {
            path: "public/.gitkeep",
            content: include_str!("files/public/.gitkeep"),
        },
        // --- storage/ (runtime artifacts) ---
        TemplateFile {
            path: "storage/logs/.gitkeep",
            content: include_str!("files/storage/logs/.gitkeep"),
        },
        TemplateFile {
            path: "storage/uploads/.gitkeep",
            content: include_str!("files/storage/uploads/.gitkeep"),
        },
        TemplateFile {
            path: "storage/framework/.gitkeep",
            content: include_str!("files/storage/framework/.gitkeep"),
        },
        // --- tests/ (integration tests) ---
        TemplateFile {
            path: "tests/smoke.rs",
            content: include_str!("files/tests/smoke.rs"),
        },
    ]
}
