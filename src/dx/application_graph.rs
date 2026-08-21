//! `ApplicationGraph` -- the assembled module dependency graph.
//!
//! Built at compile time from `application!` and `module!` declarations.
//! Contains module descriptors and provides validation: duplicate-module
//! detection, unknown-import detection, and circular-dependency detection.
//! The graph is a plain data structure -- no runtime connection, no trait
//! objects, no `TypeId` -- suitable for side-effect-free inspection
//! (`arc typegen`, `arc build`).

use std::collections::BTreeSet;

use super::graph::{ModuleDescriptor, ModuleNode};

/// A typed error from application-graph validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// Two modules share the same name.
    DuplicateModule { name: &'static str },
    /// A module imports a module that is not registered in the application.
    UnknownImport {
        module: &'static str,
        unknown_import: &'static str,
    },
    /// A circular dependency exists among modules. `cycle` lists the module
    /// names in cycle order (e.g. `[A, B, C, A]`).
    CircularDependency { cycle: Vec<&'static str> },
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateModule { name } => {
                write!(f, "duplicate module `{name}`")
            }
            Self::UnknownImport {
                module,
                unknown_import,
            } => {
                write!(
                    f,
                    "module `{module}` imports unknown module `{unknown_import}`"
                )
            }
            Self::CircularDependency { cycle } => {
                let path = cycle
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                write!(f, "circular dependency: {path}")
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// The assembled application module graph.
///
/// Constructed from a list of [`ModuleDescriptor`]s via
/// [`ApplicationGraph::new`]. Validation (duplicate, unknown-import, and
/// cycle detection) runs at construction time; `new` returns a
/// [`GraphError`] on the first validation failure.
///
/// The graph is side-effect-free: constructing it does not bind HTTP,
/// connect to PostgreSQL/Valkey/SMTP/S3, start workers, or run migrations.
/// It is the foundation for the Unified Application Graph artifact and the
/// validation `arc build` runs over it.
///
/// The graph serializes to `{"modules": [...]}` in declaration order, which
/// is what `UagArtifact` carries and `arc typegen` reads back. Every
/// descriptor field is a `&'static` slice, so the serialized form is a
/// function of the source alone -- two builds of unchanged code produce
/// byte-identical output, and a diff of the UAG is a diff of the
/// application.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplicationGraph {
    modules: Vec<ModuleDescriptor>,
}

impl ApplicationGraph {
    /// Creates a new application graph from the given module descriptors,
    /// validating for duplicates, unknown imports, and cycles. Returns the
    /// first validation error encountered.
    pub fn new(modules: Vec<ModuleDescriptor>) -> Result<Self, GraphError> {
        check_duplicates(&modules)?;
        check_unknown_imports(&modules)?;
        check_cycles(&modules)?;
        Ok(Self { modules })
    }

    /// Creates a graph without validation. For internal/testing use where
    /// the caller has already validated the modules.
    pub fn new_unchecked(modules: Vec<ModuleDescriptor>) -> Self {
        Self { modules }
    }

    /// Returns the module descriptors in this graph.
    pub fn modules(&self) -> &[ModuleDescriptor] {
        &self.modules
    }

    /// Returns the module nodes (name + import edges) for graph traversal.
    pub fn nodes(&self) -> Vec<ModuleNode> {
        self.modules.iter().map(ModuleNode::from).collect()
    }

    /// Returns the module descriptor with the given name, if present.
    pub fn find(&self, name: &str) -> Option<&ModuleDescriptor> {
        self.modules.iter().find(|m| m.name == name)
    }

    /// Returns the number of modules in the graph.
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Returns true if the graph has no modules.
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}

fn check_duplicates(modules: &[ModuleDescriptor]) -> Result<(), GraphError> {
    let mut seen = BTreeSet::new();
    for m in modules {
        if !seen.insert(m.name) {
            return Err(GraphError::DuplicateModule { name: m.name });
        }
    }
    Ok(())
}

fn check_unknown_imports(modules: &[ModuleDescriptor]) -> Result<(), GraphError> {
    let known: BTreeSet<&'static str> = modules.iter().map(|m| m.name).collect();
    for m in modules {
        for &import in m.imports {
            if !known.contains(import) {
                return Err(GraphError::UnknownImport {
                    module: m.name,
                    unknown_import: import,
                });
            }
        }
    }
    Ok(())
}

/// Detects circular dependencies among modules using depth-first search.
/// Returns the cycle path if a cycle is found, or Ok(()) if the graph is
/// acyclic.
fn check_cycles(modules: &[ModuleDescriptor]) -> Result<(), GraphError> {
    let node_map = super::graph::module_node_map(modules);
    let mut visited: BTreeSet<&'static str> = BTreeSet::new();
    let mut stack: Vec<&'static str> = Vec::new();
    let mut on_stack: BTreeSet<&'static str> = BTreeSet::new();

    for m in modules {
        if visited.contains(m.name) {
            continue;
        }
        if let Some(cycle) =
            dfs_find_cycle(m.name, &node_map, &mut visited, &mut stack, &mut on_stack)
        {
            return Err(GraphError::CircularDependency { cycle });
        }
    }
    Ok(())
}

/// Recursive DFS cycle detection. Returns the cycle path if a back-edge
/// is found.
fn dfs_find_cycle(
    node: &'static str,
    node_map: &std::collections::BTreeMap<&'static str, ModuleNode>,
    visited: &mut BTreeSet<&'static str>,
    stack: &mut Vec<&'static str>,
    on_stack: &mut BTreeSet<&'static str>,
) -> Option<Vec<&'static str>> {
    visited.insert(node);
    stack.push(node);
    on_stack.insert(node);

    if let Some(n) = node_map.get(node) {
        for &import in &n.imports {
            if !visited.contains(import) {
                if let Some(cycle) = dfs_find_cycle(import, node_map, visited, stack, on_stack) {
                    return Some(cycle);
                }
            } else if on_stack.contains(import) {
                // Found a back-edge: extract the cycle from the stack.
                let cycle_start = stack.iter().position(|&s| s == import).unwrap();
                let mut cycle: Vec<&'static str> = stack[cycle_start..].to_vec();
                cycle.push(import); // close the cycle
                return Some(cycle);
            }
        }
    }

    stack.pop();
    on_stack.remove(node);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_desc(name: &'static str, imports: &'static [&'static str]) -> ModuleDescriptor {
        ModuleDescriptor {
            name,
            imports,
            exports: &[],
            controllers: &[],
            controller_methods: &[],
            services: &[],
            policies: &[],
            routes: &[],
            listeners: &[],
            jobs: &[],
            commands: &[],
            schedules: &[],
            pages: &[],
        }
    }

    #[test]
    fn a_graph_serializes_its_modules_in_declaration_order() {
        let graph =
            ApplicationGraph::new(vec![mod_desc("Zeta", &[]), mod_desc("Alpha", &["Zeta"])])
                .unwrap();
        let json = serde_json::to_string(&graph).expect("the graph is plain data");
        assert!(
            json.starts_with("{\"modules\":[{\"name\":\"Zeta\""),
            "got: {json}"
        );
        assert!(json.contains("\"pages\":[]"), "got: {json}");
    }

    #[test]
    fn serializing_the_same_graph_twice_yields_the_same_bytes() {
        let graph = ApplicationGraph::new(vec![mod_desc("Accounts", &[])]).unwrap();
        assert_eq!(
            serde_json::to_string(&graph).unwrap(),
            serde_json::to_string(&graph).unwrap()
        );
    }

    #[test]
    fn empty_graph_is_valid() {
        let g = ApplicationGraph::new(vec![]).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn single_module_is_valid() {
        let g = ApplicationGraph::new(vec![mod_desc("Accounts", &[])]).unwrap();
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn duplicate_module_is_error() {
        let err = ApplicationGraph::new(vec![mod_desc("Accounts", &[]), mod_desc("Accounts", &[])])
            .unwrap_err();
        assert_eq!(err, GraphError::DuplicateModule { name: "Accounts" });
    }

    #[test]
    fn unknown_import_is_error() {
        let err = ApplicationGraph::new(vec![mod_desc("Billing", &["Nonexistent"])]).unwrap_err();
        assert_eq!(
            err,
            GraphError::UnknownImport {
                module: "Billing",
                unknown_import: "Nonexistent"
            }
        );
    }

    #[test]
    fn linear_dependency_is_valid() {
        let g = ApplicationGraph::new(vec![
            mod_desc("Accounts", &[]),
            mod_desc("Billing", &["Accounts"]),
            mod_desc("Checkout", &["Billing"]),
        ])
        .unwrap();
        assert_eq!(g.len(), 3);
    }

    #[test]
    fn two_module_cycle_is_detected() {
        let err =
            ApplicationGraph::new(vec![mod_desc("A", &["B"]), mod_desc("B", &["A"])]).unwrap_err();
        match err {
            GraphError::CircularDependency { cycle } => {
                assert!(
                    cycle.len() >= 3,
                    "cycle should have at least 3 nodes: {cycle:?}"
                );
                assert_eq!(
                    cycle.first(),
                    cycle.last(),
                    "cycle should be closed: {cycle:?}"
                );
            }
            other => panic!("expected CircularDependency, got {other:?}"),
        }
    }

    #[test]
    fn three_module_cycle_is_detected() {
        let err = ApplicationGraph::new(vec![
            mod_desc("A", &["B"]),
            mod_desc("B", &["C"]),
            mod_desc("C", &["A"]),
        ])
        .unwrap_err();
        assert!(matches!(err, GraphError::CircularDependency { .. }));
    }

    #[test]
    fn find_module_by_name() {
        let g = ApplicationGraph::new(vec![mod_desc("Accounts", &[])]).unwrap();
        assert!(g.find("Accounts").is_some());
        assert!(g.find("Nonexistent").is_none());
    }

    #[test]
    fn graph_error_display() {
        let err = GraphError::DuplicateModule { name: "Foo" };
        assert_eq!(err.to_string(), "duplicate module `Foo`");
    }
}
