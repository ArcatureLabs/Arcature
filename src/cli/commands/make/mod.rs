//! `arc make:<kind> <name>` — write scaffolded files into the project.
//!
//! The generator is deliberately small: [`blueprint`] decides *what* to write
//! and [`name`] decides *what to call it*, leaving this module with the parts
//! that touch the disk — refusing to overwrite, creating the directory chain,
//! and teaching the neighbouring `mod.rs` about the new file.
//!
//! # One file, except when the thing is not one file
//!
//! Nineteen of the twenty-one kinds write a single file, and [`run`] is the way
//! to ask for one. `module` writes four, because a directory holding a
//! controller but no service and no routes is not a module — it is a
//! controller in an oddly named directory. [`run_all`] is the honest answer
//! for every kind, and it is what the CLI dispatches to.
//!
//! # Refusing to overwrite, with no escape hatch
//!
//! There is no `--force`. A generator writes a starting point; once a file
//! exists it holds work the developer did, and no flag makes clobbering it a
//! good default. Deleting the file first is one command, is visible in `git
//! status`, and cannot happen by accident in a shell history replay.

pub mod blueprint;
pub mod name;

use std::path::{Path, PathBuf};

use blueprint::Artifact;
use name::{ArtifactName, NameError};

use crate::cli::parser::MakeKind;

/// What a successful generation produced, so the caller can report it and a
/// test can assert on it without re-deriving the paths.
#[derive(Debug, Clone)]
pub struct Generated {
    /// The file that was written, relative to the project root.
    pub path: PathBuf,
    /// The `mod.rs` files that gained a declaration or were created.
    pub touched_modules: Vec<PathBuf>,
    /// Follow-up the generator left to the developer.
    pub notes: Vec<String>,
}

impl Generated {
    /// Print what happened.
    ///
    /// The notes matter as much as the path: a migration that is not in
    /// `Migrator::migrations()` never runs, and finding that out at deploy
    /// time is exactly the failure this line exists to prevent.
    pub fn report(&self) {
        println!("created {}", self.path.display());
        for module in &self.touched_modules {
            println!("updated {}", module.display());
        }
        for note in &self.notes {
            println!("note: {note}");
        }
    }
}

/// Execute `arc make:<kind> <name>` against the current directory.
///
/// # Errors
///
/// See [`MakeError`]. The common ones are a malformed name and a target that
/// already exists.
pub fn run(kind: MakeKind, raw_name: &str) -> Result<Generated, MakeError> {
    let root = current_root()?;
    generate(kind, raw_name, &root)
}

/// Execute `arc make:<kind> <name>`, reporting *every* file it wrote.
///
/// [`run`] answers with the primary artifact and is what a caller wanting one
/// file should use. `module` writes four, and a command that says `created
/// app/modules/billing/mod.rs` while silently writing three siblings is a
/// command whose output cannot be trusted.
///
/// # Errors
///
/// See [`MakeError`].
pub fn run_all(kind: MakeKind, raw_name: &str) -> Result<Vec<Generated>, MakeError> {
    let root = current_root()?;
    generate_all(kind, raw_name, &root)
}

/// The directory `arc` was invoked from.
fn current_root() -> Result<PathBuf, MakeError> {
    std::env::current_dir().map_err(|source| MakeError::Io {
        path: PathBuf::from("."),
        source,
    })
}

/// The testable half of [`run`]: everything, against an explicit project root.
///
/// # Errors
///
/// See [`MakeError`].
pub fn generate(kind: MakeKind, raw_name: &str, root: &Path) -> Result<Generated, MakeError> {
    let mut generated = generate_all(kind, raw_name, root)?;
    // `generate_all` never returns an empty vector -- `plan_all` answers every
    // kind with at least the primary artifact -- but panicking on the
    // impossible is still worse than an error the caller already handles.
    if generated.is_empty() {
        return Err(MakeError::NotAnApplicationRoot {
            root: root.to_path_buf(),
        });
    }
    Ok(generated.remove(0))
}

