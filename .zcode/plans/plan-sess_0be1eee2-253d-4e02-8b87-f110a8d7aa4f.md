# Plan: Port `arcature-dx`, `arcature-build`, `arcature-test` (NestJS/AdonisJS DSL direction)

User confirmed: port đầy đủ theo hướng NestJS/AdonisJS — bộ cú pháp DSL đầy đủ riêng thống nhất (`module!`/`application!`/`routes!` + attribute macros như decorators). Đây là port 3 crate còn sót + graph types foundation mà chúng generate code against.

## Context (từ exploration)

- Old `arcature` facade có module `dx` (file `dx.rs` + 20+ files) chứa graph types + runtime contracts mà macros generate code against. New framework **hoàn toàn thiếu**.
- Old `arcature-dx` (15 macro, 704 dòng) — proc-macro crate, NO `arcature` dep (tránh cycle), expansions dùng `::arcature::` absolute paths.
- Old `arcature-build` (UAG) — dev/tooling-only, KHÔNG required để app run. 2 layer: A (schema/manifest/codegen/validate/compile) zero coupling, B (from_graph) coupled to dx graph types.
- Old `arcature-test` — self-contained, NO dep trên dx/build. TestApp + fixtures + assertions.
- `module!`/`application!` KHÔNG build/run app — chỉ validate graph + produce metadata. App vẫn build bằng `ApplicationBuilder`, gọi `app_graph()` làm validation gate.

## Workspace structure (giữ pattern macros/)

4 workspace members:
- `.` (arcature, single package, batteries included)
- `macros/` (arcature-macros — đã có)
- `build/` (arcature-build — NEW, build/dev tooling, publishable)
- `test/` (arcature-test — NEW, test toolkit, publishable cho user apps `[dev-dependencies]`)

## Phase A: Port `dx` module vào arcature (`src/dx/`, feature `dx`)

Foundation cho mọi macro + UAG. Port từ old `arcature/src/dx.rs` + `dx/*.rs`.

Files mới (chỉ những gì new framework thiếu — auth/session/policy/event/job đã có tương đương trong `src/auth`, `src/events`, `src/jobs`):
- `src/dx/mod.rs` — re-exports
- `src/dx/application_graph.rs` — `ApplicationGraph`, `GraphError` (duplicate/unknown-import/cycle validation)
- `src/dx/graph.rs` — `ModuleDescriptor`, `ListenerBinding`, `JobBinding`, `CommandBinding`, `ScheduleBinding`, `ScheduleCadence`
- `src/dx/route_metadata.rs` — `RouteDescriptor`, `RouteMethod`
- `src/dx/controller_metadata.rs` — `ControllerMetadata`, `ControllerMethod`
- `src/dx/field_metadata.rs` — `FieldShape`, `RequestMetadata`, `ResourceMetadata`
- `src/dx/dx_component.rs` — `DxComponent` marker trait (NAME const)
- `src/dx/route_model.rs` — `RouteModel` trait + `Bound<T>` extractor (feature `db`+`api`)
- `src/dx/resolve.rs` — `Resolve<S>`, `Inject<T>` (DI)
- `src/dx/service.rs` — `Service` marker (extends DxComponent + DEPS)
- `src/dx/provider.rs` — `Provider` marker (Error + DEPS)
- `src/dx/command.rs` — `Command` trait (feature `jobs`)
- `src/dx/response.rs` — `Page<T>`, `Empty`, `Json` (feature `serde`)
- `src/dx/validated.rs` — `Validated<T>` (delegates `src/validation`)

Feature `dx` trong Cargo.toml: pulls serde/api/db/jobs theo cross-feature gating (như old).

## Phase B: Port `arcature-dx` macros vào `macros/` (13 file macro mới)

`macros/` đã có: `model.rs`, `request.rs`, `controller.rs`, `job.rs`, `event.rs`, `listener.rs` + `lib.rs` + `util.rs`.

Thêm 13 file (một file = một macro, giữ rule "mỗi FILE chỉ chứa đúng 1 chức năng"):
- `macros/src/component.rs` — `#[derive(DxComponent)]`
- `macros/src/module.rs` — `module!` bang (parse/expand/validate)
- `macros/src/application.rs` — `application!` bang
- `macros/src/routes.rs` — `routes!` bang (DSL lớn nhất, parse/expand/router+metadata+helpers)
- `macros/src/redirect.rs` — `redirect!` bang
- `macros/src/page.rs` — `#[page("name")]` attribute
- `macros/src/page_macro.rs` — `page!` bang
- `macros/src/resource.rs` — `#[resource]` attribute
- `macros/src/route_model.rs` — `#[route_model]` attribute
- `macros/src/service.rs` — `#[service]` attribute
- `macros/src/provider.rs` — `#[provider]` attribute
- `macros/src/policy.rs` — `#[policy(Model)]` attribute
- `macros/src/middleware.rs` — `#[middleware]` attribute
- `macros/src/diagnostic.rs` — ARC-M00x error codes (shared helper)
- `macros/src/field_shape.rs` + `macros/src/schema.rs` — shared helpers (type→PropsSchema mapping)

