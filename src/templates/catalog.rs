//! The template file catalog.
//!
//! Every template file is embedded with `include_str!` and tagged with the
//! path it lands on inside the generated project. Embedding rather than
//! reading from disk is what makes `arc new` work from an installed binary
//! with no copy of the framework source anywhere on the machine.
//!
//! The tree under `files/` is split three ways:
//!
//! - `files/shared/` -- everything independent of the two choices.
//! - `files/{react,vue,svelte}/` -- the frontend, one directory per stack.
//! - `files/db/{postgres,sqlite,mysql}/` -- the development services.
//!
//! The three lists are concatenated, so a stack directory and the shared
//! directory must never claim the same output path.

/// The frontend framework a generated application is scaffolded with.
///
/// This mirrors the CLI's `--stack` flag. It is declared here rather than
/// reused from `crate::cli` because the `templates` feature is usable without
/// `cli` -- a build script or a test harness can generate a project without
/// pulling in an argument parser.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Stack {
    /// React 19 with `@inertiajs/react`.
    #[default]
    React,
    /// Vue 3 with `@inertiajs/vue3`.
    Vue,
    /// Svelte 5 with `@inertiajs/svelte`.
    Svelte,
}

impl Stack {
    /// Every stack, in the order the CLI lists them.
    pub const ALL: [Self; 3] = [Self::React, Self::Vue, Self::Svelte];

    /// The lowercase name used on the command line and for `__STACK__`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::React => "react",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
        }
    }

    /// The Vite entry module, relative to the project root.
    ///
    /// Only React needs a `.tsx` entry; Vue and Svelte keep their templates
    /// in single-file components and bootstrap from plain TypeScript.
    #[must_use]
    pub const fn entry(self) -> &'static str {
        match self {
            Self::React => "resources/js/app.tsx",
            Self::Vue | Self::Svelte => "resources/js/app.ts",
        }
    }
}

/// The database driver a generated application is scaffolded against.
///
/// Declared here for the same reason as [`Stack`]: `templates` must not
/// depend on `cli`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Database {
    /// PostgreSQL 17.
    #[default]
    Postgres,
    /// SQLite, as a file under `storage/`.
    Sqlite,
    /// MySQL 8.4.
    Mysql,
}

impl Database {
    /// Every driver, in the order the CLI lists them.
    pub const ALL: [Self; 3] = [Self::Postgres, Self::Sqlite, Self::Mysql];

    /// The lowercase name used on the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
        }
    }

    /// The Arcature cargo feature this driver needs, substituted for
    /// `__DB_DRIVER__` in the generated `Cargo.toml`.
    #[must_use]
    pub const fn feature(self) -> &'static str {
        match self {
            Self::Postgres => "db-postgres",
            Self::Sqlite => "db-sqlite",
            Self::Mysql => "db-mysql",
        }
    }

    /// The `DATABASE_URL` written into the generated `.env`.
    ///
    /// Still contains `__RUST_NAME__`; the renderer expands this token before
    /// the project name so the default database is named after the crate.
    #[must_use]
    pub const fn default_url(self) -> &'static str {
        match self {
            Self::Postgres => "postgres://postgres:postgres@127.0.0.1:5432/__RUST_NAME__",
            Self::Sqlite => "sqlite://storage/__RUST_NAME__.sqlite?mode=rwc",
            Self::Mysql => "mysql://__RUST_NAME__:secret@127.0.0.1:3306/__RUST_NAME__",
        }
    }
}

/// A template file: the path it lands on inside the generated project, and
/// the embedded content to render into it.
pub struct TemplateFile {
    /// Destination path, relative to the project root, `/`-separated.
    pub path: &'static str,
    /// The unrendered file content.
    pub content: &'static str,
}

/// Every file a `stack` + `database` project is made of, shared files first.
#[must_use]
pub fn files(stack: Stack, database: Database) -> Vec<TemplateFile> {
    let mut all = shared();
    all.extend(match stack {
        Stack::React => react(),
        Stack::Vue => vue(),
        Stack::Svelte => svelte(),
    });
    all.extend(match database {
        Database::Postgres => postgres(),
        Database::Sqlite => sqlite(),
        Database::Mysql => mysql(),
    });
    all
}