/// The testable half of [`run_all`].
///
/// # Errors
///
/// See [`MakeError`].
pub fn generate_all(
    kind: MakeKind,
    raw_name: &str,
    root: &Path,
) -> Result<Vec<Generated>, MakeError> {
    if !root.join("Cargo.toml").is_file() {
        return Err(MakeError::NotAnApplicationRoot {
            root: root.to_path_buf(),
        });
    }

    let parsed = ArtifactName::parse(raw_name).map_err(MakeError::Name)?;
    let artifacts = blueprint::plan_all(kind, &parsed);

    // Every target is checked before the first one is written. Writing as we
    // go would leave a module with two of its four files on disk and a
    // `MakeError::Exists` on the terminal, and the developer's only way out is
    // to work out which two were the new ones.
    for artifact in &artifacts {
        if root.join(&artifact.path).exists() {
            return Err(MakeError::Exists {
                path: artifact.path.clone(),
            });
        }
    }

    let mut written = Vec::with_capacity(artifacts.len());
    for artifact in &artifacts {
        written.push(write(artifact, root)?);
    }

    if kind == MakeKind::Module
        && let Some(primary) = written.first_mut()
    {
        register_feature_module(root, &parsed, primary)?;
    }

    Ok(written)
}

/// Teach `app/modules/mod.rs` about a module the way that file documents:
/// a `pub mod` line, one descriptor entry, one routes entry.
///
/// The `pub mod` line goes through the same [`register`] every other kind
/// uses, which is why the template keeps its declarations at the bottom --
/// that is where `with_declaration` puts the first one, and where it keeps
/// putting the rest.
fn register_feature_module(
    root: &Path,
    name: &ArtifactName,
    generated: &mut Generated,
) -> Result<(), MakeError> {
    let stem = name.file_stem("");

    let mut directory = PathBuf::from("app/modules");
    for segment in name.segments() {
        directory.push(segment);
    }
    register(root, &directory, &stem, generated)?;

    let mut module_path = String::new();
    for segment in name.segments() {
        module_path.push_str(segment);
        module_path.push_str("::");
    }
    module_path.push_str(&stem);

    // `module!` builds its accessor by lowercasing the whole type name rather
    // than snake-casing it, so `UserAccounts` answers to `useraccounts_module`
    // and computing this any other way would emit a call to a function that
    // does not exist.
    let accessor = format!("{}_module", name.type_name("").to_lowercase());

    let list_relative = PathBuf::from("app/modules/mod.rs");
    let list_path = root.join(&list_relative);
    let original = read_file(&list_path)?;
    let mut updated = original.clone();

    for (region, entry) in [
        (
            "descriptors",
            format!("{module_path}::{accessor}().clone(),"),
        ),
        ("routes", format!("{module_path}::routes::{stem}_routes(),")),
    ] {
        match insert_in_region(&updated, region, &entry) {
            Some(next) => updated = next,
            // A missing marker is a file somebody edited, and rewriting it
            // from a guess would be worse than saying so. The files are
            // already on disk and the module already compiles; all that is
            // left is one line the developer can paste.
            None => generated.notes.push(format!(
                "app/modules/mod.rs has no `// arc:modules {region}` marker -- \
                 add `{entry}` to its {region} list by hand"
            )),
        }
    }

    if updated != original {
        write_file(&list_path, &updated)?;
        if !generated.touched_modules.contains(&list_relative) {
            generated.touched_modules.push(list_relative);
        }
    }
    Ok(())
}

/// Insert `entry` just above the `// arc:end` that closes `// arc:modules
/// <region>`, at the marker's own indentation.
///
/// `None` means the region is not there. Returns `source` untouched when the
/// entry already is, so re-running the generator after deleting a module's
/// directory does not leave the list holding it twice.
fn insert_in_region(source: &str, region: &str, entry: &str) -> Option<String> {
    let open = format!("// arc:modules {region}");
    let lines: Vec<&str> = source.lines().collect();

    let start = lines.iter().position(|line| line.trim() == open)?;
    let end = lines[start..]
        .iter()
        .position(|line| line.trim() == "// arc:end")?
        + start;

    if lines[start..end].iter().any(|line| line.trim() == entry) {
        return Some(source.to_string());
    }

    let indent: String = lines[start]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let mut out: Vec<String> = lines[..end].iter().map(|l| (*l).to_string()).collect();
    out.push(format!("{indent}{entry}"));
    out.extend(lines[end..].iter().map(|l| (*l).to_string()));
    Some(out.join("\n") + "\n")
}

