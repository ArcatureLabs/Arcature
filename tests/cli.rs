//! End-to-end tests for the `arc` binary.
//!
//! The unit tests inside `src/cli` cover each piece in isolation; these run the
//! real executable so the parts nobody can unit-test are exercised too: clap's
//! argument handling, the exit codes, and what actually lands on stdout and
//! stderr. A generator that returns the right `Generated` struct but exits
//! non-zero is still broken from a shell script's point of view.
//!
//! The whole file is gated on `cli` because `CARGO_BIN_EXE_arc` only exists
//! when the binary's required feature is on.
#![cfg(feature = "cli")]

use std::path::Path;
use std::process::{Command, Output};

/// Run `arc` inside `directory`.
fn arc(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_arc"))
        .args(args)
        .current_dir(directory)
        .output()
        .expect("the arc binary should be runnable")
}

/// A directory that looks enough like an application root for the generators.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("manifest");
    dir
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_version_flag_and_the_version_command_agree() {
    let dir = project();
    let flag = arc(dir.path(), &["--version"]);
    let word = arc(dir.path(), &["version"]);

    assert!(flag.status.success());
    assert!(word.status.success());
    assert_eq!(stdout(&flag), stdout(&word));
    assert!(stdout(&flag).starts_with("arcature "), "{}", stdout(&flag));
}

#[test]
fn help_renders_to_stdout_and_exits_zero() {
    let dir = project();
    let output = arc(dir.path(), &["--help"]);

    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("make:controller"), "{text}");
    assert!(text.contains("storage:link"), "{text}");
    assert!(text.contains("db:fresh"), "{text}");
}

#[test]
fn a_mistyped_subcommand_fails_and_suggests_the_real_one() {
    let dir = project();
    let output = arc(dir.path(), &["migrat"]);

    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("migrate"), "{text}");
}

