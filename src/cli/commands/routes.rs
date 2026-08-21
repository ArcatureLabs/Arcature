//! `arc routes` -- every route the application declares, as a table or as
//! JSON.
//!
//! This is a pure read of the application graph. It starts nothing, connects
//! to nothing, and changes nothing; the only side effect is the output. That
//! is what makes it the command to reach for when the question is "did that
//! route actually register" -- the answer comes from the same artifact
//! `arc typegen` and the OpenAPI document are built from, so the three can
//! never disagree.
//!
//! # Why a policies column
//!
//! A route's `policy:`/`policies:` declaration is metadata, not enforcement:
//! it records which policy the handler is expected to call. A column here is
//! how a reviewer sees, in one screen, which routes claim a guard and which
//! do not -- a `POST /admin/users` with an empty policies cell is the thing
//! worth noticing, and it is invisible in the source unless you open every
//! controller.
//!
//! # Why the table is formatted by hand
//!
//! Column alignment over five columns is a `max()` and a `{:width$}`. A table
//! crate would be a dependency in the shipped `arc` binary, and a build-time
//! cost for every application that installs it, in exchange for code that is
//! shorter to write than to justify.

use std::collections::BTreeSet;

use crate::uag::{UagArtifact, UagRoute};

use super::Cause;
use super::uag_source;

/// Print the route table.
///
/// # Errors
///
/// [`RoutesError`] when the application graph cannot be obtained, or when the
/// JSON form cannot be serialized.
pub fn run(json: bool) -> Result<(), RoutesError> {
    let cwd = std::env::current_dir().map_err(|source| RoutesError::Cwd { source })?;
    let loaded = uag_source::load(&cwd).map_err(|source| RoutesError::Source {
        source: Box::new(source),
    })?;

    if json {
        let rendered =
            render_json(&loaded.artifact).map_err(|source| RoutesError::Json { source })?;
        println!("{rendered}");
    } else {
        print!("{}", render_table(&loaded.artifact));
    }
    Ok(())
}

/// The column headings, in order.
///
/// A `const` rather than a literal in the formatting loop because the widths
/// are computed from them: a heading is the minimum width of its column.
const HEADINGS: [&str; 5] = ["METHOD", "PATH", "NAME", "HANDLER", "POLICIES"];

/// What an empty cell prints as.
///
/// A dash rather than nothing: a blank cell in a monospaced table reads as
/// "the column ran out", and an unnamed route or an unguarded one is a fact,
/// not a gap.
const EMPTY: &str = "-";

/// Render the human table, including its trailing newline.
///
/// Returns a `String` rather than printing so the formatting can be tested
/// without capturing stdout.
#[must_use]
fn render_table(artifact: &UagArtifact) -> String {
    let routes = artifact.routes();
    if routes.is_empty() {
        return String::from(
            "No routes declared. A route reaches this table by being in a `routes!` \
             block that a `module!` names in its `routes:` section.\n",
        );
    }

    let rows: Vec<[String; 5]> = routes.iter().map(row).collect();
    let mut widths = HEADINGS.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut out = String::new();
    write_row(&mut out, &widths, &HEADINGS.map(String::from));
    for row in &rows {
        write_row(&mut out, &widths, row);
    }
    out.push_str(&format!(
        "\n{} route{}\n",
        routes.len(),
        if routes.len() == 1 { "" } else { "s" }
    ));
    out
}

/// One route as its five cells.
fn row(route: &UagRoute) -> [String; 5] {
    [
        route.method.clone(),
        route.path.clone(),
        or_empty(&route.name),
        route.handler.clone(),
        join(&route.policies),
    ]
}

/// Append one padded row. The last column is not padded: trailing spaces on
/// every line would show up in a diff and in a copy-paste.
fn write_row(out: &mut String, widths: &[usize; 5], cells: &[String; 5]) {
    let last = cells.len() - 1;
    for (index, cell) in cells.iter().enumerate() {
        if index == last {
            out.push_str(cell);
        } else {
            let pad = widths[index].saturating_sub(cell.chars().count());
            out.push_str(cell);
            out.push_str(&" ".repeat(pad + 2));
        }
    }
    out.push('\n');
}

/// A value, or the empty marker.
fn or_empty(value: &str) -> String {
    if value.is_empty() {
        String::from(EMPTY)
    } else {
        value.to_owned()
    }
}

/// A sorted set as one comma-separated cell.
fn join(values: &BTreeSet<String>) -> String {
    if values.is_empty() {
        return String::from(EMPTY);
    }
    values.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// One route in the `--json` output.
///
/// A named struct rather than re-serializing [`UagRoute`]: `--json` is a
/// stable interface for a script, and it should carry the columns the table
/// shows plus the identifiers a script needs, not every field the artifact
/// happens to hold today. `arc typegen` is the way to consume the whole
/// artifact.
#[derive(Debug, serde::Serialize)]
struct JsonRoute<'a> {
    /// The uppercase HTTP method.
    method: &'a str,
    /// The full path pattern.
    path: &'a str,
    /// The dotted route name, or `null` when the route is unnamed.
    name: Option<&'a str>,
    /// The handler path.
    handler: &'a str,
    /// The module that declared the route.
    module: &'a str,
    /// The path parameter names, in path order.
    params: &'a [String],
    /// The page identities the route renders, sorted.
    pages: &'a BTreeSet<String>,
    /// The policies the route declares, sorted.
    policies: &'a BTreeSet<String>,
}

