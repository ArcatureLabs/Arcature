//! The parameterised half of the typed route helper, proved by `tsc`.
//!
//! `routes.ts` emits a conditional rest-argument tuple:
//!
//! ```ts
//! type RouteArgs<N extends RouteName> = [ParamName<N>] extends [never]
//!   ? []
//!   : [params: RouteParams<N>];
//! ```
//!
//! so `route('home')` takes no arguments and `route('links.show', { link })`
//! demands its parameters. The unit tests in `src/uag/codegen/routes_ts.rs`
//! pin the *text* of that machinery. Nothing pinned its *behaviour* for a
//! route that has parameters, because the scaffold ships exactly one route
//! and it has none: every earlier end-to-end run exercised the `[]` branch
//! and left the parameter branch resting on string comparisons.
//!
//! This file closes that gap. A fixture application declares four route
//! shapes -- none, one, two, and a wildcard parameter -- and the generated
//! file is type-checked against usage that must compile and, more
//! importantly, against usage that must *not*. A conditional type that
//! accepts everything would pass every positive test ever written; only the
//! negative cases distinguish it from one that works.
//!
//! # The TypeScript compiler is discovered, not depended on
//!
//! Arcature publishes no npm package and its test suite installs nothing
//! (`docs/decisions/0001-no-npm-package.md`), so `tsc` is found if it is
//! present and the type-check is reported as skipped if it is not. In that
//! case the assertions above [`TSC_ENV`] still run: they are the properties
//! the conditional type is built out of, and they hold with no toolchain at
//! all.
//!
//! Point [`TSC_ENV`] at a TypeScript compiler to run the whole file:
//!
//! ```text
//! npm install typescript
//! ARCATURE_TSC=node_modules/typescript/bin/tsc cargo test --test uag_typescript
//! ```

#![cfg(feature = "uag")]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use arcature::dx::application_graph::ApplicationGraph;
use arcature::dx::graph::ModuleDescriptor;
use arcature::dx::route_metadata::{RouteDescriptor, RouteMethod};
use arcature::inertia::contracts::ContractArtifact;
use arcature::uag::codegen::routes_ts;
use arcature::uag::{UagArtifact, build};

/// The environment variable naming a TypeScript compiler.
///
/// Either an executable (`tsc.cmd`, `tsc.exe`) or the compiler's own entry
/// point (`node_modules/typescript/bin/tsc`), which is run under `node`.
const TSC_ENV: &str = "ARCATURE_TSC";

// ---------------------------------------------------------------------------
// The fixture application: one route of every parameter shape.
// ---------------------------------------------------------------------------

const fn route(name: &'static str, path: &'static str, handler: &'static str) -> RouteDescriptor {
    RouteDescriptor {
        method: RouteMethod::Get,
        path,
        name,
        handler,
        pages: &[],
        action_fields: &[],
        action_type: "",
        query_fields: &[],
        query_type: "",
        query_array: false,
        query_string_fields: &[],
        query_string_type: "",
        policies: &[],
    }
}

/// No parameters -- the branch the scaffold already exercised.
const HOME: RouteDescriptor = route("home", "/", "HomeController::index");

/// One parameter.
const SHOW: RouteDescriptor = route("links.show", "/links/{link}", "LinkController::show");

/// Two parameters, so "supplied one of them" is a distinguishable failure.
const COMMENT: RouteDescriptor = route(
    "posts.comments.show",
    "/posts/{post}/comments/{comment}",
    "CommentController::show",
);

/// A wildcard. The star belongs to the *path*, never to the parameter name:
/// the generated `route()` strips it before looking the value up, so the key
/// the type demands has to be the key the runtime reads.
const ASSET: RouteDescriptor = route("assets.show", "/assets/{*path}", "AssetController::show");

fn fixture() -> UagArtifact {
    let mut module = ModuleDescriptor::new("Links");
    module.routes = &[HOME, SHOW, COMMENT, ASSET];
    let graph = ApplicationGraph::new(vec![module]).expect("the fixture module graph is valid");
    build(&graph, &ContractArtifact::new(BTreeMap::new()))
}

fn generated() -> String {
    routes_ts::generate(&fixture())
}

// ---------------------------------------------------------------------------
// Properties the conditional type is built out of. These need no toolchain.
// ---------------------------------------------------------------------------

#[test]
fn every_parameter_shape_reaches_the_route_table() {
    let ts = generated();
    for expected in [
        r#""home": { method: "GET", path: "/", params: [] },"#,
        r#""links.show": { method: "GET", path: "/links/{link}", params: ["link"] },"#,
        r#""posts.comments.show": { method: "GET", path: "/posts/{post}/comments/{comment}", params: ["post", "comment"] },"#,
        r#""assets.show": { method: "GET", path: "/assets/{*path}", params: ["path"] },"#,
    ] {
        assert!(ts.contains(expected), "missing `{expected}` in:\n{ts}");
    }
}

