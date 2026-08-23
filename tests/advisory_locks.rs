//! Every migrator must hold a lock nobody else holds.
//!
//! Six subsystems ship their own schema and apply it under a database lock so
//! that two processes starting at once cannot run the same `CREATE TABLE`
//! twice. Postgres locks on a number, MySQL on a name, and both are written
//! out as literals in the dialect modules. Nothing but this file checks that
//! two subsystems never pick the same one.
//!
//! They already did once. The notification store and the password-reset store
//! both took `pg_advisory_lock(71420004)` while both doc comments said the key
//! was theirs alone -- the collision cost only a needless wait, but it was
//! invisible, and the comment that should have caught it was the thing that
//! was wrong.
//!
//! The check reads the sources as text rather than importing the constants,
//! because the constants live behind three mutually exclusive driver features
//! and a handful of optional subsystem features: no single build configuration
//! can see all of them at once. Text can.
//!
//! It also discovers the files instead of listing them. A list would have to
//! be edited by the same person who adds the seventh subsystem, which is
//! precisely the edit that gets forgotten.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The first key in the block reserved for Arcature's migrators.
const FIRST_KEY: u64 = 71_420_001;

/// Every `.rs` file under `src/`, in a stable order.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
            .map(|entry| entry.expect("a directory entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().and_then(|it| it.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    assert!(!found.is_empty(), "no sources found under src/");
    found
}

/// The subsystem a dialect module belongs to, as a `/`-joined path relative to
/// `src/`, with the `dialect/<driver>.rs` tail removed.
///
/// `src/auth/flows/reset/dialect/postgres.rs` and its MySQL sibling both
/// answer `auth/flows/reset`, which is what lets the two namespaces be
/// compared against each other.
fn subsystem(path: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let relative = path.strip_prefix(&root).expect("a path under src/");
    let mut parts: Vec<_> = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.pop();
    if parts.last().is_some_and(|last| last == "dialect") {
        parts.pop();
    }
    parts.join("/")
}

/// Each `needle` in `source`, paired with the literal that follows it up to
/// `terminator`.
///
/// Deliberately blunt: the literals are written out in full, one per line, and
/// a parser clever enough to handle a computed key would be a parser that
/// silently ignores the day someone writes one.
fn literals<'a>(source: &'a str, needle: &str, terminator: char) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find(needle) {
        rest = &rest[at + needle.len()..];
        let end = rest
            .find(terminator)
            .unwrap_or_else(|| panic!("unterminated `{needle}` literal"));
        found.push(&rest[..end]);
        rest = &rest[end..];
    }
    found
}

/// Every literal in `source` is the same one, or there is no literal at all.
fn agreed(source: &str, take: &str, release: &str, terminator: char) -> Option<String> {
    let mut all = literals(source, take, terminator);
    all.extend(literals(source, release, terminator));
    let first = all.first()?;
    assert!(
        all.iter().all(|other| other == first),
        "a module takes one lock and releases another: {all:?}"
    );
    Some((*first).to_owned())
}

/// The Postgres key each subsystem takes, and the MySQL name.
fn claimed() -> (BTreeMap<String, u64>, BTreeMap<String, String>) {
    let mut keys: BTreeMap<String, u64> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();

    for path in sources() {
        let source = fs::read_to_string(&path).expect("a readable source file");
        let owner = subsystem(&path);

        if let Some(key) = agreed(&source, "pg_advisory_lock(", "pg_advisory_unlock(", ')') {
            let key = key.parse().expect("an advisory lock key is a number");
            if let Some(previous) = keys.insert(owner.clone(), key) {
                assert_eq!(previous, key, "{owner} takes two different Postgres keys");
            }
        }

        if let Some(name) = agreed(&source, "GET_LOCK('", "RELEASE_LOCK('", '\'')
            && let Some(previous) = names.insert(owner.clone(), name.clone())
        {
            assert_eq!(previous, name, "{owner} takes two different MySQL locks");
        }
    }

    assert!(!keys.is_empty(), "no Postgres advisory locks found at all");
    (keys, names)
}

#[test]
fn no_two_subsystems_take_the_same_postgres_key() {
    let (keys, _) = claimed();
    let mut seen: BTreeMap<u64, &str> = BTreeMap::new();
    for (owner, key) in &keys {
        if let Some(other) = seen.insert(*key, owner) {
            panic!(
                "{other} and {owner} both take pg_advisory_lock({key}). \
                 Give the newer one the next free key and say so in its doc comment."
            );
        }
    }
}

#[test]
fn the_postgres_keys_are_a_block_with_no_gaps() {
    // Not tidiness for its own sake. A gap is the trace of someone picking a
    // number rather than taking the next one, and picking is how two people
    // pick alike.
    let (keys, _) = claimed();
    let mut sorted: Vec<_> = keys.values().copied().collect();
    sorted.sort_unstable();
    sorted.dedup();
    let expected: Vec<_> = (0..sorted.len() as u64).map(|n| FIRST_KEY + n).collect();
    assert_eq!(
        sorted, expected,
        "the advisory lock keys should run from {FIRST_KEY} with no gaps"
    );
}

#[test]
fn no_two_subsystems_take_the_same_mysql_lock() {
    let (_, names) = claimed();
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (owner, name) in &names {
        if let Some(other) = seen.insert(name, owner) {
            panic!("{other} and {owner} both take GET_LOCK('{name}')");
        }
    }
}

#[test]
fn a_subsystem_that_locks_on_one_dialect_locks_on_the_other() {
    // SQLite is absent on purpose -- it serialises writers itself, and every
    // dialect module says so where its `LOCK` would be. Postgres and MySQL
    // both need one, so a subsystem holding exactly one of the two has an
    // unguarded migration on the dialect it forgot.
    let (keys, names) = claimed();
    let postgres: Vec<_> = keys.keys().collect();
    let mysql: Vec<_> = names.keys().collect();
    assert_eq!(
        postgres, mysql,
        "these subsystems lock on one dialect but not the other"
    );
}