#[test]
fn a_generator_writes_a_file_and_declares_it_next_door() {
    let dir = project();
    let output = arc(dir.path(), &["make:controller", "users/show"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let written = dir.path().join("app/controllers/users/show_controller.rs");
    let source = std::fs::read_to_string(&written).expect("the controller should exist");
    assert!(source.contains("ShowController"), "{source}");

    let module = std::fs::read_to_string(dir.path().join("app/controllers/users/mod.rs"))
        .expect("the module file should exist");
    assert!(module.contains("pub mod show_controller;"), "{module}");
}

#[test]
fn a_generator_refuses_to_overwrite_and_leaves_the_file_alone() {
    let dir = project();
    assert!(arc(dir.path(), &["make:model", "user"]).status.success());

    let written = dir.path().join("app/models/user.rs");
    std::fs::write(&written, "// hand-written work\n").expect("edit");

    let second = arc(dir.path(), &["make:model", "user"]);
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains("already exists"),
        "{}",
        stderr(&second)
    );
    assert_eq!(
        std::fs::read_to_string(&written).expect("read"),
        "// hand-written work\n"
    );
}

#[test]
fn a_generator_outside_an_application_root_says_so() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = arc(dir.path(), &["make:service", "billing"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Cargo.toml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_name_that_tries_to_escape_the_project_is_refused() {
    let dir = project();
    let output = arc(dir.path(), &["make:controller", "../../evil"]);

    assert!(!output.status.success());
    assert!(!dir.path().parent().unwrap().join("evil.rs").exists());
}

#[test]
fn a_destructive_database_command_needs_its_force_flag() {
    let dir = project();
    for command in ["db:fresh", "db:reset"] {
        let output = arc(dir.path(), &[command]);
        assert!(!output.status.success(), "{command} should have refused");
        let text = stderr(&output);
        assert!(text.contains("--force"), "{command}: {text}");
    }
}

#[test]
fn the_destructive_refusal_never_waits_for_an_answer() {
    // No stdin is attached, so a command that prompted would block until the
    // read failed rather than returning promptly with a message. Asserting on
    // the message is what proves the flag is the only confirmation there is.
    let dir = project();
    let output = arc(dir.path(), &["db:fresh"]);
    assert!(
        stderr(&output).contains("Re-run it as"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_application_graph_commands_are_offered_and_refuse_outside_a_crate() {
    // A bare temporary directory, deliberately not `project()`: with no
    // `Cargo.toml` anywhere above it there is nothing to read a graph from,
    // so each command fails on the spot instead of shelling out to cargo.
    let dir = tempfile::tempdir().expect("tempdir");
    let help = stdout(&arc(dir.path(), &["--help"]));

    for command in ["routes", "typegen", "build"] {
        assert!(
            help.contains(command),
            "`{command}` should be in --help:
{help}"
        );

        let output = arc(dir.path(), &[command]);
        assert!(!output.status.success(), "{command} should not succeed");
        assert!(
            stderr(&output).contains("Cargo.toml"),
            "{command} should say what is missing: {}",
            stderr(&output)
        );
    }
}

#[test]
fn routes_takes_a_json_flag_and_typegen_does_not() {
    let dir = tempfile::tempdir().expect("tempdir");

    assert!(
        stdout(&arc(dir.path(), &["routes", "--help"])).contains("--json"),
        "`arc routes` should offer --json"
    );
    assert!(
        !arc(dir.path(), &["typegen", "--json"]).status.success(),
        "`arc typegen` should refuse an argument it does not take"
    );
}

#[test]
fn every_stack_the_help_offers_actually_scaffolds() {
    // The stack list is a promise made in `--help`. A name that parses and
    // then fails, or writes half a tree, is worse than one that was never
    // offered, so each is run end to end rather than checked against the
    // catalogue in-process.
    for (stack, entry) in [
        ("react", "resources/js/app.tsx"),
        ("vue", "resources/js/app.ts"),
        ("svelte", "resources/js/app.ts"),
    ] {
        let dir = project();
        // `--no-install`: `arc new` runs `npm install` by default, and a test
        // suite that reaches the network takes minutes and fails offline.
        // What this test is about is the file tree, not npm.
        let output = arc(
            dir.path(),
            &["new", stack, "--stack", stack, "--no-install"],
        );

        assert!(
            output.status.success(),
            "`arc new --stack {stack}` failed: {}",
            stderr(&output)
        );
        let root = dir.path().join(stack);
        assert!(root.join(entry).exists(), "{stack} has no entry module");
        assert!(
            root.join("bootstrap/app.rs").exists(),
            "{stack} has no bootstrap"
        );
    }
}

#[test]
fn a_stack_the_framework_does_not_ship_is_a_parse_error() {
    let dir = project();
    let output = arc(dir.path(), &["new", "demo", "--stack", "ember"]);

    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("ember"), "{text}");
    assert!(!dir.path().join("demo").exists());
}

#[cfg(feature = "auth")]
#[test]
fn key_generate_show_prints_a_key_without_writing_anything() {
    let dir = project();
    let output = arc(dir.path(), &["key:generate", "--show"]);

    assert!(output.status.success(), "{}", stderr(&output));
    let printed = stdout(&output);
    let key = printed
        .trim()
        .strip_prefix("APP_KEY=")
        .expect("the key should be printed as an assignment");
    assert_eq!(key.len(), 128);
    assert!(!dir.path().join(".env").exists());
}

#[cfg(feature = "auth")]
#[test]
fn key_generate_writes_into_an_existing_env_file() {
    let dir = project();
    std::fs::write(dir.path().join(".env"), "APP_NAME=demo\n").expect("env");

    let output = arc(dir.path(), &["key:generate"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let env = std::fs::read_to_string(dir.path().join(".env")).expect("read");
    assert!(env.contains("APP_NAME=demo"), "{env}");
    assert!(env.contains("APP_KEY="), "{env}");
}

#[test]
fn storage_link_is_idempotent_only_in_the_sense_that_it_refuses_twice() {
    let dir = project();
    let first = arc(dir.path(), &["storage:link"]);
    if !first.status.success() {
        // Windows without Developer Mode or an elevated shell cannot link at
        // all. That is the environment's answer, not a regression, and the
        // message must still name the fix.
        assert!(stderr(&first).contains("not copy"), "{}", stderr(&first));
        return;
    }

    let second = arc(dir.path(), &["storage:link"]);
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains("already exists"),
        "{}",
        stderr(&second)
    );
}

#[test]
fn storage_link_never_leaves_a_copy_of_the_uploads() {
    let dir = project();
    let source = dir.path().join("storage").join("app").join("public");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(source.join("avatar.txt"), "bytes").expect("write");
    // Occupy the destination so the command must refuse.
    std::fs::create_dir_all(dir.path().join("public").join("storage")).expect("occupied");

    let output = arc(dir.path(), &["storage:link"]);
    assert!(!output.status.success());
    assert!(
        !dir.path()
            .join("public")
            .join("storage")
            .join("avatar.txt")
            .exists()
    );
}