#[test]
fn a_wildcard_parameter_is_named_without_its_star() {
    let ts = generated();
    // The type demands `path`, and the runtime looks up `path` after
    // stripping the star off the token it found in the path. If either side
    // moved, `route('assets.show', { path })` would type-check and then throw
    // "missing parameter" at runtime -- the one failure the helper exists to
    // make impossible.
    assert!(ts.contains(r#"params: ["path"] }"#), "{ts}");
    assert!(
        ts.contains(r#"const wildcard = token.startsWith("*");"#),
        "{ts}"
    );
    assert!(
        ts.contains("const key = wildcard ? token.slice(1) : token;"),
        "{ts}"
    );
}

#[test]
fn the_argument_tuple_is_conditional_on_the_parameter_union() {
    let ts = generated();
    assert!(
        ts.contains("type RouteArgs<N extends RouteName> = [ParamName<N>] extends [never]"),
        "{ts}"
    );
    assert!(ts.contains("  ? []"), "{ts}");
    assert!(ts.contains("  : [params: RouteParams<N>];"), "{ts}");
}

// ---------------------------------------------------------------------------
// The end-to-end proof: `tsc --noEmit` over usage that must and must not
// compile.
// ---------------------------------------------------------------------------

/// Usage that must type-check. Every parameter shape, supplied correctly.
const ACCEPTED: &str = r#"
import { route, routeMethod } from "./routes";
import type { RouteName } from "./routes";

// No parameters, no second argument.
export const home: string = route("home");

// One parameter, as a number and as a string -- both are `string | number`.
export const numeric: string = route("links.show", { link: 42 });
export const textual: string = route("links.show", { link: "arcature" });

// Two parameters, both supplied.
export const nested: string = route("posts.comments.show", { post: 1, comment: 2 });

// A wildcard parameter, named without its star.
export const wildcard: string = route("assets.show", { path: "img/logo.svg" });

export const verb: string = routeMethod("posts.comments.show");
export const named: RouteName = "assets.show";
"#;

/// Usage that must not type-check, one file each so the failures cannot mask
/// one another, with the `tsc` code each one is expected to raise.
///
/// The codes differ because the mistakes are caught at different points, and
/// pinning the real one is what makes this a proof that the mistake is caught
/// rather than a proof that something went wrong somewhere:
///
/// * `TS2554` -- wrong number of arguments. The conditional tuple made the
///   parameter object required, or made it not exist.
/// * `TS2353` -- excess property. An object literal is checked for unknown
///   keys before it is checked against the parameter type, so a misspelling
///   is an excess property before it is a missing one.
/// * `TS2322` -- a known key with the wrong value type.
/// * `TS2345` -- the argument as a whole does not match, which is what is
///   left once no key is excess and none is individually mistyped: a missing
///   key, or a route name outside the union.
const REJECTED: &[(&str, &str, &str)] = &[
    (
        "omitted-parameters",
        // The whole point of the conditional tuple.
        r#"import { route } from "./routes";
export const bad: string = route("links.show");
"#,
        "TS2554",
    ),
    (
        "misspelt-parameter",
        r#"import { route } from "./routes";
export const bad: string = route("links.show", { lnik: 42 });
"#,
        "TS2353",
    ),
    (
        "wrong-parameter-type",
        r#"import { route } from "./routes";
export const bad: string = route("links.show", { link: true });
"#,
        "TS2322",
    ),
    (
        "one-of-two-parameters",
        r#"import { route } from "./routes";
export const bad: string = route("posts.comments.show", { post: 1 });
"#,
        "TS2345",
    ),
    (
        "parameters-on-a-parameterless-route",
        r#"import { route } from "./routes";
export const bad: string = route("home", { link: 42 });
"#,
        "TS2554",
    ),
    (
        "starred-wildcard-key",
        // `{*path}` in the path, `path` in the object. Writing the star is a
        // compile error rather than a runtime "missing parameter".
        r#"import { route } from "./routes";
export const bad: string = route("assets.show", { "*path": "img/logo.svg" });
"#,
        "TS2353",
    ),
    (
        "unknown-route-name",
        r#"import { route } from "./routes";
export const bad: string = route("links.shwo", { link: 42 });
"#,
        "TS2345",
    ),
];

/// How the compiler is spelled: an executable, or a script for `node`.
#[derive(Clone)]
enum Tsc {
    Executable(PathBuf),
    Script(PathBuf),
}

impl Tsc {
    /// A path with no extension -- `node_modules/typescript/bin/tsc` -- is
    /// the compiler's own entry point and needs `node` in front of it.
    fn at(path: PathBuf) -> Self {
        let executable = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| {
                ["exe", "cmd", "bat"]
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            });
        if executable {
            Self::Executable(path)
        } else {
            Self::Script(path)
        }
    }

    fn command(&self) -> Command {
        match self {
            Self::Executable(path) => Command::new(path),
            Self::Script(path) => {
                let mut node = Command::new("node");
                node.arg(path);
                node
            }
        }
    }

    fn answers(&self) -> bool {
        self.command()
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }
}

/// A `tsc` invocation, or `None` when no compiler could be found.
fn compiler() -> Option<Command> {
    if let Some(named) = std::env::var_os(TSC_ENV) {
        let tsc = Tsc::at(PathBuf::from(&named));
        assert!(
            tsc.answers(),
            "{TSC_ENV} is set to `{}`, which did not run",
            Path::new(&named).display()
        );
        return Some(tsc.command());
    }

    // A checkout that happens to have TypeScript installed beside it.
    let vendored =
        Tsc::Script(Path::new(env!("CARGO_MANIFEST_DIR")).join("node_modules/typescript/bin/tsc"));
    if matches!(&vendored, Tsc::Script(path) if path.is_file()) {
        return Some(vendored.command());
    }

    let on_path = Tsc::Executable(PathBuf::from("tsc"));
    on_path.answers().then(|| on_path.command())
}

/// Type-check `files` (relative to `dir`) and return `tsc`'s own output.
fn typecheck(dir: &Path, files: &[String]) -> (bool, String) {
    let mut command = compiler().expect("a compiler was found");
    command
        .current_dir(dir)
        .args(["--noEmit", "--strict", "--target", "es2020"])
        .args(["--module", "commonjs", "--moduleResolution", "node"])
        .args(files);
    let output = command.output().expect("tsc runs");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    (output.status.success(), text)
}

/// Lay the generated file and the fixtures out in a directory of their own.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a temp directory is available");
    fs::write(dir.path().join("routes.ts"), generated()).expect("routes.ts is written");
    fs::write(dir.path().join("accepted.ts"), ACCEPTED).expect("the positive fixture is written");
    for (name, source, _) in REJECTED {
        fs::write(dir.path().join(format!("{name}.ts")), source)
            .expect("the negative fixture is written");
    }
    dir
}

#[test]
fn correct_parameters_type_check() {
    if compiler().is_none() {
        eprintln!("skipped: no TypeScript compiler; set {TSC_ENV} to run this test");
        return;
    }
    let dir = workspace();
    let (ok, output) = typecheck(dir.path(), &["accepted.ts".to_owned()]);
    assert!(ok, "the generated helper rejected correct usage:\n{output}");
}

#[test]
fn an_omitted_misspelt_or_mistyped_parameter_is_a_compile_error() {
    if compiler().is_none() {
        eprintln!("skipped: no TypeScript compiler; set {TSC_ENV} to run this test");
        return;
    }
    let dir = workspace();
    let files: Vec<String> = REJECTED
        .iter()
        .map(|(name, _, _)| format!("{name}.ts"))
        .collect();
    let (ok, output) = typecheck(dir.path(), &files);
    assert!(!ok, "every one of these should have failed:\n{output}");

    // One file at a time: a single failing file would satisfy "the run
    // failed", and six silently-accepted mistakes would go with it.
    //
    // Every case is checked before anything is asserted. Failing on the first
    // mismatch would hide the remaining cases behind it -- the same masking
    // this loop exists to undo, reintroduced one level up.
    let mut wrong = Vec::new();
    for (name, _, code) in REJECTED {
        let (ok, output) = typecheck(dir.path(), &[format!("{name}.ts")]);
        if ok {
            wrong.push(format!("{name}.ts type-checked and should not have"));
        } else if !output.contains(code) {
            wrong.push(format!("{name}.ts failed, but not with {code}:\n{output}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

#[test]
fn the_route_name_union_is_enforced_alongside_the_parameters() {
    if compiler().is_none() {
        eprintln!("skipped: no TypeScript compiler; set {TSC_ENV} to run this test");
        return;
    }
    // Renaming a route in Rust has to break the frontend. This is the same
    // guarantee the scaffold already proved for a parameterless route, run
    // here against a parameterised one so the two cannot drift apart.
    let dir = workspace();
    let renamed = generated().replace("\"links.show\"", "\"links.detail\"");
    fs::write(dir.path().join("routes.ts"), renamed).expect("the renamed table is written");
    let (ok, output) = typecheck(dir.path(), &["accepted.ts".to_owned()]);
    assert!(
        !ok,
        "a renamed route left the frontend compiling:\n{output}"
    );
    assert!(output.contains("TS2345"), "{output}");
}
