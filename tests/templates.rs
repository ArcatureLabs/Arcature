//! What `arc new` writes to disk.
//!
//! These tests generate real project trees into a temporary directory. They
//! do not compile them -- a generated project depends on the published
//! `arcature` crate, and building one from inside this repository would
//! resolve against crates.io rather than the working tree.

#![cfg(feature = "templates")]

use std::path::{Path, PathBuf};

use arcature::templates::{Database, ProjectName, Stack, TemplateError, generate};

/// A temporary directory that removes itself, so a failing assertion does not
/// leave a project tree behind in the system temp directory.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "arcature-templates-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("scratch");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn read(root: &Path, relative: &str) -> String {
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// Walk a generated tree and hand back every file's path and contents.
fn walk(root: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("prefix")
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/");
                let contents = std::fs::read_to_string(&path).unwrap_or_default();
                found.push((relative, contents));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn a_project_name_keeps_its_hyphens_and_gains_an_underscored_crate_name() {
    let name = ProjectName::parse("my-app").expect("valid");
    assert_eq!(name.raw(), "my-app");
    assert_eq!(name.rust_identifier(), "my_app");
}

#[test]
fn a_name_that_would_not_survive_both_ecosystems_is_refused() {
    for bad in ["", "MyApp", "my-app-", "my--app", "1app", "my_app"] {
        assert!(
            matches!(
                ProjectName::parse(bad),
                Err(TemplateError::InvalidName { .. })
            ),
            "{bad} should be refused"
        );
    }
    assert!(matches!(
        ProjectName::parse(&"a".repeat(65)),
        Err(TemplateError::InvalidName { .. })
    ));
}

#[test]
fn every_stack_and_driver_combination_generates_a_coherent_tree() {
    let scratch = Scratch::new("matrix");
    for stack in Stack::ALL {
        for database in Database::ALL {
            let label = format!("{}-{}", stack.as_str(), database.as_str());
            let target = scratch.join(&label);
            generate(&target, stack, database).unwrap_or_else(|e| panic!("{label}: {e}"));

            for required in [
                "Cargo.toml",
                ".cargo/config.toml",
                ".env",
                ".env.example",
                ".gitignore",
                "Dockerfile",
                "docker-compose.yml",
                "justfile",
                "package.json",
                "tsconfig.json",
                "vite.config.ts",
                "src/main.rs",
                "src/lib.rs",
                "bootstrap/mod.rs",
                "bootstrap/app.rs",
                "bootstrap/state.rs",
                "bootstrap/error_pages.rs",
                "config/mod.rs",
                "database/mod.rs",
                "database/migrations/mod.rs",
                "database/seeders/mod.rs",
                "routes/mod.rs",
                "app/mod.rs",
                "resources/css/app.css",
                "public/robots.txt",
                "tests/smoke.rs",
            ] {
                assert!(
                    target.join(required).is_file(),
                    "{label} is missing {required}"
                );
            }
            assert!(
                target.join(stack.entry()).is_file(),
                "{label} is missing its Vite entry {}",
                stack.entry()
            );
        }
    }
}

#[test]
fn no_placeholder_token_survives_into_a_generated_project() {
    let scratch = Scratch::new("tokens");
    for stack in Stack::ALL {
        for database in Database::ALL {
            let label = format!("{}-{}", stack.as_str(), database.as_str());
            let target = scratch.join(&label);
            generate(&target, stack, database).expect("generated");
            for (path, contents) in walk(&target) {
                for token in [
                    "__RUST_NAME__",
                    "__PROJECT_NAME__",
                    "__ARCATURE_VERSION__",
                    "__STACK__",
                    "__DB_DRIVER__",
                    "__DATABASE_URL__",
                    "__JS_ENTRY__",
                ] {
                    assert!(
                        !contents.contains(token),
                        "{label}/{path} still contains {token}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_generated_project_depends_on_no_arcature_npm_package() {
    let scratch = Scratch::new("npm");
    for stack in Stack::ALL {
        let target = scratch.join(stack.as_str());
        generate(&target, stack, Database::default()).expect("generated");
        for (path, contents) in walk(&target) {
            assert!(
                !contents.contains("@arcature/"),
                "{}/{path} references an @arcature/ npm package; \
                 the framework publishes none",
                stack.as_str()
            );
        }
        let manifest = read(&target, "package.json");
        let adapter = match stack {
            Stack::React => "@inertiajs/react",
            Stack::Vue => "@inertiajs/vue3",
            Stack::Svelte => "@inertiajs/svelte",
        };
        assert!(manifest.contains(adapter), "{manifest}");
    }
}

#[test]
fn the_vite_config_binds_no_tcp_port_of_its_own() {
    let scratch = Scratch::new("oneport");
    for stack in Stack::ALL {
        let target = scratch.join(stack.as_str());
        generate(&target, stack, Database::default()).expect("generated");
        // Comments talk about ports at length; only the code may not.
        let config = strip_line_comments(&read(&target, "vite.config.ts"));
        // `export` and `import` both contain the substring `port`, so the
        // check is on the keys a second port would need, not the word.
        for banned in ["server", "strictPort", "port:"] {
            assert!(
                !config.contains(banned),
                "{} vite.config.ts declares `{banned}` in code; Rust owns the only TCP port",
                stack.as_str()
            );
        }
    }
}

/// Drop `//` line comments so a test can look at the code alone, joining what
/// is left with spaces. Good enough for the Vite configs, which contain no
/// string literal holding a `//`.
fn strip_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_manifest_carries_the_driver_that_was_asked_for_and_no_cli_features() {
    let scratch = Scratch::new("manifest");
    for database in Database::ALL {
        let target = scratch.join(database.as_str());
        generate(&target, Stack::default(), database).expect("generated");
        let manifest = read(&target, "Cargo.toml");
        assert!(manifest.contains(&format!("\"{}\"", database.feature())));
        for other in Database::ALL {
            if other != database {
                assert!(
                    !manifest.contains(&format!("\"{}\"", other.feature())),
                    "{manifest}"
                );
            }
        }
        assert!(manifest.contains("\"dx\""), "{manifest}");
        assert!(!manifest.contains("\"cli\""), "{manifest}");
        assert!(!manifest.contains("\"templates\""), "{manifest}");
        assert!(
            manifest.contains("dev = [\"arcature/dev-proxy\", \"arcature/uag\"]"),
            "{manifest}"
        );
        // The graph dumper must not be linked by the dev loop. `dev` turns on
        // the framework's `uag` feature, which is what makes the endpoint
        // exist; the *binary* is held back by its own `required-features`, so
        // `cargo build --features dev` never pays for the extra link.
        assert!(
            manifest.contains("required-features = [\"uag\"]"),
            "{manifest}"
        );
        assert!(manifest.contains("uag = [\"arcature/uag\"]"), "{manifest}");
        // Two `[[bin]]` targets make a bare `cargo run` ambiguous unless one
        // of them is named as the default.
        assert!(
            manifest.contains(&format!("default-run = \"{}\"", database.as_str())),
            "{manifest}"
        );
        assert!(
            manifest.contains("debug = \"line-tables-only\""),
            "{manifest}"
        );
        assert!(manifest.contains("codegen-units = 256"), "{manifest}");
        assert!(
            manifest.contains("[profile.dev.package.\"*\"]"),
            "{manifest}"
        );
    }
}

/// The production build is the one nobody watches. `cargo build --release` in
/// a generated project must carry neither the CLI nor the dev proxy, and both
/// properties rest on a single line: `default-features = false`. Without it
/// every framework default comes back, and the absence-of-string assertions
/// in the test above pass while the features they name are enabled.
#[test]
fn a_release_build_of_a_generated_project_enables_no_cli_and_no_dev_proxy() {
    let scratch = Scratch::new("release");
    for database in Database::ALL {
        let target = scratch.join(database.as_str());
        generate(&target, Stack::default(), database).expect("generated");
        let manifest = read(&target, "Cargo.toml");

        assert!(
            manifest.contains("default-features = false"),
            "the arcature dependency must opt out of the framework defaults, \
             or the explicit feature list under it means nothing: {manifest}"
        );
        // The application's own default feature set is empty, so a plain
        // `cargo build --release` gets the dependency list and nothing more.
        assert!(manifest.contains("default = []"), "{manifest}");

        // `dev-proxy` forwards Vite requests over IPC and belongs to
        // `arc dev`. It may be reachable through the `dev` feature and
        // through no other line in the file.
        for line in manifest.lines() {
            let line = line.trim();
            assert!(
                !line.contains("dev-proxy") || line.starts_with("dev = ["),
                "dev-proxy must be reachable only through the `dev` feature: {line}"
            );
        }

        // Everything a release binary must not link, checked against the
        // dependency section rather than the whole file, so the `dev` and
        // `uag` feature definitions above cannot mask a real entry.
        let dependencies = manifest
            .split("[dependencies]")
            .nth(1)
            .expect("a [dependencies] section")
            .split("\n[")
            .next()
            .expect("the section body");
        for feature in [
            "\"cli\"",
            "\"templates\"",
            "\"dev-proxy\"",
            "\"uag\"",
            "\"otel\"",
            "\"oauth\"",
            "\"api-docs\"",
            "\"storage-s3\"",
            "\"test-kit\"",
        ] {
            assert!(
                !dependencies.contains(feature),
                "{feature} must not be in the release dependency list: {dependencies}"
            );
        }
    }
}

#[test]
fn the_env_file_ships_an_empty_app_key_and_the_matching_database_url() {
    let scratch = Scratch::new("env");
    for database in Database::ALL {
        let target = scratch.join(database.as_str());
        generate(&target, Stack::default(), database).expect("generated");
        let env = read(&target, ".env");
        assert!(
            env.lines().any(|line| line.trim() == "APP_KEY="),
            "APP_KEY must ship empty for `arc key:generate` to fill in: {env}"
        );
        let expected = database
            .default_url()
            .replace("__RUST_NAME__", database.as_str());
        assert!(env.contains(&expected), "{env}");
    }
}

#[test]
fn the_gitignore_excludes_the_generated_typescript_and_the_dev_scratch_directory() {
    let scratch = Scratch::new("gitignore");
    let target = scratch.join("demo");
    generate(&target, Stack::default(), Database::default()).expect("generated");
    let ignore = read(&target, ".gitignore");
    assert!(ignore.contains("resources/js/generated/"), "{ignore}");
    assert!(ignore.contains(".arcature/"), "{ignore}");
}

#[test]
fn the_typescript_config_aliases_the_generated_bindings() {
    let scratch = Scratch::new("tsconfig");
    for stack in Stack::ALL {
        let target = scratch.join(stack.as_str());
        generate(&target, stack, Database::default()).expect("generated");
        let tsconfig = read(&target, "tsconfig.json");
        assert!(tsconfig.contains("@/generated"), "{tsconfig}");
        assert!(tsconfig.contains("resources/js/generated"), "{tsconfig}");
    }
}

#[test]
fn no_template_reaches_for_a_compile_time_checked_sqlx_query() {
    let scratch = Scratch::new("sqlx");
    let target = scratch.join("demo");
    generate(&target, Stack::default(), Database::default()).expect("generated");
    for (path, contents) in walk(&target) {
        assert!(
            !contents.contains("sqlx::query!"),
            "{path} uses sqlx::query!, which needs a live database at compile time"
        );
    }
}

#[test]
fn generating_over_something_that_already_exists_is_refused() {
    let scratch = Scratch::new("existing");
    let target = scratch.join("demo");
    std::fs::create_dir_all(&target).expect("mkdir");
    assert!(matches!(
        generate(&target, Stack::default(), Database::default()),
        Err(TemplateError::ExistingTarget { .. })
    ));
}

#[test]
fn the_crate_name_and_binary_are_named_after_the_destination_directory() {
    let scratch = Scratch::new("naming");
    let target = scratch.join("sub-app");
    generate(&target, Stack::default(), Database::default()).expect("generated");
    let manifest = read(&target, "Cargo.toml");
    assert!(manifest.contains("name = \"sub_app\""), "{manifest}");
    let main = read(&target, "src/main.rs");
    assert!(main.contains("sub_app::run("), "{main}");
}

#[test]
fn a_frontend_change_cannot_reach_the_rust_build() {
    // `arc dev` refuses to run Cargo for a `.tsx`, `.css` or `.vue` save --
    // Vite has already handled it -- and `src/cli/commands/dev/watch.rs`
    // tests that refusal. This is the other half of the same property, and
    // the half a template edit could break silently: even if something did
    // ask Cargo to build, no frontend file may be an input to that build.
    //
    // There are two ways a non-Rust file becomes a rebuild trigger. A build
    // script re-runs whenever anything it declares changes, and the
    // `include_*!` family records the file it reads in rustc's dep-info. A
    // generated project uses neither, so the frontend and the Rust build are
    // disjoint by construction rather than by convention.
    for stack in Stack::ALL {
        for database in Database::ALL {
            let label = format!("{}-{}", stack.as_str(), database.as_str());
            let scratch = Scratch::new(&label);
            let target = scratch.join("demo");
            generate(&target, stack, database).expect("generated");

            let manifest = read(&target, "Cargo.toml");
            assert!(
                !manifest.contains("build ="),
                "{label}: the manifest names a build script"
            );

            for (path, contents) in walk(&target) {
                assert!(
                    !path.ends_with("build.rs"),
                    "{label}: {path} is a build script, which re-runs on every build"
                );
                if !path.ends_with(".rs") {
                    continue;
                }
                for embed in ["include_str!", "include_bytes!", "include_dir!"] {
                    assert!(
                        !contents.contains(embed),
                        "{label}: {path} uses {embed}, which makes the file it reads \
                         an input to the Rust build"
                    );
                }
            }
        }
    }
}

/// The compiled-view scaffold is wired end to end: the templates land where
/// askama looks for them, the view struct that names them is generated, and
/// the manifest turns on the framework feature that makes the derive exist.
///
/// Any one of the three missing is a generated project that does not
/// compile, and the three live in different files, so nothing but a test
/// keeps them in step.
#[test]
fn the_scaffold_ships_a_compiled_view_and_the_feature_that_builds_it() {
    let scratch = Scratch::new("views");
    for stack in Stack::ALL {
        for database in Database::ALL {
            let label = format!("{}-{}", stack.as_str(), database.as_str());
            let target = scratch.join(&label);
            generate(&target, stack, database).expect("generated");

            // Askama resolves `path = "..."` against `CARGO_MANIFEST_DIR/
            // templates`, so the project root is the only place these work.
            for required in [
                "templates/layout.html",
                "templates/welcome.html",
                "app/views/mod.rs",
            ] {
                assert!(
                    target.join(required).is_file(),
                    "{label} is missing {required}"
                );
            }

            let manifest = read(&target, "Cargo.toml");
            assert!(manifest.contains("\"views\""), "{manifest}");

            let view = read(&target, "app/views/mod.rs");
            assert!(view.contains("path = \"welcome.html\""), "{view}");
            // Without this the derive emits `askama::` paths and the
            // generated project would have to depend on askama itself.
            assert!(view.contains("askama = arcature::askama"), "{view}");

            let welcome = read(&target, "templates/welcome.html");
            assert!(
                welcome.contains("{% extends \"layout.html\" %}"),
                "{welcome}"
            );

            let routes = read(&target, "routes/mod.rs");
            assert!(routes.contains("HomeController::welcome"), "{routes}");

            // Askama reads the templates during `cargo build`, so the Rust
            // stage of the image needs them as source. Without this line the
            // tree compiles on a laptop and the release image does not build
            // at all.
            let dockerfile = read(&target, "Dockerfile");
            assert!(
                dockerfile.contains("COPY templates ./templates"),
                "{dockerfile}"
            );
        }
    }
}