/// Write the planned artifact and register it.
fn write(artifact: &Artifact, root: &Path) -> Result<Generated, MakeError> {
    let target = root.join(&artifact.path);
    if target.exists() {
        return Err(MakeError::Exists {
            path: artifact.path.clone(),
        });
    }

    let directory = target.parent().ok_or_else(|| MakeError::Exists {
        path: artifact.path.clone(),
    })?;
    create_dir_all(directory)?;
    write_file(&target, &artifact.contents)?;

    let mut generated = Generated {
        path: artifact.path.clone(),
        touched_modules: Vec::new(),
        notes: artifact.notes.clone(),
    };

    if artifact.register_module {
        let stem = artifact
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        let relative_dir = artifact.path.parent().unwrap_or(Path::new(""));
        register(root, relative_dir, &stem, &mut generated)?;
    }

    Ok(generated)
}

/// Declare `stem` in `<root>/<relative_dir>/mod.rs`, creating that `mod.rs`
/// (and declaring *it* one level up) when the directory is new.
///
/// Recursion stops at the project root: the top-level `app/`, `database/`,
/// and `tests/` directories are wired into the crate by `src/lib.rs` with
/// `#[path]`, and a generator that edits a crate root is a generator that can
/// break the build in a way `git diff` does not make obvious.
fn register(
    root: &Path,
    relative_dir: &Path,
    stem: &str,
    generated: &mut Generated,
) -> Result<(), MakeError> {
    let module_relative = relative_dir.join("mod.rs");
    let module_path = root.join(&module_relative);

    if !module_path.exists() {
        let directory_name = relative_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        write_file(&module_path, &new_module_header(&directory_name))?;
        generated.touched_modules.push(module_relative.clone());

        match relative_dir.parent() {
            Some(parent) if parent != Path::new("") => {
                register(root, parent, &directory_name, generated)?;
            }
            _ => generated.notes.push(format!(
                "created {} -- declare it in src/lib.rs with \
                 `#[path = \"../{directory_name}/mod.rs\"] pub mod {directory_name};` \
                 if it is not there already",
                module_relative.display()
            )),
        }
    }

    let existing = read_file(&module_path)?;
    if declares(&existing, stem) {
        return Ok(());
    }
    write_file(&module_path, &with_declaration(&existing, stem))?;
    if !generated.touched_modules.contains(&module_relative) {
        generated.touched_modules.push(module_relative);
    }
    Ok(())
}

/// Whether `source` already declares a `stem` module, so re-running a
/// generator after deleting only the file does not duplicate the line.
fn declares(source: &str, stem: &str) -> bool {
    let with_pub = format!("pub mod {stem};");
    let bare = format!("mod {stem};");
    source
        .lines()
        .map(str::trim)
        .any(|line| line == with_pub || line == bare)
}

/// Insert `pub mod <stem>;` after the last existing `pub mod` line, so the
/// declarations stay grouped above any `pub use` re-exports.
fn with_declaration(source: &str, stem: &str) -> String {
    let declaration = format!("pub mod {stem};");
    let lines: Vec<&str> = source.lines().collect();
    let last_module = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with("pub mod "));

    let mut out: Vec<String> = match last_module {
        Some(index) => {
            let mut merged: Vec<String> =
                lines[..=index].iter().map(|l| (*l).to_string()).collect();
            merged.push(declaration);
            merged.extend(lines[index + 1..].iter().map(|l| (*l).to_string()));
            merged
        }
        None => {
            let mut merged: Vec<String> = lines.iter().map(|l| (*l).to_string()).collect();
            if merged.last().is_some_and(|last| !last.trim().is_empty()) {
                merged.push(String::new());
            }
            merged.push(declaration);
            merged
        }
    };
    out.push(String::new());
    out.join("\n").trim_end().to_string() + "\n"
}