/// Files every generated application gets, whatever the stack or driver.
fn shared() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: ".cargo/config.toml",
            content: include_str!("files/shared/.cargo/config.toml"),
        },
        TemplateFile {
            path: ".dockerignore",
            content: include_str!("files/shared/.dockerignore"),
        },
        TemplateFile {
            path: ".env",
            content: include_str!("files/shared/.env"),
        },
        TemplateFile {
            path: ".env.example",
            content: include_str!("files/shared/.env.example"),
        },
        TemplateFile {
            path: ".gitignore",
            content: include_str!("files/shared/.gitignore"),
        },
        TemplateFile {
            path: "Cargo.toml",
            content: include_str!("files/shared/Cargo.toml"),
        },
        TemplateFile {
            path: "Dockerfile",
            content: include_str!("files/shared/Dockerfile"),
        },
        TemplateFile {
            path: "app/controllers/home_controller.rs",
            content: include_str!("files/shared/app/controllers/home_controller.rs"),
        },
        TemplateFile {
            path: "app/controllers/mod.rs",
            content: include_str!("files/shared/app/controllers/mod.rs"),
        },
        TemplateFile {
            path: "app/mod.rs",
            content: include_str!("files/shared/app/mod.rs"),
        },
        TemplateFile {
            path: "app/models/mod.rs",
            content: include_str!("files/shared/app/models/mod.rs"),
        },
        TemplateFile {
            path: "app/pages/errors.rs",
            content: include_str!("files/shared/app/pages/errors.rs"),
        },
        TemplateFile {
            path: "app/pages/home.rs",
            content: include_str!("files/shared/app/pages/home.rs"),
        },
        TemplateFile {
            path: "app/pages/mod.rs",
            content: include_str!("files/shared/app/pages/mod.rs"),
        },
        TemplateFile {
            path: "app/policies/mod.rs",
            content: include_str!("files/shared/app/policies/mod.rs"),
        },
        TemplateFile {
            path: "app/requests/mod.rs",
            content: include_str!("files/shared/app/requests/mod.rs"),
        },
        TemplateFile {
            path: "app/resources/mod.rs",
            content: include_str!("files/shared/app/resources/mod.rs"),
        },
        TemplateFile {
            path: "app/services/mod.rs",
            content: include_str!("files/shared/app/services/mod.rs"),
        },
        TemplateFile {
            path: "bootstrap/app.rs",
            content: include_str!("files/shared/bootstrap/app.rs"),
        },
        TemplateFile {
            path: "bootstrap/error_pages.rs",
            content: include_str!("files/shared/bootstrap/error_pages.rs"),
        },
        TemplateFile {
            path: "bootstrap/mod.rs",
            content: include_str!("files/shared/bootstrap/mod.rs"),
        },
        TemplateFile {
            path: "bootstrap/state.rs",
            content: include_str!("files/shared/bootstrap/state.rs"),
        },
        TemplateFile {
            path: "config/mod.rs",
            content: include_str!("files/shared/config/mod.rs"),
        },
        TemplateFile {
            path: "database/migrations/mod.rs",
            content: include_str!("files/shared/database/migrations/mod.rs"),
        },
        TemplateFile {
            path: "database/mod.rs",
            content: include_str!("files/shared/database/mod.rs"),
        },
        TemplateFile {
            path: "database/seeders/mod.rs",
            content: include_str!("files/shared/database/seeders/mod.rs"),
        },
        TemplateFile {
            path: "justfile",
            content: include_str!("files/shared/justfile"),
        },
        TemplateFile {
            path: "public/.gitkeep",
            content: include_str!("files/shared/public/.gitkeep"),
        },
        TemplateFile {
            path: "public/robots.txt",
            content: include_str!("files/shared/public/robots.txt"),
        },
        TemplateFile {
            path: "resources/css/app.css",
            content: include_str!("files/shared/resources/css/app.css"),
        },
        TemplateFile {
            path: "routes/mod.rs",
            content: include_str!("files/shared/routes/mod.rs"),
        },
        TemplateFile {
            path: "src/bin/uag.rs",
            content: include_str!("files/shared/src/bin/uag.rs"),
        },
        TemplateFile {
            path: "src/lib.rs",
            content: include_str!("files/shared/src/lib.rs"),
        },
        TemplateFile {
            path: "src/main.rs",
            content: include_str!("files/shared/src/main.rs"),
        },
        TemplateFile {
            path: "storage/framework/.gitkeep",
            content: include_str!("files/shared/storage/framework/.gitkeep"),
        },
        TemplateFile {
            path: "storage/logs/.gitkeep",
            content: include_str!("files/shared/storage/logs/.gitkeep"),
        },
        TemplateFile {
            path: "storage/uploads/.gitkeep",
            content: include_str!("files/shared/storage/uploads/.gitkeep"),
        },
        TemplateFile {
            path: "tests/smoke.rs",
            content: include_str!("files/shared/tests/smoke.rs"),
        },
    ]
}

