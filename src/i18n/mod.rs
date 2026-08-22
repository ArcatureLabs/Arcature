//! Localization: Fluent translation catalogs.
//!
//! ```
//! use arcature::i18n::{Catalog, Catalogs, LocaleId, TranslationArgs};
//!
//! // In a real application these are `include_str!("../locales/en.ftl")`.
//! let english = Catalog::parse(
//!     LocaleId::parse("en").unwrap(),
//!     "welcome = Welcome back, { $name }.",
//! )
//! .unwrap()
//! .isolating(false);
//!
//! let french = Catalog::parse(
//!     LocaleId::parse("fr").unwrap(),
//!     "welcome = Bon retour, { $name }.",
//! )
//! .unwrap()
//! .isolating(false);
//!
//! let catalogs = Catalogs::new(english).with(french);
//!
//! let args = TranslationArgs::new().with("name", "Ada");
//! let fr = LocaleId::parse("fr").unwrap();
//! assert_eq!(
//!     catalogs.translate(&fr, "welcome", &args).unwrap(),
//!     "Bon retour, Ada."
//! );
//! ```
//!
//! # Why Fluent and not a map
//!
//! The obvious implementation of translation is `HashMap<String, String>`
//! keyed by locale, and it is wrong for every language whose grammar is not
//! English's.
//!
//! It is wrong about **plurals**: English has two categories, so a map with a
//! `_one` and a `_other` key looks complete. Polish has four, Arabic six, and
//! Japanese one. Selecting between them is not a `if n == 1` the calling code
//! can write, because the calling code does not know which language it is
//! rendering, and the rule for the language it is rendering is a table from
//! CLDR rather than an arithmetic expression a developer can guess.
//!
//! It is wrong about **agreement**: "the file was deleted" has a gendered
//! participle in French and Russian, so the correct string depends on a
//! property of an argument, not only on the key.
//!
//! It is wrong about **numbers and dates**: `1,234.5` is `1 234,5` in French
//! and `1.234,5` in German, and a value formatted before it reaches the map
//! is formatted in the server's locale rather than the reader's.
//!
//! [Mozilla Fluent] gets all three right by putting the decision inside the
//! catalog, where the translator can see it and change it, instead of inside
//! the calling code, where they cannot. A message selects its own plural
//! form, and adding a language with four categories is an edit to one `.ftl`
//! file and to nothing else.
//!
//! [Mozilla Fluent]: https://projectfluent.org/
//!
//! # The runtime parser, and why it is acceptable here
//!
//! Arcature's view layer chose askama specifically so that no template parser
//! runs inside the request path -- see `src/view/mod.rs`, which is blunt
//! about server-side template injection being the shortest route from a form
//! field to remote code execution. This module then adds a runtime parser.
//! That is a real tension and it deserves a real answer rather than a
//! footnote.
//!
//! The answer is that the two parsers eat different food.
//!
//! **A catalog is developer-authored and lives in the repository.** The
//! `.ftl` text passed to [`Catalog::parse`] is a file a translator wrote and
//! a reviewer merged. It is not attacker input, it does not arrive over the
//! network, and it is not selected by anything a request controls. In the
//! intended use it is `include_str!`, which means the bytes are in the binary
//! and the parse is a startup cost, not a per-request one.
//!
//! **A request supplies arguments, not messages.** Values from a request
//! reach Fluent as [`ArgValue`]s -- a string, an integer, a float -- and
//! Fluent interpolates them. It does not evaluate them: there is no path by
//! which a `$name` of `{ $other }` becomes a placeable, because the message's
//! pattern was fixed when the catalog was parsed and an argument is
//! substituted into that pattern rather than re-parsed with it. Fluent has no
//! function calls a catalog can invoke beyond `NUMBER` and `DATETIME`, no
//! filesystem access, no property lookup on host objects, and no `eval`. The
//! machinery an SSTI payload needs is not there to reach.
//!
//! What follows is a rule this module holds itself to, and the thing to check
//! in review: **never call [`Catalog::parse`] on bytes that came from a
//! request.** A feature that let an administrator upload a `.ftl` file, or
//! that read a catalog out of a database row, would put attacker-influenced
//! text into the parser and would need its own analysis. Nothing here does
//! that, and nothing here offers a way to.
//!
//! # Locales are matched, never resolved into a path
//!
//! The other half of the boundary. A locale tag arrives from an
//! `Accept-Language` header, a query parameter or a session -- all attacker-
//! reachable -- and the classic way to lose is to turn it into
//! `locales/{tag}.ftl` and open it. `../../etc/passwd` and `..\..\..\windows\
//! win.ini` are then one request away.
//!
//! This module never reads a file. Catalogs are values the application
//! constructs and hands over, the registry is an in-memory
//! [`BTreeMap`](std::collections::BTreeMap) built at startup, and lookup is a
//! map lookup against [`Catalogs::contains`] -- a whitelist test whose entries
//! came from the application's own source. There is no filesystem path for a
//! hostile tag to traverse because there is no filesystem access at all.
//!
//! That is the belt. [`LocaleId`] is the braces: it is the only locale type
//! the API accepts, its only constructor validates, and it refuses anything
//! that is not a canonical BCP-47 language identifier of at most 35 bytes. A
//! request's raw string cannot be passed where a locale is expected without
//! going through it.
//!
//! # `unsafe` in the dependency tree
//!
//! `arcature` is `#![forbid(unsafe_code)]`. Its dependencies are not, and
//! `.github/SECURITY.md` is where the project keeps that honest rather than
//! quiet. Two facts about this feature belong in the same place.
//!
//! **`fluent-bundle` pulls in `self_cell`, which contains `unsafe`.**
//! `self_cell` is how `FluentResource` holds a `String` of `.ftl` source
//! together with an AST that borrows from it -- a self-referential struct,
//! which safe Rust cannot express, so the crate builds one with a small
//! amount of `unsafe` and a well-known soundness argument. It is not
//! incidental: it is the reason parsing a catalog does not copy every string
//! out of the source. Enabling `i18n` accepts that. The rest of the subtree
//! -- `fluent-syntax`, `fluent-langneg`, `intl-memoizer`, `intl_pluralrules`,
//! `unic-langid`, `type-map`, `rustc-hash`, `smallvec` -- is pure Rust with
//! no C and no network or filesystem access.
//!
//! **The `cargo geiger` baseline does not change, and that is not a claim
//! that nothing was added.** `unsafe-baseline.<host-target>.txt` is recorded
//! over the *default* feature set, and `i18n` is not in `default`, so
//! `self_cell` is outside the graph the baseline measures and the file is
//! byte-identical after this change. A reader who expected the number to move
//! should know why it did not, rather than conclude the dependency is free.
//! An application that turns `i18n` on takes on `self_cell`'s `unsafe`, and
//! that is not visible in the recorded numbers.
//!
//! # What this module does not do
//!
//! Locale negotiation -- deciding *which* registered locale a given request
//! is in -- is not here. This module owns the catalogs and the whitelist;
//! choosing from that whitelist is a separate concern with its own security
//! properties.

mod args;
mod catalog;
mod error;
mod locale;

pub use args::{ArgValue, TranslationArgs};
pub use catalog::{Catalog, Catalogs};
pub use error::{I18nError, LocaleRejection};
pub use locale::LocaleId;