/// The header a freshly created `mod.rs` opens with. A bare file with one
/// `pub mod` line and no explanation is the kind of thing that gets deleted
/// by the next reader.
fn new_module_header(directory: &str) -> String {
    format!("//! The application's `{directory}`.\n")
}

fn create_dir_all(path: &Path) -> Result<(), MakeError> {
    std::fs::create_dir_all(path).map_err(|source| MakeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_file(path: &Path) -> Result<String, MakeError> {
    std::fs::read_to_string(path).map_err(|source| MakeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), MakeError> {
    std::fs::write(path, contents).map_err(|source| MakeError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// An error from a `make:*` command.
#[derive(Debug)]
pub enum MakeError {
    /// The working directory is not an Arcature application.
    NotAnApplicationRoot { root: PathBuf },
    /// The name could not be parsed.
    Name(NameError),
    /// The target file already exists. There is no `--force`.
    Exists { path: PathBuf },
    /// A filesystem operation failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for MakeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnApplicationRoot { root } => write!(
                formatter,
                "{} has no Cargo.toml, so it is not an application root; \
                 run this from the directory `arc new` created",
                root.display()
            ),
            Self::Name(error) => write!(formatter, "{error}"),
            Self::Exists { path } => write!(
                formatter,
                "{} already exists and will not be overwritten; \
                 delete it first if you meant to regenerate it",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for MakeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Name(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project root with the parts a generator expects to find.
    fn scaffold() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").expect("manifest");
        std::fs::create_dir_all(root.join("app/controllers")).expect("controllers");
        std::fs::write(
            root.join("app/mod.rs"),
            "//! The application layer.\n\npub mod controllers;\n",
        )
        .expect("app mod");
        std::fs::write(
            root.join("app/controllers/mod.rs"),
            "//! The application's controllers (Axum handlers).\n\npub mod home_controller;\n\npub use home_controller::HomeController;\n",
        )
        .expect("controllers mod");
        dir
    }

    #[test]
    fn a_generator_writes_its_file_and_registers_it_next_door() {
        let dir = scaffold();
        let generated =
            generate(MakeKind::Controller, "user", dir.path()).expect("generation succeeds");

        assert_eq!(
            generated.path,
            PathBuf::from("app/controllers/user_controller.rs")
        );
        let written = std::fs::read_to_string(dir.path().join(&generated.path)).expect("written");
        assert!(written.contains("pub struct UserController;"));

        let module = std::fs::read_to_string(dir.path().join("app/controllers/mod.rs"))
            .expect("module read");
        assert!(module.contains("pub mod user_controller;"));
        // The declaration joins the existing ones rather than landing after
        // the re-export.
        let declaration = module.find("pub mod user_controller;").expect("declared");
        let reexport = module.find("pub use home_controller").expect("re-export");
        assert!(declaration < reexport, "{module}");
    }

    #[test]
    fn a_generator_refuses_to_overwrite_an_existing_file() {
        let dir = scaffold();
        generate(MakeKind::Controller, "user", dir.path()).expect("first run");
        let before = std::fs::read_to_string(dir.path().join("app/controllers/user_controller.rs"))
            .expect("read");

        let error = generate(MakeKind::Controller, "User", dir.path())
            .expect_err("the second run must refuse");
        assert!(matches!(error, MakeError::Exists { .. }));
        assert!(error.to_string().contains("will not be overwritten"));

        let after = std::fs::read_to_string(dir.path().join("app/controllers/user_controller.rs"))
            .expect("read");
        assert_eq!(before, after, "the existing file was modified");
    }

    #[test]
    fn a_new_directory_gets_a_module_file_and_is_declared_upward() {
        let dir = scaffold();
        let generated = generate(MakeKind::Service, "billing", dir.path()).expect("generation");

        assert_eq!(
            generated.path,
            PathBuf::from("app/services/billing_service.rs")
        );
        let services = std::fs::read_to_string(dir.path().join("app/services/mod.rs"))
            .expect("services mod created");
        assert!(services.contains("pub mod billing_service;"));

        let app = std::fs::read_to_string(dir.path().join("app/mod.rs")).expect("app mod");
        assert!(app.contains("pub mod services;"), "{app}");
    }

    #[test]
    fn a_nested_name_creates_the_whole_module_chain() {
        let dir = scaffold();
        generate(MakeKind::Controller, "admin/users/show", dir.path()).expect("generation");

        assert!(
            dir.path()
                .join("app/controllers/admin/users/show_controller.rs")
                .is_file()
        );
        let users = std::fs::read_to_string(dir.path().join("app/controllers/admin/users/mod.rs"))
            .expect("users mod");
        assert!(users.contains("pub mod show_controller;"));
        let admin = std::fs::read_to_string(dir.path().join("app/controllers/admin/mod.rs"))
            .expect("admin");
        assert!(admin.contains("pub mod users;"));
        let controllers = std::fs::read_to_string(dir.path().join("app/controllers/mod.rs"))
            .expect("controllers");
        assert!(controllers.contains("pub mod admin;"));
    }

    #[test]
    fn a_top_level_directory_the_crate_root_does_not_know_about_is_reported() {
        let dir = scaffold();
        let generated = generate(MakeKind::Seeder, "users", dir.path()).expect("generation");
        assert!(
            generated
                .notes
                .iter()
                .any(|note| note.contains("src/lib.rs")),
            "{:?}",
            generated.notes
        );
    }

    #[test]
    fn a_test_lands_in_tests_without_a_module_declaration() {
        let dir = scaffold();
        let generated = generate(MakeKind::Test, "checkout", dir.path()).expect("generation");
        assert_eq!(generated.path, PathBuf::from("tests/checkout.rs"));
        assert!(generated.touched_modules.is_empty());
        assert!(!dir.path().join("tests/mod.rs").exists());
    }

    #[test]
    fn a_second_declaration_of_the_same_module_is_not_appended_twice() {
        let dir = scaffold();
        generate(MakeKind::Controller, "user", dir.path()).expect("first run");
        std::fs::remove_file(dir.path().join("app/controllers/user_controller.rs"))
            .expect("remove the file but keep the declaration");
        generate(MakeKind::Controller, "user", dir.path()).expect("second run");

        let module =
            std::fs::read_to_string(dir.path().join("app/controllers/mod.rs")).expect("module");
        assert_eq!(
            module.matches("pub mod user_controller;").count(),
            1,
            "{module}"
        );
    }

    #[test]
    fn a_name_that_escapes_the_project_is_refused() {
        let dir = scaffold();
        let error = generate(MakeKind::Controller, "../../evil", dir.path())
            .expect_err("traversal must be refused");
        assert!(matches!(error, MakeError::Name(_)));
    }

    #[test]
    fn a_directory_without_a_manifest_is_not_an_application_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = generate(MakeKind::Controller, "user", dir.path())
            .expect_err("a bare directory is not an app");
        assert!(matches!(error, MakeError::NotAnApplicationRoot { .. }));
        assert!(error.to_string().contains("Cargo.toml"));
    }

    /// The scaffold's `app/modules/mod.rs`, cut down to the parts the
    /// generator actually matches on.
    fn with_module_list(root: &Path) {
        std::fs::create_dir_all(root.join("app/modules")).expect("modules dir");
        std::fs::write(
            root.join("app/modules/mod.rs"),
            "//! Feature modules.\n\
             \n\
             pub fn modules() -> Vec<ModuleDescriptor> {\n\
             \x20   vec![\n\
             \x20       // arc:modules descriptors\n\
             \x20       // arc:end\n\
             \x20   ]\n\
             }\n\
             \n\
             pub fn routes() -> Routes<AppState> {\n\
             \x20   let collections: Vec<Routes<AppState>> = vec![\n\
             \x20       // arc:modules routes\n\
             \x20       // arc:end\n\
             \x20   ];\n\
             \x20   collections.into_iter().fold(Routes::empty(), Routes::merge)\n\
             }\n",
        )
        .expect("modules mod");
    }

    #[test]
    fn a_module_writes_a_directory_and_registers_the_whole_thing() {
        let dir = scaffold();
        with_module_list(dir.path());

        let generated = generate_all(MakeKind::Module, "billing", dir.path()).expect("generation");
        let paths: Vec<_> = generated.iter().map(|g| g.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("app/modules/billing/mod.rs"),
                PathBuf::from("app/modules/billing/controller.rs"),
                PathBuf::from("app/modules/billing/service.rs"),
                PathBuf::from("app/modules/billing/routes.rs"),
            ]
        );
        for path in &paths {
            assert!(
                dir.path().join(path).is_file(),
                "{} missing",
                path.display()
            );
        }
        // Nothing was left for the developer: the point of the kind is that
        // the module is wired up when the command returns.
        assert!(
            generated.iter().all(|g| g.notes.is_empty()),
            "{:?}",
            generated.iter().map(|g| &g.notes).collect::<Vec<_>>()
        );

        let list = std::fs::read_to_string(dir.path().join("app/modules/mod.rs")).expect("list");
        assert!(list.contains("pub mod billing;"), "{list}");
        assert!(
            list.contains("        billing::billing_module().clone(),"),
            "{list}"
        );
        assert!(
            list.contains("        billing::routes::billing_routes(),"),
            "{list}"
        );

        let root_file = std::fs::read_to_string(dir.path().join("app/modules/billing/mod.rs"))
            .expect("module root");
        assert!(root_file.contains("pub Billing {"), "{root_file}");
        assert!(
            root_file.contains("routes: routes::BILLING_ROUTES,"),
            "{root_file}"
        );
        // The service is named in the `module!` block but deliberately not
        // imported: the macro records it as a string and never resolves it, so
        // an import would only ever be an unused one.
        assert!(
            root_file.contains("services: [BillingService],"),
            "{root_file}"
        );
        assert!(!root_file.contains("use service::"), "{root_file}");
    }

    #[test]
    fn a_module_route_name_carries_the_module_so_two_modules_cannot_collide() {
        let dir = scaffold();
        with_module_list(dir.path());
        generate_all(MakeKind::Module, "admin/billing", dir.path()).expect("generation");

        let routes =
            std::fs::read_to_string(dir.path().join("app/modules/admin/billing/routes.rs"))
                .expect("routes");
        // `Routes::merge` resolves a duplicate name by letting the later one
        // win, silently. A bare `index` in every module would make that a
        // matter of registration order.
        assert!(routes.contains("{ name: admin.billing.index }"), "{routes}");
        assert!(routes.contains("get \"/admin/billing\""), "{routes}");

        let list = std::fs::read_to_string(dir.path().join("app/modules/mod.rs")).expect("list");
        assert!(
            list.contains("        admin::billing::billing_module().clone(),"),
            "{list}"
        );
        assert!(
            list.contains("        admin::billing::routes::billing_routes(),"),
            "{list}"
        );
    }

    #[test]
    fn the_registered_accessor_matches_what_the_module_macro_emits() {
        let dir = scaffold();
        with_module_list(dir.path());
        generate_all(MakeKind::Module, "user_accounts", dir.path()).expect("generation");

        // `module!` lowercases the whole type name instead of snake-casing it,
        // so `UserAccounts` answers to `useraccounts_module`. Anything else
        // here compiles into a call to a function that does not exist.
        let list = std::fs::read_to_string(dir.path().join("app/modules/mod.rs")).expect("list");
        assert!(
            list.contains("user_accounts::useraccounts_module().clone(),"),
            "{list}"
        );
        assert!(
            list.contains("user_accounts::routes::user_accounts_routes(),"),
            "{list}"
        );
    }

    #[test]
    fn a_module_is_registered_once_however_often_it_is_regenerated() {
        let dir = scaffold();
        with_module_list(dir.path());
        generate_all(MakeKind::Module, "billing", dir.path()).expect("first run");
        std::fs::remove_dir_all(dir.path().join("app/modules/billing"))
            .expect("remove the directory but keep the registration");
        generate_all(MakeKind::Module, "billing", dir.path()).expect("second run");

        let list = std::fs::read_to_string(dir.path().join("app/modules/mod.rs")).expect("list");
        assert_eq!(list.matches("pub mod billing;").count(), 1, "{list}");
        assert_eq!(
            list.matches("billing::billing_module().clone(),").count(),
            1,
            "{list}"
        );
        assert_eq!(
            list.matches("billing::routes::billing_routes(),").count(),
            1,
            "{list}"
        );
    }

    #[test]
    fn a_module_list_with_no_markers_is_reported_rather_than_guessed_at() {
        // No `with_module_list`: this is the shape of a project that predates
        // the generated list, or one whose markers somebody deleted.
        let dir = scaffold();
        let generated = generate_all(MakeKind::Module, "billing", dir.path()).expect("generation");

        assert!(dir.path().join("app/modules/billing/routes.rs").is_file());
        let notes = &generated[0].notes;
        assert!(
            notes.iter().any(|n| n.contains("arc:modules descriptors")
                && n.contains("billing::billing_module().clone(),")),
            "{notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("arc:modules routes")
                && n.contains("billing::routes::billing_routes(),")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_module_whose_directory_is_half_there_writes_nothing() {
        let dir = scaffold();
        with_module_list(dir.path());
        std::fs::create_dir_all(dir.path().join("app/modules/billing")).expect("dir");
        std::fs::write(
            dir.path().join("app/modules/billing/service.rs"),
            "// mine\n",
        )
        .expect("service");

        let error = generate_all(MakeKind::Module, "billing", dir.path())
            .expect_err("a collision must refuse");
        assert!(matches!(error, MakeError::Exists { .. }));
        // The check runs over every artifact before the first write, so the
        // three files that would have succeeded are not left behind for the
        // developer to tell apart from their own.
        assert!(!dir.path().join("app/modules/billing/mod.rs").exists());
        assert!(
            !dir.path()
                .join("app/modules/billing/controller.rs")
                .exists()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("app/modules/billing/service.rs"))
                .expect("read"),
            "// mine\n"
        );
    }

    #[test]
    fn a_view_writes_the_struct_and_the_template_it_is_the_type_of() {
        let dir = scaffold();

        let generated =
            generate_all(MakeKind::View, "admin/receipt", dir.path()).expect("generation");
        let paths: Vec<_> = generated.iter().map(|g| g.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("app/views/admin/receipt_view.rs"),
                PathBuf::from("templates/admin/receipt.html"),
            ]
        );
        for path in &paths {
            assert!(
                dir.path().join(path).is_file(),
                "{} missing",
                path.display()
            );
        }

        // The two halves have to agree on one string or the crate does not
        // compile, and they are produced by two different functions. This is
        // the assertion that keeps them together.
        let view = std::fs::read_to_string(dir.path().join(&paths[0])).expect("view");
        assert!(
            view.contains(r#"#[template(path = "admin/receipt.html", askama = arcature::askama)]"#),
            "{view}"
        );

        // Only the Rust half is declared to rustc. A `mod.rs` in `templates/`
        // would be a Rust file in a directory the compiler never looks at.
        let views =
            std::fs::read_to_string(dir.path().join("app/views/admin/mod.rs")).expect("views mod");
        assert!(views.contains("pub mod receipt_view;"), "{views}");
        assert!(!dir.path().join("templates/admin/mod.rs").exists());
    }

    #[test]
    fn every_kind_can_be_generated_into_a_fresh_project() {
        let dir = scaffold();
        for kind in MakeKind::ALL {
            let generated = generate(*kind, "widget", dir.path())
                .unwrap_or_else(|e| panic!("{} failed: {e}", kind.as_str()));
            assert!(
                dir.path().join(&generated.path).is_file(),
                "{} wrote nothing",
                kind.as_str()
            );
        }
    }
}
