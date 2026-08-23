//! What each `arc make:<kind>` writes, and where.
//!
//! One function per kind would spread twenty-two near-identical decisions
//! across twenty-two places, so instead [`plan`] answers all of them: the
//! destination path, the file body, whether a sibling `mod.rs` should learn
//! about the new file, and any follow-up the generator cannot do for the
//! developer.
//!
//! # Scaffolds that compile
//!
//! Every blueprint here produces a file that compiles as written, with two
//! kinds of exception.
//!
//! `policy` and `listener` bind to a type the developer chooses -- the model
//! a policy guards, the event a listener reacts to -- and no generator can
//! guess it. Those two name a placeholder and say so at the top of the file,
//! because a scaffold that silently omits the binding is worse than one that
//! fails to compile until it is pointed somewhere real.
//!
//! `notification`, `upload` and `auth` compile only once the application
//! enables the features their imports live behind, which a fresh `arc new`
//! does not. The alternative was to have the generator edit `Cargo.toml`, and
//! a generator that reaches into the manifest is a generator that can break a
//! build it was never pointed at. Each artifact's notes say which feature and
//! why, and adding it is one line.
//!
//! `auth` is also the one kind that does not scaffold its own screens. What a
//! login form looks like is an application's answer, and a generator that
//! guesses Inertia or a server-rendered view is wrong for half of them; what a
//! login *handler* does is not, and getting it wrong leaks who has an account.
//! So the dangerous half is written and the visible half is left to
//! `arc make:page` and `arc make:view`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::name::{ArtifactName, pluralize, to_pascal_case};
use crate::cli::parser::MakeKind;

/// One file a generator is about to write, plus what to do around it.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// Where the file goes, relative to the project root.
    pub path: PathBuf,
    /// The rendered file body.
    pub contents: String,
    /// Whether the sibling `mod.rs` should gain a `pub mod` line. False only
    /// for `tests/`, where each file is its own crate and no `mod.rs` exists.
    pub register_module: bool,
    /// Follow-up the generator deliberately left to the developer.
    pub notes: Vec<String>,
}

/// Decide everything about the file `kind` + `name` produces.
///
/// For a kind that writes more than one file this is the *primary* artifact
/// -- the one whose path names the thing. Use [`plan_all`] to get the rest.
#[must_use]
pub fn plan(kind: MakeKind, name: &ArtifactName) -> Artifact {
    match kind {
        MakeKind::Module => module_root(name),
        MakeKind::Controller => rust(name, "app/controllers", "Controller", controller),
        MakeKind::Model => rust(name, "app/models", "", model),
        MakeKind::Migration => migration(name),
        MakeKind::Request => rust(name, "app/requests", "Request", request),
        MakeKind::Resource => rust(name, "app/resources", "Resource", resource),
        MakeKind::Policy => policy(name),
        MakeKind::Service => rust(name, "app/services", "Service", service),
        MakeKind::Job => rust(name, "app/jobs", "", job),
        MakeKind::Event => rust(name, "app/events", "", event),
        MakeKind::Listener => listener(name),
        MakeKind::Middleware => rust(name, "app/middleware", "", middleware),
        MakeKind::Command => rust(name, "app/commands", "", command),
        MakeKind::Page => rust(name, "app/pages", "Page", page),
        MakeKind::Test => test(name),
        MakeKind::Factory => rust(name, "database/factories", "Factory", factory),
        MakeKind::Seeder => rust(name, "database/seeders", "Seeder", seeder),
        MakeKind::Notification => notification(name),
        MakeKind::Mail => rust(name, "app/mail", "", mailable),
        MakeKind::View => rust(name, "app/views", "View", view_struct),
        MakeKind::Upload => upload(name),
        MakeKind::Auth => account(name),
    }
}

/// Every file `kind` + `name` produces, primary artifact first.
///
/// Nineteen of the twenty-two kinds write one file, and [`plan`] is the older,
/// narrower way to ask for one of those. Three kinds are not one file, for the
/// same underlying reason: what they produce is not a file, it is a set of
/// files that only means anything together.
///
/// A module is a directory whose entire point is that the controller, the
/// service and the routes sit together, and a directory holding one of the
/// three is not a module. A view is a struct and the template it is the type
/// of, and askama reads that template when the crate compiles -- so a view
/// without its template is not a half-finished scaffold, it is a build
/// failure. And a sign-in flow is an account, the handlers that read and
/// write it, and the table it lives in -- a registration handler that does
/// not know how the login handler compares passwords stores a hash nothing
/// can verify.
///
/// A second function rather than a `Vec<Artifact>` field on [`Artifact`] --
/// or a second path on [`super::Generated`] -- because both of those structs
/// are public API and neither is `#[non_exhaustive]`, so growing either a
/// field is a breaking change. Growing a module a function is not.
#[must_use]
pub fn plan_all(kind: MakeKind, name: &ArtifactName) -> Vec<Artifact> {
    match kind {
        MakeKind::Module => module(name),
        MakeKind::View => view(name),
        MakeKind::Auth => auth(name),
        _ => vec![plan(kind, name)],
    }
}

/// The shape almost every kind shares: `<root>/<segments>/<stem>.rs`, a
/// `mod.rs` registration, and no follow-up.
fn rust(
    name: &ArtifactName,
    root: &str,
    suffix: &str,
    render: fn(&Rendered) -> String,
) -> Artifact {
    let rendered = Rendered::new(name, suffix);
    Artifact {
        path: destination(root, name, &rendered.stem),
        contents: render(&rendered),
        register_module: true,
        notes: Vec::new(),
    }
}

/// `<root>/<segments...>/<stem>.rs`.
fn destination(root: &str, name: &ArtifactName, stem: &str) -> PathBuf {
    let mut path = PathBuf::from(root);
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(format!("{stem}.rs"));
    path
}

/// The handful of strings a blueprint interpolates, computed once so a
/// template body reads as a template and not as string plumbing.
pub struct Rendered {
    /// The snake_case file stem (no extension).
    pub stem: String,
    /// The PascalCase type name.
    pub type_name: String,
    /// The name as a `/`-joined path, for page contracts and doc lines.
    pub slash_path: String,
    /// The base PascalCase name with the kind suffix removed (`UserPolicy`
    /// -> `User`), which is the model or event a binding points at.
    pub base_type: String,
    /// The base name in snake_case, for a sibling module path.
    pub base_stem: String,
}

impl Rendered {
    fn new(name: &ArtifactName, suffix: &str) -> Self {
        let base_stem = name.file_stem("");
        Self {
            stem: name.file_stem(suffix),
            type_name: name.type_name(suffix),
            slash_path: name.slash_path(),
            base_type: to_pascal_case(&base_stem),
            base_stem,
        }
    }
}

// ---------------------------------------------------------------------------
// The blueprints.
// ---------------------------------------------------------------------------