/// The React frontend: entry module, config, pages, layout.
fn react() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "package.json",
            content: include_str!("files/react/package.json"),
        },
        TemplateFile {
            path: "resources/js/app.tsx",
            content: include_str!("files/react/resources/js/app.tsx"),
        },
        TemplateFile {
            path: "resources/js/components/.gitkeep",
            content: include_str!("files/react/resources/js/components/.gitkeep"),
        },
        TemplateFile {
            path: "resources/js/layouts/default.tsx",
            content: include_str!("files/react/resources/js/layouts/default.tsx"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/404.tsx",
            content: include_str!("files/react/resources/js/pages/errors/404.tsx"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/419.tsx",
            content: include_str!("files/react/resources/js/pages/errors/419.tsx"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/500.tsx",
            content: include_str!("files/react/resources/js/pages/errors/500.tsx"),
        },
        TemplateFile {
            path: "resources/js/pages/home.tsx",
            content: include_str!("files/react/resources/js/pages/home.tsx"),
        },
        TemplateFile {
            path: "resources/js/types.ts",
            content: include_str!("files/react/resources/js/types.ts"),
        },
        TemplateFile {
            path: "tsconfig.json",
            content: include_str!("files/react/tsconfig.json"),
        },
        TemplateFile {
            path: "vite.config.ts",
            content: include_str!("files/react/vite.config.ts"),
        },
    ]
}

/// The Vue frontend: entry module, config, pages, layout.
fn vue() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "package.json",
            content: include_str!("files/vue/package.json"),
        },
        TemplateFile {
            path: "resources/js/app.ts",
            content: include_str!("files/vue/resources/js/app.ts"),
        },
        TemplateFile {
            path: "resources/js/components/.gitkeep",
            content: include_str!("files/vue/resources/js/components/.gitkeep"),
        },
        TemplateFile {
            path: "resources/js/layouts/Default.vue",
            content: include_str!("files/vue/resources/js/layouts/Default.vue"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/404.vue",
            content: include_str!("files/vue/resources/js/pages/errors/404.vue"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/419.vue",
            content: include_str!("files/vue/resources/js/pages/errors/419.vue"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/500.vue",
            content: include_str!("files/vue/resources/js/pages/errors/500.vue"),
        },
        TemplateFile {
            path: "resources/js/pages/home.vue",
            content: include_str!("files/vue/resources/js/pages/home.vue"),
        },
        TemplateFile {
            path: "resources/js/types.ts",
            content: include_str!("files/vue/resources/js/types.ts"),
        },
        TemplateFile {
            path: "tsconfig.json",
            content: include_str!("files/vue/tsconfig.json"),
        },
        TemplateFile {
            path: "vite.config.ts",
            content: include_str!("files/vue/vite.config.ts"),
        },
    ]
}

/// The Svelte frontend: entry module, config, pages, layout.
fn svelte() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "package.json",
            content: include_str!("files/svelte/package.json"),
        },
        TemplateFile {
            path: "resources/js/app.ts",
            content: include_str!("files/svelte/resources/js/app.ts"),
        },
        TemplateFile {
            path: "resources/js/components/.gitkeep",
            content: include_str!("files/svelte/resources/js/components/.gitkeep"),
        },
        TemplateFile {
            path: "resources/js/layouts/Default.svelte",
            content: include_str!("files/svelte/resources/js/layouts/Default.svelte"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/404.svelte",
            content: include_str!("files/svelte/resources/js/pages/errors/404.svelte"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/419.svelte",
            content: include_str!("files/svelte/resources/js/pages/errors/419.svelte"),
        },
        TemplateFile {
            path: "resources/js/pages/errors/500.svelte",
            content: include_str!("files/svelte/resources/js/pages/errors/500.svelte"),
        },
        TemplateFile {
            path: "resources/js/pages/home.svelte",
            content: include_str!("files/svelte/resources/js/pages/home.svelte"),
        },
        TemplateFile {
            path: "resources/js/types.ts",
            content: include_str!("files/svelte/resources/js/types.ts"),
        },
        TemplateFile {
            path: "svelte.config.js",
            content: include_str!("files/svelte/svelte.config.js"),
        },
        TemplateFile {
            path: "tsconfig.json",
            content: include_str!("files/svelte/tsconfig.json"),
        },
        TemplateFile {
            path: "vite.config.ts",
            content: include_str!("files/svelte/vite.config.ts"),
        },
    ]
}

/// Development services for the postgres driver.
fn postgres() -> Vec<TemplateFile> {
    vec![TemplateFile {
        path: "docker-compose.yml",
        content: include_str!("files/db/postgres/docker-compose.yml"),
    }]
}

/// Development services for the sqlite driver.
fn sqlite() -> Vec<TemplateFile> {
    vec![TemplateFile {
        path: "docker-compose.yml",
        content: include_str!("files/db/sqlite/docker-compose.yml"),
    }]
}

/// Development services for the mysql driver.
fn mysql() -> Vec<TemplateFile> {
    vec![TemplateFile {
        path: "docker-compose.yml",
        content: include_str!("files/db/mysql/docker-compose.yml"),
    }]
}
