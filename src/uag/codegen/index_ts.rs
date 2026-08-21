//! `index.ts` -- the barrel the other three files are reached through.
//!
//! The scaffold's `tsconfig.json` maps `@/generated` to this file and
//! `@/generated/*` to the directory, so an application can write either
//! `import { route } from '@/generated'` or reach a single module directly.
//! Without the barrel the first of those two paths resolves to nothing,
//! which is a broken promise in a config file rather than an error anyone
//! gets told about.
//!
//! It takes no artifact. Every other generator in this module is a function
//! of the graph; this one is a constant, because the *set of modules* does
//! not vary -- `arc typegen` writes exactly three files whatever the
//! application looks like. Keeping it here anyway is what makes it obvious
//! that adding a fourth means adding a line to this string.

use super::GENERATED_HEADER;

/// The barrel's contents.
///
/// `export *` and not a hand-written list of names: the three modules own
/// their own exports, and a barrel that enumerated them would be a fourth
/// place to update whenever one of them grows a type.
///
/// `pages` is re-exported with `export type *` because `pages.d.ts` holds
/// nothing but types. Under `verbatimModuleSyntax` -- which the scaffold
/// turns on -- a plain `export *` from a declaration file is still legal,
/// but the type-only form states the fact and keeps the emitted module
/// graph honest for a bundler reading this file.
#[must_use]
pub fn generate() -> String {
    format!(
        "{GENERATED_HEADER}\n\
         export * from \"./routes\";\n\
         export * from \"./forms\";\n\
         export type * from \"./pages\";\n"
    )
}

#[cfg(test)]
mod tests {
    use super::generate;

    #[test]
    fn it_re_exports_every_generated_module() {
        let out = generate();
        assert!(out.contains("export * from \"./routes\";"), "{out}");
        assert!(out.contains("export * from \"./forms\";"), "{out}");
        assert!(out.contains("export type * from \"./pages\";"), "{out}");
    }

    #[test]
    fn it_carries_the_generated_header() {
        assert!(generate().starts_with(super::GENERATED_HEADER));
    }
}