/// Render the `--json` form: one array, in the artifact's own route order.
///
/// # Errors
///
/// `serde_json::Error` if the rows cannot be serialized, which would mean a
/// bug in this module rather than bad input.
fn render_json(artifact: &UagArtifact) -> Result<String, serde_json::Error> {
    let rows: Vec<JsonRoute<'_>> = artifact
        .routes()
        .iter()
        .map(|route| JsonRoute {
            method: &route.method,
            path: &route.path,
            // `null`, not `""`: a script asking "is this route named" should
            // not have to know that the artifact spells absence as empty.
            name: (!route.name.is_empty()).then_some(route.name.as_str()),
            handler: &route.handler,
            module: &route.module,
            params: &route.params,
            pages: &route.pages,
            policies: &route.policies,
        })
        .collect();
    serde_json::to_string_pretty(&rows)
}

/// A failure listing routes.
#[derive(Debug)]
pub enum RoutesError {
    /// The working directory could not be read.
    Cwd {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The application graph could not be obtained.
    Source {
        /// Why. Boxed to keep this enum small.
        source: Cause,
    },
    /// The route rows could not be serialized.
    Json {
        /// The serialization failure.
        source: serde_json::Error,
    },
}

impl std::fmt::Display for RoutesError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cwd { source } => {
                write!(formatter, "could not read the working directory: {source}")
            }
            Self::Source { source } => write!(formatter, "{source}"),
            Self::Json { source } => write!(formatter, "could not render the route list: {source}"),
        }
    }
}

impl std::error::Error for RoutesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cwd { source } => Some(source),
            Self::Source { source } => Some(source.as_ref()),
            Self::Json { source } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn route(method: &str, path: &str, name: &str, handler: &str, policies: &[&str]) -> UagRoute {
        UagRoute {
            module: String::from("Web"),
            method: method.to_owned(),
            path: path.to_owned(),
            name: name.to_owned(),
            handler: handler.to_owned(),
            params: Vec::new(),
            pages: BTreeSet::new(),
            action: None,
            query: None,
            query_string: None,
            policies: policies.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn artifact(routes: Vec<UagRoute>) -> UagArtifact {
        UagArtifact::new(BTreeMap::new(), routes, BTreeMap::new())
    }

    #[test]
    fn every_column_is_padded_to_its_widest_cell() {
        let table = render_table(&artifact(vec![
            route("GET", "/", "home", "HomeController::index", &[]),
            route(
                "DELETE",
                "/links/{link}",
                "links.destroy",
                "LinksController::destroy",
                &["LinkPolicy"],
            ),
        ]));

        let lines: Vec<&str> = table.lines().collect();
        // Heading, two routes, a blank line, the count.
        assert_eq!(lines.len(), 5, "got:\n{table}");
        // `DELETE` is the widest method and `PATH` starts two spaces after it.
        let name_column = lines[0].find("NAME").expect("the heading is present");
        for line in &lines[1..3] {
            let cell: String = line.chars().skip(name_column).collect();
            assert!(
                cell.starts_with("home") || cell.starts_with("links.destroy"),
                "the name column should start at {name_column} on every row, got: {line}"
            );
        }
    }

    #[test]
    fn a_route_that_declares_a_policy_shows_it_and_one_that_does_not_shows_a_dash() {
        let table = render_table(&artifact(vec![
            route("GET", "/a", "a", "A::a", &[]),
            route("POST", "/b", "b", "B::b", &["Admin", "Owner"]),
        ]));
        assert!(table.contains("Admin, Owner"), "got:\n{table}");
        let unguarded = table
            .lines()
            .find(|line| line.starts_with("GET"))
            .expect("the GET row is present");
        assert!(unguarded.ends_with('-'), "got: {unguarded}");
    }

    #[test]
    fn an_unnamed_route_prints_a_dash_rather_than_an_empty_cell() {
        let table = render_table(&artifact(vec![route("GET", "/health", "", "H::h", &[])]));
        let row = table
            .lines()
            .find(|line| line.starts_with("GET"))
            .expect("the row is present");
        assert!(row.contains(" -  "), "got: {row}");
    }

    #[test]
    fn an_application_with_no_routes_says_so_rather_than_printing_a_bare_heading() {
        let table = render_table(&artifact(Vec::new()));
        assert!(table.starts_with("No routes declared."), "got: {table}");
    }

    #[test]
    fn the_json_form_is_an_array_with_the_table_columns_and_a_null_for_an_unnamed_route() {
        let mut named = route("GET", "/links/{link}", "links.show", "L::show", &["View"]);
        named.params = vec![String::from("link")];
        named.pages = ["Links/Show".to_owned()].into_iter().collect();
        let rendered =
            render_json(&artifact(vec![named, route("GET", "/x", "", "X::x", &[])])).expect("json");

        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid json");
        let rows = parsed.as_array().expect("an array");
        assert_eq!(rows.len(), 2);

        // Artifact order is by path, so `/links/{link}` comes first.
        let first = &rows[0];
        assert_eq!(first["method"], "GET");
        assert_eq!(first["path"], "/links/{link}");
        assert_eq!(first["name"], "links.show");
        assert_eq!(first["handler"], "L::show");
        assert_eq!(first["module"], "Web");
        assert_eq!(first["params"], serde_json::json!(["link"]));
        assert_eq!(first["pages"], serde_json::json!(["Links/Show"]));
        assert_eq!(first["policies"], serde_json::json!(["View"]));

        assert_eq!(rows[1]["name"], serde_json::Value::Null);
        assert_eq!(rows[1]["policies"], serde_json::json!([]));
    }
}