Re-export trong `macros/src/lib.rs` + `arcature/src/lib.rs` (feature `dx`), thêm vào `src/prelude.rs` (giống `controller`/`model`/`request` đã làm).

## Phase C: Port `arcature-build` vào `build/` workspace member

- `build/Cargo.toml` — features: `default=[]` (compile only), `uag-schema` (serde), `uag` (pulls arcature `dx`+`serde`).
- `build/src/lib.rs` — `compile()` + `pub mod uag`
- `build/src/compile.rs` — Cargo build-script directives (Layer A, verbatim)
- `build/src/uag/{mod,schema,version,manifest,codegen,validate,error}.rs` — Layer A verbatim (owned serde mirror, TypeScript generators, cross-stack validator)
- `build/src/uag/from_graph.rs` — Layer B REWRITE cho new graph types (`src/dx/ApplicationGraph` → `Uag`)

## Phase D: Port `arcature-test` vào `test/` workspace member

- `test/Cargo.toml` — features: `db`, `cache`, `mail`, `storage`, `jobs`, `auth` (additive, map sang arcature subsystems)
- `test/src/lib.rs` — `TestApp`, modules, re-export `axum`/`tower`
- `test/src/app/{mod,start,stop,cookie_jar}.rs` — TestApp (bind Router 127.0.0.1:0, reqwest, cookie jar)
- `test/src/http/{mod,request,response,assertions}.rs` — TestRequest/TestResponse + fluent assertions
- `test/src/{assert,safety,error,events,factories,pages,inertia}.rs` — self-contained helpers
- `test/src/{db,cache,mail,storage,jobs,auth}/mod.rs` — fixtures, map sang new subsystem types:
  - `TestDb` → `src/database::Db` + `DbConfig`
  - `TestCache` → `src/cache::Cache` + `CacheConfig` + `Namespace`
  - `TestMailer` → `src/mail::Mailer::capture_ok`
  - `TestStorage` → `src/storage::Storage` + `StorageConfig::fs`
  - `TestJobs` → `src/jobs::Jobs` + `Registry` + `Worker`
  - `test_session_config` → `src/auth::SessionConfig` + `SessionKey`

## Phase E: Templates + CLI + integration

- Update generated app template: dùng DSL (`module!`, `application!`, `routes!`, `#[controller]`, `#[service]`, `#[resource]`, `#[page]`, etc.) thay cho ApplicationBuilder trực tiếp — vẫn dùng ApplicationBuilder trong `run.rs` nhưng gọi `app_graph()` làm validation gate (giống old template pattern).
- Add `arc dev` command (cho dev-proxy + Vite IPC lifecycle — spawn Vite middlewareMode, set `ARCATURE_VITE_IPC`, run app).
- Add `arc inspect`/`arc routes`/`arc modules`/`arc controllers` (gọi metadata bin + deserialize UAG).
- Biên dịch (`--all-features`, `--no-default-features`), clippy, full test suite.
- Regenerate app + `cargo check` + `cargo test` trên generated app.
- Conventional commit: `feat: port dx DSL macros, build UAG, test toolkit from old arcature (NestJS/AdonisJS-style unified DSL)`.

## Verification gates (mỗi phase)

- Phase A: `cargo check --features dx` sạch.
- Phase B: `cargo check --all-features` + `cargo test --all-features` (macro unit tests).
- Phase C: `cargo check -p arcature-build --features uag` sạch.
- Phase D: `cargo check -p arcature-test --all-features` + `cargo test -p arcature-test` (HTTP/assertion/inertia tests offline; fixtures need PG/Redis, gated).
- Phase E: generated app `cargo check` + `cargo test` sạch; `--all-features` framework clean; clippy clean.

## Order

Tuần tự A → B → C → D → E (mỗi phase build trên trước; commit riêng mỗi phase để dễ rollback nếu conflict). Đây là port lớn (~3000+ dòng) nhưng mechanical (port verbatim + điều chỉnh `::arcature::` paths + map subsystem types), rủi ro architecture thấp vì old design đã tách 2-layer sạch.