fn controller(r: &Rendered) -> String {
    let Rendered {
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! The `{type_name}`: HTTP entry points for `{slash_path}`.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// The `{slash_path}` controller.\n\
         pub struct {type_name};\n\
         \n\
         #[controller]\n\
         impl {type_name} {{\n\
         \x20   /// The index action. Register it in `routes/mod.rs`, then\n\
         \x20   /// replace this body with the real response.\n\
         \x20   pub async fn index() -> Result<Response> {{\n\
         \x20       Ok(text(StatusCode::OK, \"{type_name}::index\"))\n\
         \x20   }}\n\
         }}\n"
    )
}

fn model(r: &Rendered) -> String {
    let Rendered {
        type_name,
        base_stem,
        ..
    } = r;
    let table = pluralize(base_stem);
    format!(
        "//! The `{type_name}` model: one row of `{table}`.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// A `{table}` row.\n\
         #[model(table = \"{table}\")]\n\
         pub struct {type_name} {{\n\
         \x20   #[sea_orm(primary_key)]\n\
         \x20   pub id: i64,\n\
         }}\n"
    )
}

fn migration(name: &ArtifactName) -> Artifact {
    let stem = name.file_stem("");
    let module = format!("m{}_{stem}", utc_stamp());
    let mut path = PathBuf::from("database/migrations");
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(format!("{module}.rs"));

    let contents = format!(
        "//! Migration `{stem}`.\n\
         //!\n\
         //! `up` runs on `arc migrate`. `down` has to undo exactly what `up`\n\
         //! did -- a rollback that leaves the schema in a state no migration\n\
         //! describes is worse than no rollback at all.\n\
         \n\
         // `DeriveMigrationName` expands to a path relative to a crate\n\
         // named `sea_orm_migration`, and the prelude carries names out\n\
         // of that crate rather than the crate itself.\n\
         use arcature::database::sea_orm_migration;\n\
         use arcature::database::sea_orm_migration::prelude::*;\n\
         \n\
         /// The `{stem}` schema change.\n\
         #[derive(DeriveMigrationName)]\n\
         pub struct Migration;\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigrationTrait for Migration {{\n\
         \x20   async fn up(&self, _manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       todo!(\"describe the schema change for `{stem}`\")\n\
         \x20   }}\n\
         \n\
         \x20   async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       todo!(\"undo the schema change for `{stem}`\")\n\
         \x20   }}\n\
         }}\n"
    );

    Artifact {
        path,
        contents,
        register_module: true,
        notes: vec![format!(
            "add `Box::new({module}::Migration)` to `Migrator::migrations()` \
             in database/migrations/mod.rs -- ordering is yours to choose, so \
             the generator does not guess it"
        )],
    }
}

fn request(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}` payload.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// A validated request body. `#[request]` adds `Validate`; the\n\
         /// `Deserialize` derive stays explicit so the extractor can\n\
         /// deserialize and validate in one step.\n\
         #[request]\n\
         #[derive(Debug, Clone, Deserialize)]\n\
         pub struct {type_name} {{\n\
         \x20   #[validate(length(min = 1, max = 255))]\n\
         \x20   pub name: String,\n\
         }}\n"
    )
}

fn resource(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}`: the JSON shape the client sees.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// A browser-safe projection. Convert from the model explicitly\n\
         /// (`impl From<Model> for {type_name}`) so the database schema can\n\
         /// change without breaking the API.\n\
         #[resource]\n\
         pub struct {type_name} {{\n\
         \x20   pub id: String,\n\
         \x20   pub name: String,\n\
         }}\n"
    )
}

fn policy(name: &ArtifactName) -> Artifact {
    let r = Rendered::new(name, "Policy");
    let Rendered {
        type_name,
        base_type,
        base_stem,
        ..
    } = &r;

    let contents = format!(
        "//! The `{type_name}` authorization policy.\n\
         //!\n\
         //! `#[policy(M)]` records *which* model this policy guards; the\n\
         //! `Policy<M>` impl below is the decision itself, and no macro can\n\
         //! guess it. Point the import, the attribute, and `type User` at\n\
         //! real types -- until then this file names `{base_type}` and a user\n\
         //! model that may not exist yet.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         use crate::app::models::{base_stem}::{base_type};\n\
         use crate::app::models::user::User;\n\
         \n\
         /// Authorization decisions for `{base_type}`.\n\
         #[policy({base_type})]\n\
         pub struct {type_name};\n\
         \n\
         impl Policy<{base_type}> for {type_name} {{\n\
         \x20   type User = User;\n\
         \n\
         \x20   fn check(_user: &Self::User, action: &str, _resource: &{base_type}) -> bool {{\n\
         \x20       matches!(action, \"view\")\n\
         \x20   }}\n\
         }}\n"
    );

    Artifact {
        path: destination("app/policies", name, &r.stem),
        contents,
        register_module: true,
        notes: vec![format!(
            "{} names `{base_type}` and `User`; point them at the model this \
             policy guards and the application's user type",
            r.stem
        )],
    }
}

fn service(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}`: business logic over the models.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// `#[service]` builds this per request from the application's\n\
         /// resources. Keep the methods framework-agnostic -- take domain\n\
         /// values, return domain values, and let the controller map the\n\
         /// result to HTTP.\n\
         #[service]\n\
         pub struct {type_name} {{\n\
         \x20   db: Db,\n\
         }}\n\
         \n\
         impl {type_name} {{\n\
         \x20   /// The pool this service was resolved with.\n\
         \x20   pub fn db(&self) -> &Db {{\n\
         \x20       &self.db\n\
         \x20   }}\n\
         }}\n"
    )
}

fn job(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}` background job.\n\
         \n\
         use arcature::Job;\n\
         use arcature::prelude::*;\n\
         \n\
         /// The payload the worker deserializes. Keep it small and\n\
         /// self-contained: a job outlives the request that enqueued it, so\n\
         /// anything it needs has to travel in these fields or be re-read\n\
         /// from the database by the handler.\n\
         #[derive(Debug, Clone, Serialize, Deserialize, Job)]\n\
         pub struct {type_name} {{\n\
         \x20   pub id: i64,\n\
         }}\n\
         \n\
         /// The handler. Register it with `Registry::add` at startup.\n\
         #[job_handler]\n\
         pub async fn handle(_job: {type_name}) -> Result<()> {{\n\
         \x20   Ok(())\n\
         }}\n"
    )
}

fn event(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}` in-process event.\n\
         \n\
         use arcature::Event;\n\
         use arcature::prelude::*;\n\
         \n\
         /// Dispatched through the `Dispatcher`; listeners receive it by\n\
         /// reference, so the fields describe what happened rather than what\n\
         /// should happen next.\n\
         #[derive(Debug, Clone, Event)]\n\
         pub struct {type_name} {{\n\
         \x20   pub id: i64,\n\
         }}\n"
    )
}

fn listener(name: &ArtifactName) -> Artifact {
    let r = Rendered::new(name, "");
    let Rendered {
        stem,
        base_type,
        base_stem,
        ..
    } = &r;
    let event_type = format!("{base_type}Event");

    let contents = format!(
        "//! The `{stem}` listener.\n\
         //!\n\
         //! A listener is meaningless without an event, and the generator\n\
         //! cannot know which one -- `{event_type}` is a placeholder. Point\n\
         //! the import, the `#[listener(..)]` attribute, and the handler\n\
         //! argument at the event this reacts to.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         use crate::app::events::{base_stem}::{event_type};\n\
         \n\
         /// Reacts to `{event_type}`. Register it on the `Dispatcher` at\n\
         /// startup; the attribute only records the binding for inspection.\n\
         #[listener({event_type})]\n\
         pub async fn {stem}(_event: &{event_type}) -> Result<()> {{\n\
         \x20   Ok(())\n\
         }}\n"
    );

    Artifact {
        path: destination("app/listeners", name, stem),
        contents,
        register_module: true,
        notes: vec![format!(
            "{stem} listens for the placeholder `{event_type}`; point it at a \
             real event before building"
        )],
    }
}

fn middleware(r: &Rendered) -> String {
    let Rendered {
        stem, type_name, ..
    } = r;
    format!(
        "//! The `{type_name}` middleware.\n\
         \n\
         use arcature::prelude::*;\n\
         use arcature::routing::Request;\n\
         \n\
         /// `#[middleware]` turns this function into a `pub struct\n\
         /// {type_name}` implementing `Middleware`, so `routes!` can name it\n\
         /// as `middleware: [{type_name}]`. The function stays callable\n\
         /// directly, which is what makes it testable without a router.\n\
         #[middleware]\n\
         pub async fn {stem}(request: Request, next: Next) -> Result<Response> {{\n\
         \x20   Ok(next.run(request).await)\n\
         }}\n"
    )
}

fn command(r: &Rendered) -> String {
    let Rendered {
        stem, slash_path, ..
    } = r;
    let command_name = slash_path.replace('/', ":");
    format!(
        "//! The `{command_name}` application command.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// Invoked by name. The attribute records the binding for\n\
         /// inspection; registration stays explicit in the application's\n\
         /// `CommandRegistry` so nothing runs that was not asked for.\n\
         #[command(\"{command_name}\")]\n\
         pub async fn {stem}() -> Result<()> {{\n\
         \x20   Ok(())\n\
         }}\n"
    )
}

fn page(r: &Rendered) -> String {
    let Rendered {
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! The props for the `{slash_path}` Inertia page.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// Everything the `{slash_path}` component receives. Every field\n\
         /// crosses the Client Exposure Firewall, so a nested type has to be\n\
         /// a `#[resource]` (or another `#[page]`) -- a plain `Serialize`\n\
         /// domain model will not compile here, by design.\n\
         #[page(\"{slash_path}\")]\n\
         pub struct {type_name} {{\n\
         \x20   pub title: String,\n\
         }}\n"
    )
}

fn test(name: &ArtifactName) -> Artifact {
    let stem = name.file_stem("");
    let mut path = PathBuf::from("tests");
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(format!("{stem}.rs"));

    let contents = format!(
        "//! Integration test: {stem}.\n\
         \n\
         #[test]\n\
         fn {stem}_behaves_as_specified() {{\n\
         \x20   // Replace with the behaviour under test. Name the test after\n\
         \x20   // the guarantee it protects, not after the function it calls.\n\
         }}\n"
    );

    Artifact {
        path,
        contents,
        // Each file under `tests/` is its own crate; there is no `mod.rs` to
        // register with, and creating one would break `cargo test`.
        register_module: false,
        notes: Vec::new(),
    }
}

fn factory(r: &Rendered) -> String {
    let Rendered {
        type_name,
        base_type,
        base_stem,
        ..
    } = r;
    format!(
        "//! The `{type_name}`: deterministic `{base_type}` values for tests\n\
         //! and seeders.\n\
         //!\n\
         //! Arcature ships no factory runtime, so a factory is a plain\n\
         //! constructor. The counter keeps generated values unique inside one\n\
         //! test without reaching for a random source, which is what makes a\n\
         //! failure reproducible.\n\
         \n\
         /// Builds `{base_type}` field values.\n\
         #[derive(Debug, Default)]\n\
         pub struct {type_name} {{\n\
         \x20   sequence: u32,\n\
         }}\n\
         \n\
         impl {type_name} {{\n\
         \x20   /// A fresh factory, starting its sequence at zero.\n\
         \x20   pub fn new() -> Self {{\n\
         \x20       Self::default()\n\
         \x20   }}\n\
         \n\
         \x20   /// The next unique `(id, name)` pair.\n\
         \x20   pub fn next(&mut self) -> (u32, String) {{\n\
         \x20       self.sequence += 1;\n\
         \x20       (self.sequence, format!(\"{base_stem}-{{}}\", self.sequence))\n\
         \x20   }}\n\
         }}\n"
    )
}

fn seeder(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}`: rows this seeder owns.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// Seeds a known starting state. `arc db:seed` reaches this through\n\
         /// the application's own binary, which decides what runs and in what\n\
         /// order -- the CLI never guesses at seeder ordering.\n\
         pub struct {type_name};\n\
         \n\
         impl {type_name} {{\n\
         \x20   /// Insert this seeder's rows.\n\
         \x20   pub async fn run(_db: &Db) -> Result<()> {{\n\
         \x20       Ok(())\n\
         \x20   }}\n\
         }}\n"
    )
}

fn notification(name: &ArtifactName) -> Artifact {
    let r = Rendered::new(name, "");
    let Rendered {
        stem, type_name, ..
    } = &r;
    let kind = stem.replace('_', ".");

    let contents = format!(
        "//! The `{type_name}` notification.\n\
         //!\n\
         //! All three channels are written out, because the trait defaults\n\
         //! every one of them to `None` and a channel that was never\n\
         //! considered looks exactly like a channel that was considered and\n\
         //! declined. Delete the ones this notification should not use; the\n\
         //! deletion is the decision.\n\
         \n\
         use arcature::notifications::{{\n\
         \x20   BroadcastContent, DatabaseContent, MailContent, Notification, Recipient,\n\
         }};\n\
         use arcature::serde_json;\n\
         \n\
         /// What happened, in the fields the channels below render from.\n\
         #[derive(Debug, Clone)]\n\
         pub struct {type_name} {{\n\
         \x20   pub id: i64,\n\
         }}\n\
         \n\
         impl Notification for {type_name} {{\n\
         \x20   fn to_mail(&self, recipient: &Recipient) -> Option<MailContent> {{\n\
         \x20       // No address, no mail -- and no error. A recipient\n\
         \x20       // without one is not a failure to send.\n\
         \x20       recipient.email_address()?;\n\
         \n\
         \x20       Some(MailContent::new(\n\
         \x20           \"{type_name}\",\n\
         \x20           format!(\"Something happened, and it concerns #{{}}.\", self.id),\n\
         \x20       ))\n\
         \x20   }}\n\
         \n\
         \x20   fn to_database(&self, _recipient: &Recipient) -> Option<DatabaseContent> {{\n\
         \x20       Some(DatabaseContent::new(\n\
         \x20           KIND,\n\
         \x20           serde_json::json!({{ \"id\": self.id }}),\n\
         \x20       ))\n\
         \x20   }}\n\
         \n\
         \x20   fn to_broadcast(&self, _recipient: &Recipient) -> Option<BroadcastContent> {{\n\
         \x20       Some(BroadcastContent::new(\n\
         \x20           KIND,\n\
         \x20           serde_json::json!({{ \"id\": self.id }}),\n\
         \x20       ))\n\
         \x20   }}\n\
         }}\n\
         \n\
         /// The name the front end and the stored rows know this by.\n\
         ///\n\
         /// Deliberately a constant and deliberately not derived from\n\
         /// `{type_name}`: this string is written into the notifications\n\
         /// table and switched on by the client, so a `refactor: rename` of\n\
         /// the type must not quietly rewrite a protocol and orphan every row\n\
         /// already stored. Renaming the type is free; changing this is a\n\
         /// migration.\n\
         const KIND: &str = \"{kind}\";\n"
    );

    Artifact {
        path: destination("app/notifications", name, stem),
        contents,
        register_module: true,
        notes: vec![
            "notifications are behind the `notifications` feature; add it to the \
             application's `arcature` dependency before building"
                .to_string(),
            "`to_database` and `to_broadcast` render whatever the features are, but \
             delivering them needs `notifications-db` and `notifications-broadcast`"
                .to_string(),
        ],
    }
}

fn mailable(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "//! The `{type_name}` mailable.\n\
         //!\n\
         //! There is no `use arcature::prelude::*` here, and its absence is\n\
         //! load-bearing: the prelude exports a one-parameter `Result<T>`\n\
         //! alias, which would shadow the two-parameter `Result` that\n\
         //! `Mailable::build` is required to return.\n\
         \n\
         use arcature::mail::lettre::Message;\n\
         use arcature::mail::{{Email, EmailError, Mailable}};\n\
         \n\
         /// Everything this email says, in the fields it says it from.\n\
         pub struct {type_name} {{\n\
         \x20   pub name: String,\n\
         }}\n\
         \n\
         impl Mailable for {type_name} {{\n\
         \x20   fn build(&self, email: Email) -> Result<Message, EmailError> {{\n\
         \x20       // `From` and `To` are already set by\n\
         \x20       // `Mail::to(..).send(..)`; the rest is this type's\n\
         \x20       // decision.\n\
         \x20       //\n\
         \x20       // Plain text only, deliberately. `.alternative(plain,\n\
         \x20       // html)` adds an HTML part, but nothing escapes what you\n\
         \x20       // interpolate into it -- a `format!` into an HTML body is\n\
         \x20       // the same mistake in an email as it is in a page. Render\n\
         \x20       // the HTML half from an askama template and let the\n\
         \x20       // template engine escape it.\n\
         \x20       email\n\
         \x20           .subject(\"{type_name}\")\n\
         \x20           .plain(format!(\"Hello, {{}}.\", self.name))\n\
         \x20   }}\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// The view blueprint: a struct and the template it is the type of.
// ---------------------------------------------------------------------------

/// A server-rendered view: the Rust struct, and the `.html` it renders.
///
/// Two files because askama reads the template at build time. A view struct
/// whose `path` names a file that is not there is not a scaffold with a gap
/// in it -- it is a compile error, and the project stops building until the
/// developer writes the half the generator declined to. The pair is the
/// artifact.
fn view(name: &ArtifactName) -> Vec<Artifact> {
    let rendered = Rendered::new(name, "View");
    vec![
        Artifact {
            path: destination("app/views", name, &rendered.stem),
            contents: view_struct(&rendered),
            register_module: true,
            notes: Vec::new(),
        },
        Artifact {
            path: view_template_path(name),
            contents: view_template(&rendered),
            // No `mod.rs` beside a template: `templates/` is read by the
            // askama derive, not declared to rustc. Registering it would
            // write `pub mod welcome;` into a directory that has no Rust in
            // it at all.
            register_module: false,
            notes: Vec::new(),
        },
    ]
}

/// `templates/<segments...>/<base stem>.html`.
///
/// The base stem, not the file stem: the struct is `WelcomeView` and the
/// template is `welcome.html`, because `path` is a template's name and
/// nothing in askama makes it a type's name.
fn view_template_path(name: &ArtifactName) -> PathBuf {
    let mut path = PathBuf::from("templates");
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(format!("{}.html", name.file_stem("")));
    path
}

fn view_struct(r: &Rendered) -> String {
    let Rendered {
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! The `{type_name}`, and the template it is the type of.\n\
         \n\
         // `Template` is both the trait and the `#[derive(Template)]` macro,\n\
         // so one `use` names both. It is in `arcature::prelude` as well,\n\
         // alongside the `view` helper the controller rendering this calls.\n\
         use arcature::view::Template;\n\
         \n\
         /// `templates/{slash_path}.html`.\n\
         ///\n\
         /// The fields are the names the template is allowed to use, and\n\
         /// that is checked when this crate compiles: a `{{{{ subtitle }}}}`\n\
         /// with no `subtitle` field here is a build failure, not a blank\n\
         /// space on a page nobody looked at.\n\
         ///\n\
         /// `askama = arcature::askama` points the derive at the askama the\n\
         /// framework pins, so this application does not depend on askama\n\
         /// directly and cannot drift to a different version of it.\n\
         #[derive(Template)]\n\
         #[template(path = \"{slash_path}.html\", askama = arcature::askama)]\n\
         pub struct {type_name} {{\n\
         \x20   /// The document title, and the heading.\n\
         \x20   pub title: String,\n\
         \x20   /// The body copy.\n\
         \x20   pub message: String,\n\
         }}\n"
    )
}

fn view_template(r: &Rendered) -> String {
    let Rendered { type_name, .. } = r;
    format!(
        "{{# The template `{type_name}` renders. Its field names are the only\n\
         \x20  names available here, and askama checks that at build time.\n\
         \n\
         \x20  `{{{{ }}}}` escapes, because this file's extension is `.html`.\n\
         \x20  Write `{{{{ value|safe }}}}` only for markup you produced\n\
         \x20  yourself -- never for a value that came in on a request. #}}\n\
         {{% extends \"layout.html\" %}}\n\
         \n\
         {{% block title %}}{{{{ title }}}}{{% endblock %}}\n\
         \n\
         {{% block content %}}\n\
         <main>\n\
         \x20 <h1>{{{{ title }}}}</h1>\n\
         \x20 <p>{{{{ message }}}}</p>\n\
         </main>\n\
         {{% endblock %}}\n"
    )
}

fn upload(name: &ArtifactName) -> Artifact {
    // Not the bare `Controller` suffix: `make:controller avatar` and
    // `make:upload avatar` would then be the same path, and the second one
    // would refuse to write rather than sit beside the first.
    let r = Rendered::new(name, "UploadController");
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = &r;

    let contents = format!(
        "//! The `{type_name}`: one upload endpoint for `{slash_path}`.\n\
         //!\n\
         //! Every check `arcature::validation::upload` documents has already\n\
         //! run by the time this handler's body starts -- the filename is\n\
         //! sanitized, the bytes were sniffed, and the extension is one the\n\
         //! policy allows. What is left here is where the bytes go and what\n\
         //! the response says.\n\
         \n\
         use arcature::prelude::*;\n\
         use arcature::serde_json;\n\
         use arcature::storage::StorageError;\n\
         use arcature::validation::upload::UploadedFile;\n\
         \n\
         use crate::bootstrap::AppState;\n\
         \n\
         /// The disk sub-tree these uploads live under.\n\
         ///\n\
         /// A constant, and the application's own string. Nothing that\n\
         /// arrived on the request is ever a prefix -- that is the whole\n\
         /// reason this is not a parameter.\n\
         const PREFIX: &str = \"{slash_path}\";\n\
         \n\
         /// The `{slash_path}` upload controller.\n\
         pub struct {type_name};\n\
         \n\
         #[controller]\n\
         impl {type_name} {{\n\
         \x20   /// Accept one file and store it under its content address.\n\
         \x20   ///\n\
         \x20   /// Register this as a `post` route. With no `UploadPolicy`\n\
         \x20   /// layer on it the route still refuses everything outside\n\
         \x20   /// `AllowedExtensions::images()`, because the default is\n\
         \x20   /// fail-closed on purpose -- so a route that takes documents\n\
         \x20   /// needs the layer, and one that takes images does not.\n\
         \x20   ///\n\
         \x20   /// `UploadedFile` implements `FromRequest`, not\n\
         \x20   /// `FromRequestParts`: it consumes the body, so it has to be\n\
         \x20   /// the last argument.\n\
         \x20   pub async fn store(\n\
         \x20       State(state): State<AppState>,\n\
         \x20       upload: UploadedFile,\n\
         \x20   ) -> Result<Response> {{\n\
         \x20       let storage = state.storage.as_ref().ok_or_else(|| {{\n\
         \x20           Error::Storage(\"no storage disk is configured\".to_string())\n\
         \x20       }})?;\n\
         \n\
         \x20       // `?` rather than a `map_err`: `UploadError` splits the\n\
         \x20       // server's problem from the client's, and\n\
         \x20       // `From<UploadError> for Error` keeps that split. A disk\n\
         \x20       // that is down answers 500; a file whose bytes disagree\n\
         \x20       // with its extension answers 422.\n\
         \x20       let address =\n\
         \x20           upload.store_under(&storage.default_disk(), PREFIX).await?;\n\
         \n\
         \x20       // `address.path()` is the key without the prefix, and the\n\
         \x20       // object was written under one -- so this, not that, is\n\
         \x20       // the name that finds the bytes again.\n\
         \x20       let key = address.path_under(PREFIX).map_err(StorageError::from)?;\n\
         \n\
         \x20       Ok((\n\
         \x20           StatusCode::CREATED,\n\
         \x20           json(serde_json::json!({{\n\
         \x20               \"path\": key.as_str(),\n\
         \x20               \"bytes\": address.byte_len(),\n\
         \x20               // Metadata, and only ever metadata: show it, send\n\
         \x20               // it in a `Content-Disposition`, never resolve it\n\
         \x20               // as a path.\n\
         \x20               \"filename\": upload.filename().to_string(),\n\
         \x20           }})),\n\
         \x20       )\n\
         \x20           .into_response())\n\
         \x20   }}\n\
         }}\n"
    );

    Artifact {
        path: destination("app/controllers", name, stem),
        contents,
        register_module: true,
        notes: vec![
            "uploads are behind the `uploads` feature; add it to the application's \
             `arcature` dependency before building"
                .to_string(),
            "an `UploadPolicy` layer decides which extensions the route accepts; \
             with no layer the route accepts images and nothing else"
                .to_string(),
        ],
    }
}

// ---------------------------------------------------------------------------
// The auth blueprint: one account, three controllers, a route table, a table.
// ---------------------------------------------------------------------------

/// Everything a sign-in needs, minus the screens.
///
/// The six files answer one question each -- who an account is, how one is
/// created, how a session starts and ends, how a forgotten password is
/// recovered, which paths reach those, and what the table looks like -- and
/// the reason they arrive together is that five of the six are wrong on
/// their own. A registration handler that does not know how the login
/// handler compares passwords is a registration handler that stores a hash
/// nothing can verify.
///
/// It is headless: JSON in, JSON or `204` out. A generated login *form* would
/// have to pick Inertia or a server-rendered view, a CSS convention and a
/// copy deck, and be wrong about at least one of them in every application.
/// The parts that are the same everywhere -- and that are dangerous to get
/// wrong -- are here; `arc make:page` and `arc make:view` are one command
/// each for the parts that are not.
fn auth(name: &ArtifactName) -> Vec<Artifact> {
    let r = Rendered::new(name, "");
    vec![
        account(name),
        auth_part(name, "registration_controller", registration_controller(&r)),
        auth_part(name, "session_controller", session_controller(&r)),
        auth_part(name, "password_controller", password_controller(&r)),
        auth_part(name, "routes", auth_routes(&r)),
        auth_migration(name),
    ]
}

/// `app/auth/<segments...>/<stem>_<part>.rs`, registered like any other file.
///
/// Every sibling is registered, unlike `make:module`, because these are
/// ordinary files in an ordinary directory rather than a directory whose own
/// `mod.rs` is written by hand. The generic registration walks up from
/// `app/auth/` and declares each level it had to create, so `app/mod.rs`
/// learns about `auth` without the generator naming it.
fn auth_part(name: &ArtifactName, part: &str, contents: String) -> Artifact {
    let stem = format!("{}_{part}", name.file_stem(""));
    Artifact {
        path: destination("app/auth", name, &stem),
        contents,
        register_module: true,
        notes: Vec::new(),
    }
}

/// The account itself: the model, plus the two traits that make it signable.
fn account(name: &ArtifactName) -> Artifact {
    let r = Rendered::new(name, "");
    let Rendered {
        stem,
        type_name,
        base_stem,
        ..
    } = &r;
    let table = pluralize(base_stem);

    let contents = format!(
        "//! The `{type_name}` account: one row of `{table}`.\n\
         //!\n\
         //! Three impls, and only the first is what `arc make:model` writes.\n\
         //! `AuthUser` is what lets a session hold this account; `UserLoader`\n\
         //! is what turns the id in that session back into an account on the\n\
         //! next request. A model with neither still compiles -- it simply\n\
         //! cannot be signed in.\n\
         \n\
         use arcature::auth::UserLoader;\n\
         use arcature::database::sea_orm::ActiveValue;\n\
         use arcature::database::{{insert, update}};\n\
         use arcature::prelude::*;\n\
         \n\
         use crate::bootstrap::AppState;\n\
         \n\
         /// A `{table}` row.\n\
         #[model(table = \"{table}\")]\n\
         pub struct {type_name} {{\n\
         \x20   #[sea_orm(primary_key)]\n\
         \x20   pub id: i64,\n\
         \x20   #[sea_orm(unique)]\n\
         \x20   pub email: String,\n\
         \x20   /// The Argon2 PHC string. Never the password.\n\
         \x20   pub password_hash: String,\n\
         \x20   /// `None` until the address is proved reachable.\n\
         \x20   pub email_verified_at: Option<DateTimeUtc>,\n\
         }}\n\
         \n\
         impl AuthUser for {type_name} {{\n\
         \x20   type Id = i64;\n\
         \n\
         \x20   fn id(&self) -> &i64 {{\n\
         \x20       &self.id\n\
         \x20   }}\n\
         \n\
         \x20   /// The credential every session is stamped against.\n\
         \x20   ///\n\
         \x20   /// Returning it is what signs the other devices out when the\n\
         \x20   /// password changes: a session carries a digest of this, and\n\
         \x20   /// one whose digest no longer matches is flushed on its next\n\
         \x20   /// request. Return `None` and nothing checks, so a stolen\n\
         \x20   /// session outlives the password it was issued against.\n\
         \x20   fn stored_credential(&self) -> Option<&[u8]> {{\n\
         \x20       Some(self.password_hash.as_bytes())\n\
         \x20   }}\n\
         }}\n\
         \n\
         impl UserLoader<AppState> for {type_name} {{\n\
         \x20   type Error = arcature::Error;\n\
         \n\
         \x20   /// `core::result::Result` spelled out: the prelude brings a\n\
         \x20   /// one-parameter `Result<T>` alias, and this signature needs\n\
         \x20   /// the two-parameter one.\n\
         \x20   async fn load_user(\n\
         \x20       id: &i64,\n\
         \x20       state: &AppState,\n\
         \x20   ) -> core::result::Result<Option<Self>, Self::Error> {{\n\
         \x20       {type_name}::query(connection(state)?)\n\
         \x20           .where_eq({type_name}Column::Id, *id)\n\
         \x20           .one()\n\
         \x20           .await\n\
         \x20   }}\n\
         }}\n\
         \n\
         /// Find one account by address.\n\
         pub async fn find_by_email(\n\
         \x20   state: &AppState,\n\
         \x20   email: &str,\n\
         ) -> Result<Option<{type_name}>> {{\n\
         \x20   {type_name}::query(connection(state)?)\n\
         \x20       .where_eq({type_name}Column::Email, email)\n\
         \x20       .one()\n\
         \x20       .await\n\
         }}\n\
         \n\
         /// Insert one account. The caller hashes the password.\n\
         pub async fn create(\n\
         \x20   state: &AppState,\n\
         \x20   email: &str,\n\
         \x20   password_hash: &str,\n\
         ) -> Result<{type_name}> {{\n\
         \x20   let record = {type_name}ActiveModel {{\n\
         \x20       id: ActiveValue::NotSet,\n\
         \x20       email: ActiveValue::Set(email.to_string()),\n\
         \x20       password_hash: ActiveValue::Set(password_hash.to_string()),\n\
         \x20       email_verified_at: ActiveValue::Set(None),\n\
         \x20   }};\n\
         \x20   insert(connection(state)?, record).await\n\
         }}\n\
         \n\
         /// Replace one account's password hash.\n\
         ///\n\
         /// Writing this column is what ends every session that account has\n\
         /// open, including the one doing the writing: the digest\n\
         /// `stored_credential` returns changes with it. Call\n\
         /// `AuthManager::rebind_credential` with the *re-read* account to\n\
         /// keep the current session alive and drop only the others.\n\
         pub async fn set_password(\n\
         \x20   state: &AppState,\n\
         \x20   id: i64,\n\
         \x20   password_hash: &str,\n\
         ) -> Result<{type_name}> {{\n\
         \x20   let record = {type_name}ActiveModel {{\n\
         \x20       id: ActiveValue::Unchanged(id),\n\
         \x20       email: ActiveValue::NotSet,\n\
         \x20       password_hash: ActiveValue::Set(password_hash.to_string()),\n\
         \x20       email_verified_at: ActiveValue::NotSet,\n\
         \x20   }};\n\
         \x20   update(connection(state)?, record).await\n\
         }}\n\
         \n\
         /// The database, or a server error rather than a panic.\n\
         pub fn connection(state: &AppState) -> Result<&Db> {{\n\
         \x20   state\n\
         \x20       .db\n\
         \x20       .as_ref()\n\
         \x20       .ok_or_else(|| Error::Config(\"no database is configured\".to_string()))\n\
         }}\n"
    );

    Artifact {
        path: destination("app/auth", name, stem),
        contents,
        register_module: true,
        notes: vec![
            "the flows these controllers call live behind `auth-flows` and \
             `auth-reset`, neither of which is in the feature list `arc new` \
             writes; add both to the application's `arcature` dependency"
                .to_string(),
        ],
    }
}

fn registration_controller(r: &Rendered) -> String {
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! Registration for `{type_name}`.\n\
         //!\n\
         //! One handler, and it answers identically whether or not the\n\
         //! address is already taken. \"That address is already registered\"\n\
         //! is a membership oracle anyone can query for any address, and a\n\
         //! registration form is the one place it needs no password.\n\
         //!\n\
         //! It also does not sign the new account in. Auto-login is the\n\
         //! friendlier default and it reopens the same hole from the other\n\
         //! side: a `Set-Cookie` on one branch and not the other says which\n\
         //! branch ran. Sign in after the address is verified, or add\n\
         //! `AuthManager` here and accept the oracle knowingly.\n\
         \n\
         use arcature::prelude::*;\n\
         use arcature::serde_json;\n\
         \n\
         use super::{stem}::{{create, find_by_email}};\n\
         use crate::bootstrap::AppState;\n\
         \n\
         /// What a registration form sends.\n\
         #[request]\n\
         #[derive(Debug, Clone, Deserialize)]\n\
         pub struct Register{type_name} {{\n\
         \x20   /// Length only. `validator` is built here without its `email`\n\
         \x20   /// feature, and that is the better check anyway: an address\n\
         \x20   /// is proved by mail arriving at it, not by a regular\n\
         \x20   /// expression agreeing with its shape.\n\
         \x20   #[validate(length(min = 3, max = 254))]\n\
         \x20   pub email: String,\n\
         \x20   #[validate(length(min = 12, max = 4096))]\n\
         \x20   pub password: String,\n\
         }}\n\
         \n\
         /// The `{slash_path}` registration controller.\n\
         pub struct {type_name}RegistrationController;\n\
         \n\
         #[controller]\n\
         impl {type_name}RegistrationController {{\n\
         \x20   /// Create one account.\n\
         \x20   pub async fn store(\n\
         \x20       State(state): State<AppState>,\n\
         \x20       Validated(input): Validated<Register{type_name}>,\n\
         \x20   ) -> Result<Response> {{\n\
         \x20       let email = input.email.trim().to_lowercase();\n\
         \n\
         \x20       // Hash first, unconditionally. Hashing only when the\n\
         \x20       // address turns out to be free would make the response\n\
         \x20       // time say whether it was.\n\
         \x20       let hash = state\n\
         \x20           .hasher\n\
         \x20           .hash(input.password.as_bytes())\n\
         \x20           .map_err(|error| Error::Other(error.to_string()))?;\n\
         \n\
         \x20       if find_by_email(&state, &email).await?.is_none() {{\n\
         \x20           create(&state, &email, hash.as_str()).await?;\n\
         \x20       }}\n\
         \n\
         \x20       Ok((\n\
         \x20           StatusCode::ACCEPTED,\n\
         \x20           json(serde_json::json!({{\n\
         \x20               \"status\": \"check your mail to finish signing up\",\n\
         \x20           }})),\n\
         \x20       )\n\
         \x20           .into_response())\n\
         \x20   }}\n\
         }}\n"
    )
}

fn session_controller(r: &Rendered) -> String {
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! Sign-in and sign-out for `{type_name}`.\n\
         //!\n\
         //! Two decisions here are load-bearing, and both are about what the\n\
         //! response does *not* say.\n\
         //!\n\
         //! A rejected sign-in answers `CREDENTIAL_REJECTION` whether the\n\
         //! address is unknown or the password is wrong, and\n\
         //! `CredentialChecker` runs a full Argon2 verification either way --\n\
         //! against a throwaway hash when there is no account. Skipping that\n\
         //! work for an unknown address makes the two cases differ by\n\
         //! milliseconds, which is a membership oracle measurable over the\n\
         //! network.\n\
         //!\n\
         //! And a stored hash that will not parse falls through to the same\n\
         //! rejection rather than a 500, because a distinguishable error is\n\
         //! that oracle again by another route.\n\
         \n\
         use std::sync::{{LazyLock, OnceLock}};\n\
         \n\
         use arcature::auth::PasswordHashString;\n\
         use arcature::auth::flows::{{CREDENTIAL_REJECTION, CredentialChecker, LoginThrottle}};\n\
         use arcature::axum::Extension;\n\
         use arcature::http::ClientIp;\n\
         use arcature::prelude::*;\n\
         use arcature::serde_json;\n\
         \n\
         use super::{stem}::{{{type_name}, find_by_email}};\n\
         use crate::bootstrap::AppState;\n\
         \n\
         /// Failed sign-ins, counted per address and per client address.\n\
         ///\n\
         /// A `static` because the count has to outlive the request: an\n\
         /// instance built per call counts to one forever and throttles\n\
         /// nothing. That makes it per-process, so two instances behind a\n\
         /// load balancer each keep their own tally. Move it into `AppState`\n\
         /// when it should be shared, and to a shared store when it should be\n\
         /// shared across processes.\n\
         static THROTTLE: LazyLock<LoginThrottle> = LazyLock::new(LoginThrottle::new);\n\
         \n\
         /// The verifier, built once.\n\
         ///\n\
         /// `CredentialChecker::new` hashes a throwaway password at\n\
         /// construction so it has something to verify against when no\n\
         /// account matches, and that costs one full Argon2. Building it per\n\
         /// request would spend that on every sign-in.\n\
         static CHECKER: OnceLock<CredentialChecker> = OnceLock::new();\n\
         \n\
         fn checker(state: &AppState) -> Result<&'static CredentialChecker> {{\n\
         \x20   if let Some(checker) = CHECKER.get() {{\n\
         \x20       return Ok(checker);\n\
         \x20   }}\n\
         \x20   // Built from the application's own hasher, so the throwaway\n\
         \x20   // verification costs exactly what a real one costs. A checker\n\
         \x20   // with default parameters would take a different amount of\n\
         \x20   // time and put the timing difference straight back.\n\
         \x20   let built = CredentialChecker::new((*state.hasher).clone())\n\
         \x20       .map_err(|error| Error::Other(error.to_string()))?;\n\
         \x20   Ok(CHECKER.get_or_init(|| built))\n\
         }}\n\
         \n\
         /// What a sign-in form sends.\n\
         #[request]\n\
         #[derive(Debug, Clone, Deserialize)]\n\
         pub struct {type_name}Credentials {{\n\
         \x20   #[validate(length(min = 3, max = 254))]\n\
         \x20   pub email: String,\n\
         \x20   #[validate(length(min = 1, max = 4096))]\n\
         \x20   pub password: String,\n\
         }}\n\
         \n\
         /// The `{slash_path}` session controller.\n\
         pub struct {type_name}SessionController;\n\
         \n\
         #[controller]\n\
         impl {type_name}SessionController {{\n\
         \x20   /// Start a session.\n\
         \x20   ///\n\
         \x20   /// `ClientIp` arrives as an extension rather than an\n\
         \x20   /// extractor, and optionally: the raw serve path does not\n\
         \x20   /// insert one. `None` throttles by address alone, which is\n\
         \x20   /// weaker than throttling by both and stronger than a 500.\n\
         \x20   pub async fn store(\n\
         \x20       State(state): State<AppState>,\n\
         \x20       auth: AuthManager<{type_name}>,\n\
         \x20       client: Option<Extension<ClientIp>>,\n\
         \x20       Validated(input): Validated<{type_name}Credentials>,\n\
         \x20   ) -> Result<Response> {{\n\
         \x20       let email = input.email.trim().to_lowercase();\n\
         \x20       let address = client.map(|Extension(ip)| ip.addr());\n\
         \n\
         \x20       let decision = THROTTLE.check(&email, address);\n\
         \x20       if !decision.is_allowed() {{\n\
         \x20           let seconds = decision.retry_after().map_or(60, |d| d.as_secs());\n\
         \x20           return Ok((\n\
         \x20               StatusCode::TOO_MANY_REQUESTS,\n\
         \x20               [(\"retry-after\", seconds.to_string())],\n\
         \x20               json(serde_json::json!({{ \"message\": \"too many attempts\" }})),\n\
         \x20           )\n\
         \x20               .into_response());\n\
         \x20       }}\n\
         \n\
         \x20       let account = find_by_email(&state, &email).await?;\n\
         \n\
         \x20       // A hash the parser rejects becomes \"no stored hash\",\n\
         \x20       // which the checker answers with the same rejection as a\n\
         \x20       // wrong password. The alternative is a 500 on exactly the\n\
         \x20       // accounts whose rows are damaged, which says something\n\
         \x20       // about those accounts.\n\
         \x20       let stored = account\n\
         \x20           .as_ref()\n\
         \x20           .and_then(|found| PasswordHashString::new(&found.password_hash).ok());\n\
         \n\
         \x20       let outcome = checker(&state)?.check(stored.as_ref(), input.password.as_bytes());\n\
         \n\
         \x20       match (outcome.is_verified(), account) {{\n\
         \x20           (true, Some(account)) => {{\n\
         \x20               THROTTLE.record_success(&email, address);\n\
         \x20               auth.login(&account)\n\
         \x20                   .await\n\
         \x20                   .map_err(|error| Error::Other(error.to_string()))?;\n\
         \x20               Ok(no_content())\n\
         \x20           }}\n\
         \x20           _ => {{\n\
         \x20               THROTTLE.record_failure(&email, address);\n\
         \x20               Ok((\n\
         \x20                   StatusCode::UNPROCESSABLE_ENTITY,\n\
         \x20                   json(serde_json::json!({{ \"message\": CREDENTIAL_REJECTION }})),\n\
         \x20               )\n\
         \x20                   .into_response())\n\
         \x20           }}\n\
         \x20       }}\n\
         \x20   }}\n\
         \n\
         \x20   /// End the session.\n\
         \x20   pub async fn destroy(auth: AuthManager<{type_name}>) -> Result<Response> {{\n\
         \x20       auth.logout()\n\
         \x20           .await\n\
         \x20           .map_err(|error| Error::Other(error.to_string()))?;\n\
         \x20       Ok(no_content())\n\
         \x20   }}\n\
         }}\n"
    )
}

fn password_controller(r: &Rendered) -> String {
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = r;
    format!(
        "//! Password reset for `{type_name}`.\n\
         //!\n\
         //! `request` mints a one-time link; `reset` redeems it.\n\
         //!\n\
         //! `request` answers the same way for an address with no account,\n\
         //! for the reason registration does -- a forgotten-password form\n\
         //! that says \"no such address\" is a membership oracle needing no\n\
         //! password at all.\n\
         //!\n\
         //! What it does not do is send the mail. The link has to reach the\n\
         //! person somehow, and which template, transport and copy that means\n\
         //! is the application's. The line below builds the URL and stops.\n\
         \n\
         use std::time::Duration;\n\
         \n\
         use arcature::auth::flows::PasswordResets;\n\
         use arcature::prelude::*;\n\
         use arcature::serde_json;\n\
         \n\
         use super::{stem}::{{connection, find_by_email, set_password}};\n\
         use crate::bootstrap::AppState;\n\
         \n\
         /// How long a reset link is good for.\n\
         ///\n\
         /// Short on purpose: the window is how long a link sitting in an\n\
         /// inbox, a proxy log or a browser history is still worth stealing.\n\
         const RESET_TTL: Duration = Duration::from_secs(60 * 60);\n\
         \n\
         /// What a forgotten-password form sends.\n\
         #[request]\n\
         #[derive(Debug, Clone, Deserialize)]\n\
         pub struct {type_name}PasswordRequest {{\n\
         \x20   #[validate(length(min = 3, max = 254))]\n\
         \x20   pub email: String,\n\
         }}\n\
         \n\
         /// What the reset form sends back.\n\
         #[request]\n\
         #[derive(Debug, Clone, Deserialize)]\n\
         pub struct {type_name}PasswordReset {{\n\
         \x20   #[validate(length(min = 1, max = 512))]\n\
         \x20   pub token: String,\n\
         \x20   #[validate(length(min = 12, max = 4096))]\n\
         \x20   pub password: String,\n\
         }}\n\
         \n\
         /// The `{slash_path}` password controller.\n\
         pub struct {type_name}PasswordController;\n\
         \n\
         #[controller]\n\
         impl {type_name}PasswordController {{\n\
         \x20   /// Mint a reset link, if the address has an account.\n\
         \x20   pub async fn request(\n\
         \x20       State(state): State<AppState>,\n\
         \x20       Validated(input): Validated<{type_name}PasswordRequest>,\n\
         \x20   ) -> Result<Response> {{\n\
         \x20       let email = input.email.trim().to_lowercase();\n\
         \x20       let resets = PasswordResets::new(connection(&state)?.sqlx().clone());\n\
         \n\
         \x20       if find_by_email(&state, &email).await?.is_some() {{\n\
         \x20           let issued = resets\n\
         \x20               .issue(&email, RESET_TTL)\n\
         \x20               .await\n\
         \x20               .map_err(|error| Error::Other(error.to_string()))?;\n\
         \n\
         \x20           // The one place the plaintext exists. Mail it. Do not\n\
         \x20           // log it, and do not return it -- a reset link in a\n\
         \x20           // response body belongs to anyone who can read the\n\
         \x20           // response.\n\
         \x20           let _link = format!(\n\
         \x20               \"{{}}/password/reset?token={{}}\",\n\
         \x20               state.app_url,\n\
         \x20               issued.plaintext().expose()\n\
         \x20           );\n\
         \x20       }}\n\
         \n\
         \x20       Ok(json(serde_json::json!({{\n\
         \x20           \"status\": \"if that address has an account, a link is on its way\",\n\
         \x20       }})))\n\
         \x20   }}\n\
         \n\
         \x20   /// Redeem a reset link and set the new password.\n\
         \x20   pub async fn reset(\n\
         \x20       State(state): State<AppState>,\n\
         \x20       Validated(input): Validated<{type_name}PasswordReset>,\n\
         \x20   ) -> Result<Response> {{\n\
         \x20       let resets = PasswordResets::new(connection(&state)?.sqlx().clone());\n\
         \n\
         \x20       let Some(email) = resets\n\
         \x20           .consume(&input.token)\n\
         \x20           .await\n\
         \x20           .map_err(|error| Error::Other(error.to_string()))?\n\
         \x20       else {{\n\
         \x20           return Ok((\n\
         \x20               StatusCode::UNPROCESSABLE_ENTITY,\n\
         \x20               json(serde_json::json!({{ \"message\": \"that link is not valid\" }})),\n\
         \x20           )\n\
         \x20               .into_response());\n\
         \x20       }};\n\
         \n\
         \x20       let Some(account) = find_by_email(&state, &email).await? else {{\n\
         \x20           // The link redeemed, but the account is gone.\n\
         \x20           return Ok(no_content());\n\
         \x20       }};\n\
         \n\
         \x20       let hash = state\n\
         \x20           .hasher\n\
         \x20           .hash(input.password.as_bytes())\n\
         \x20           .map_err(|error| Error::Other(error.to_string()))?;\n\
         \x20       set_password(&state, account.id, hash.as_str()).await?;\n\
         \n\
         \x20       // Writing the hash already ended every session bound to\n\
         \x20       // the old one. This revokes the outstanding *links* too,\n\
         \x20       // so a second copy of the mail cannot be redeemed after\n\
         \x20       // the first.\n\
         \x20       resets\n\
         \x20           .revoke_all_for(&email)\n\
         \x20           .await\n\
         \x20           .map_err(|error| Error::Other(error.to_string()))?;\n\
         \n\
         \x20       Ok(no_content())\n\
         \x20   }}\n\
         }}\n"
    )
}

fn auth_routes(r: &Rendered) -> String {
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = r;
    let namespace = slash_path.replace('/', ".");
    format!(
        "//! The paths the `{slash_path}` sign-in flow serves.\n\
         //!\n\
         //! The paths are absolute: a `Routes` collection adds no prefix of\n\
         //! its own. Wrap the block in a `group` if this flow needs one, or\n\
         //! if a second `arc make:auth` would otherwise claim `/login` twice\n\
         //! -- two routes on one path is a panic at boot, from axum.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         use super::{stem}_password_controller::{type_name}PasswordController;\n\
         use super::{stem}_registration_controller::{type_name}RegistrationController;\n\
         use super::{stem}_session_controller::{type_name}SessionController;\n\
         use crate::bootstrap::AppState;\n\
         \n\
         routes! {{\n\
         \x20   pub {stem}_auth {{\n\
         \x20       state: AppState;\n\
         \n\
         \x20       post \"/register\" => {type_name}RegistrationController::store \
         {{ name: {namespace}.register }}\n\
         \x20       post \"/login\" => {type_name}SessionController::store \
         {{ name: {namespace}.login }}\n\
         \x20       post \"/logout\" => {type_name}SessionController::destroy \
         {{ name: {namespace}.logout }}\n\
         \x20       post \"/password/forgot\" => {type_name}PasswordController::request \
         {{ name: {namespace}.password.forgot }}\n\
         \x20       post \"/password/reset\" => {type_name}PasswordController::reset \
         {{ name: {namespace}.password.reset }}\n\
         \x20   }}\n\
         }}\n"
    )
}

/// The table the account maps to.
///
/// Explicit `ColumnDef` builders rather than the `schema::*` shorthands, and
/// a `DeriveIden` enum named for the table so the default snake-casing spells
/// it -- both so this file depends on nothing beyond the migration prelude it
/// already imports.
fn auth_migration(name: &ArtifactName) -> Artifact {
    let base_stem = name.file_stem("");
    let table = pluralize(&base_stem);
    let iden = to_pascal_case(&table);
    let module = format!("m{}_create_{table}", utc_stamp());
    let mut path = PathBuf::from("database/migrations");
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(format!("{module}.rs"));

    let contents = format!(
        "//! Migration `create_{table}`: the table the account model maps to.\n\
         \n\
         // `DeriveMigrationName` expands to a path relative to a crate\n\
         // named `sea_orm_migration`, and the prelude carries names out\n\
         // of that crate rather than the crate itself.\n\
         use arcature::database::sea_orm_migration;\n\
         use arcature::database::sea_orm_migration::prelude::*;\n\
         \n\
         /// Create `{table}`.\n\
         #[derive(DeriveMigrationName)]\n\
         pub struct Migration;\n\
         \n\
         /// The columns, spelled the way `#[model]` spells them.\n\
         #[derive(DeriveIden)]\n\
         enum {iden} {{\n\
         \x20   Table,\n\
         \x20   Id,\n\
         \x20   Email,\n\
         \x20   PasswordHash,\n\
         \x20   EmailVerifiedAt,\n\
         }}\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigrationTrait for Migration {{\n\
         \x20   async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       manager\n\
         \x20           .create_table(\n\
         \x20               Table::create()\n\
         \x20                   .table({iden}::Table)\n\
         \x20                   .if_not_exists()\n\
         \x20                   .col(\n\
         \x20                       ColumnDef::new({iden}::Id)\n\
         \x20                           .big_integer()\n\
         \x20                           .not_null()\n\
         \x20                           .auto_increment()\n\
         \x20                           .primary_key(),\n\
         \x20                   )\n\
         \x20                   // Unique in the database, not only in the\n\
         \x20                   // handler: two registrations racing each other\n\
         \x20                   // both read \"no such address\" and both insert.\n\
         \x20                   // The constraint is what makes the second one\n\
         \x20                   // fail.\n\
         \x20                   .col(\n\
         \x20                       ColumnDef::new({iden}::Email)\n\
         \x20                           .string_len(254)\n\
         \x20                           .not_null()\n\
         \x20                           .unique_key(),\n\
         \x20                   )\n\
         \x20                   .col(\n\
         \x20                       ColumnDef::new({iden}::PasswordHash)\n\
         \x20                           .string_len(255)\n\
         \x20                           .not_null(),\n\
         \x20                   )\n\
         \x20                   .col(\n\
         \x20                       ColumnDef::new({iden}::EmailVerifiedAt)\n\
         \x20                           .timestamp_with_time_zone()\n\
         \x20                           .null(),\n\
         \x20                   )\n\
         \x20                   .to_owned(),\n\
         \x20           )\n\
         \x20           .await\n\
         \x20   }}\n\
         \n\
         \x20   async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       manager\n\
         \x20           .drop_table(Table::drop().table({iden}::Table).to_owned())\n\
         \x20           .await\n\
         \x20   }}\n\
         }}\n"
    );

    Artifact {
        path,
        contents,
        register_module: true,
        notes: vec![
            format!(
                "add `Box::new({module}::Migration)` to `Migrator::migrations()` \
                 in database/migrations/mod.rs -- ordering is yours to choose, so \
                 the generator does not guess it"
            ),
            "the reset links need `arcature_password_resets`, which is not this \
             migration: call `PasswordResets::new(pool).migrate()` once at boot, \
             or write the table into a migration of your own"
                .to_string(),
            format!(
                "the route collection is not mounted anywhere -- merge \
                 `crate::app::auth::{base_stem}_routes::{base_stem}_auth_routes()` \
                 into `bootstrap/app.rs` beside `crate::routes::app_routes()`"
            ),
        ],
    }
}

// ---------------------------------------------------------------------------
// The module blueprint: one directory, four files.
// ---------------------------------------------------------------------------

/// A feature module: the `module!` block, and the controller, service and
/// routes it declares.
///
/// Four files rather than five. `arc make:policy` is one command away, and a
/// policy scaffold cannot compile until it is pointed at a model and a user
/// type that a fresh application does not have -- shipping one inside a
/// module would mean `arc make:module billing` produces a project that does
/// not build.
fn module(name: &ArtifactName) -> Vec<Artifact> {
    let rendered = Rendered::new(name, "");
    vec![
        module_root(name),
        module_part(name, "controller.rs", module_controller(&rendered)),
        module_part(name, "service.rs", module_service(&rendered)),
        module_part(name, "routes.rs", module_routes(&rendered)),
    ]
}

/// `app/modules/<segments...>/<stem>/<file>` -- the directory a module owns.
fn module_file(name: &ArtifactName, file: &str) -> PathBuf {
    let mut path = PathBuf::from("app/modules");
    for segment in name.segments() {
        path.push(segment);
    }
    path.push(name.file_stem(""));
    path.push(file);
    path
}

/// One of the three files the module's own `mod.rs` already declares.
///
/// `register_module` is false here, and on the root as well. The generic
/// registration derives the declaration from the file stem, which for these
/// three would append a second `pub mod controller;` to a `mod.rs` that
/// already has one -- and for the root would write `pub mod mod;`.
/// [`super::generate_all`] registers the module as a whole instead.
fn module_part(name: &ArtifactName, file: &str, contents: String) -> Artifact {
    Artifact {
        path: module_file(name, file),
        contents,
        register_module: false,
        notes: Vec::new(),
    }
}

fn module_root(name: &ArtifactName) -> Artifact {
    let r = Rendered::new(name, "");
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = &r;
    let controller_type = format!("{type_name}Controller");
    let service_type = format!("{type_name}Service");
    let routes_const = format!("{}_ROUTES", stem.to_uppercase());

    let contents = format!(
        "//! The `{slash_path}` feature module.\n\
         //!\n\
         //! Everything this feature owns lives in this directory, and the\n\
         //! `module!` block below is the index of it. The application graph\n\
         //! reads that index at boot: a type missing from it still compiles\n\
         //! and still serves, it is simply invisible to `arc routes` and\n\
         //! `arc typegen`.\n\
         \n\
         pub mod controller;\n\
         pub mod routes;\n\
         pub mod service;\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         // `controllers:` and `routes:` are resolved at this site -- the\n\
         // first reads the controller's method metadata, the second is a path\n\
         // to a const. `services:` and `policies:` are recorded as names\n\
         // only, which is why `{service_type}` is named below but not\n\
         // imported: an import the macro never resolves is an unused one.\n\
         use controller::{controller_type};\n\
         \n\
         module! {{\n\
         \x20   pub {type_name} {{\n\
         \x20       controllers: [{controller_type}],\n\
         \x20       services: [{service_type}],\n\
         \x20       routes: routes::{routes_const},\n\
         \x20   }}\n\
         }}\n"
    );

    Artifact {
        path: module_file(name, "mod.rs"),
        contents,
        register_module: false,
        notes: Vec::new(),
    }
}

fn module_controller(r: &Rendered) -> String {
    let Rendered {
        type_name,
        slash_path,
        ..
    } = r;
    let controller_type = format!("{type_name}Controller");
    format!(
        "//! HTTP entry points for the `{slash_path}` module.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// The `{slash_path}` controller.\n\
         pub struct {controller_type};\n\
         \n\
         #[controller]\n\
         impl {controller_type} {{\n\
         \x20   /// The index action, already wired up in this module's\n\
         \x20   /// `routes.rs`. Replace the body with the real response.\n\
         \x20   pub async fn index() -> Result<Response> {{\n\
         \x20       Ok(text(StatusCode::OK, \"{controller_type}::index\"))\n\
         \x20   }}\n\
         }}\n"
    )
}

fn module_service(r: &Rendered) -> String {
    let Rendered {
        type_name,
        slash_path,
        ..
    } = r;
    let service_type = format!("{type_name}Service");
    format!(
        "//! Business logic for the `{slash_path}` module.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         /// `#[service]` builds this per request from the application's\n\
         /// resources. Keep the methods framework-agnostic -- take domain\n\
         /// values, return domain values, and let the controller map the\n\
         /// result to HTTP.\n\
         #[service]\n\
         pub struct {service_type} {{\n\
         \x20   db: Db,\n\
         }}\n\
         \n\
         impl {service_type} {{\n\
         \x20   /// The pool this service was resolved with.\n\
         \x20   pub fn db(&self) -> &Db {{\n\
         \x20       &self.db\n\
         \x20   }}\n\
         }}\n"
    )
}

fn module_routes(r: &Rendered) -> String {
    let Rendered {
        stem,
        type_name,
        slash_path,
        ..
    } = r;
    let controller_type = format!("{type_name}Controller");
    let route_name = slash_path.replace('/', ".");
    format!(
        "//! The paths the `{slash_path}` module serves.\n\
         //!\n\
         //! `app/modules/mod.rs` merges this block into the application's own\n\
         //! table, so the paths here are absolute -- living in a module adds\n\
         //! no prefix. Two modules claiming the same path is a panic at boot,\n\
         //! from axum; two modules claiming the same route *name* is not, and\n\
         //! the later one silently wins. That is why the name below carries\n\
         //! the module's own.\n\
         \n\
         use arcature::prelude::*;\n\
         \n\
         use super::controller::{controller_type};\n\
         use crate::bootstrap::AppState;\n\
         \n\
         routes! {{\n\
         \x20   pub {stem} {{\n\
         \x20       state: AppState;\n\
         \n\
         \x20       get \"/{slash_path}\" => {controller_type}::index \
         {{ name: {route_name}.index }}\n\
         \x20   }}\n\
         }}\n"
    )
}

// ---------------------------------------------------------------------------
// Timestamps.
// ---------------------------------------------------------------------------

/// `YYYYMMDD_HHMMSS` in UTC, the ordering prefix SeaORM migrations use.
///
/// Computed from `SystemTime` by hand rather than through `chrono`: `chrono`
/// arrives with the `database` feature, and `arc make:migration` has to work
/// in a CLI-only build. A clock before the epoch (only reachable on a badly
/// misconfigured machine) falls back to zero rather than panicking -- a
/// wrong-but-ordered filename beats a crashed generator.
fn utc_stamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let time_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60,
    );
    format!("{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}")
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a proleptic
/// Gregorian date. Reproduced because it is exact, branch-light, and shorter
/// than the dependency it would otherwise justify.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11], March-based
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(kind: MakeKind, name: &str) -> Artifact {
        plan(kind, &ArtifactName::parse(name).expect("valid name"))
    }

    #[test]
    fn every_kind_plans_a_rust_file_under_a_known_root() {
        for kind in MakeKind::ALL {
            let artifact = planned(*kind, "widget");
            assert_eq!(
                artifact.path.extension().and_then(|e| e.to_str()),
                Some("rs"),
                "{} did not plan a .rs file",
                kind.as_str()
            );
            assert!(
                !artifact.contents.trim().is_empty(),
                "{} planned an empty file",
                kind.as_str()
            );
            assert!(
                artifact.contents.ends_with('\n'),
                "{} planned a file without a trailing newline",
                kind.as_str()
            );
        }
    }

    #[test]
    fn a_nested_name_nests_the_generated_file() {
        let artifact = planned(MakeKind::Controller, "admin/users");
        assert_eq!(
            artifact.path,
            PathBuf::from("app/controllers/admin/users_controller.rs")
        );
        assert!(artifact.contents.contains("pub struct UsersController;"));
    }

    #[test]
    fn a_model_guesses_a_plural_table_name() {
        let artifact = planned(MakeKind::Model, "Category");
        assert_eq!(artifact.path, PathBuf::from("app/models/category.rs"));
        assert!(
            artifact
                .contents
                .contains("#[model(table = \"categories\")]")
        );
    }

    #[test]
    fn a_page_carries_the_name_the_developer_typed_as_its_contract() {
        let artifact = planned(MakeKind::Page, "users/show");
        assert_eq!(artifact.path, PathBuf::from("app/pages/users/show_page.rs"));
        assert!(artifact.contents.contains("#[page(\"users/show\")]"));
        assert!(artifact.contents.contains("pub struct ShowPage"));
    }

    #[test]
    fn a_command_name_uses_colons_where_the_path_used_slashes() {
        let artifact = planned(MakeKind::Command, "users/prune");
        assert!(artifact.contents.contains("#[command(\"users:prune\")]"));
    }

    #[test]
    fn a_migration_is_timestamped_and_asks_to_be_registered() {
        let artifact = planned(MakeKind::Migration, "create_users_table");
        let file = artifact.path.file_name().and_then(|n| n.to_str()).unwrap();
        assert!(
            file.starts_with('m'),
            "{file} is missing its ordering prefix"
        );
        assert!(file.ends_with("_create_users_table.rs"), "{file}");
        assert_eq!(artifact.notes.len(), 1);
        assert!(artifact.notes[0].contains("Migrator::migrations()"));
    }

    #[test]
    fn a_test_is_not_registered_in_a_module_tree() {
        let artifact = planned(MakeKind::Test, "checkout");
        assert_eq!(artifact.path, PathBuf::from("tests/checkout.rs"));
        assert!(!artifact.register_module);
    }

    #[test]
    fn the_two_blueprints_with_placeholders_say_so() {
        for kind in [MakeKind::Policy, MakeKind::Listener] {
            let artifact = planned(kind, "widget");
            assert!(
                !artifact.notes.is_empty(),
                "{} has a placeholder but no note",
                kind.as_str()
            );
        }
    }

    #[test]
    fn the_civil_calendar_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2000-02-29: the leap day the century rule keeps.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
    }

    #[test]
    fn the_migration_stamp_is_fixed_width_and_sortable() {
        let stamp = utc_stamp();
        assert_eq!(stamp.len(), 15, "{stamp}");
        assert_eq!(&stamp[8..9], "_");
        assert!(
            stamp
                .chars()
                .enumerate()
                .all(|(i, c)| i == 8 || c.is_ascii_digit()),
            "{stamp}"
        );
    }
}
