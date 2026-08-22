# 0006 — The generated TypeScript stays derived

## Decision

`resources/js/generated/` is not committed, and the scaffold's own pages do not
import from it. `arc typegen` writes the directory from the application graph,
`arc dev` rewrites it after every restart, `arc build` writes it before Vite
runs, and `.gitignore` and `.dockerignore` both keep it out of anything that
travels.

The consequence, and the thing this record exists to settle: the type-safe route
helper is **opt-in**. A stock application gets `route()` the moment it writes
`import { route } from '@/generated'`, and not before. Nothing in the scaffold
writes that import on its behalf.

## Context

The helper is worth wanting on by default. `arc typegen` emits `route()` typed
over a union of route names, with parameterised routes demanding their
parameters, so renaming a route in Rust turns every stale call site red. In a
scaffold that never imports it, renaming a route leaves `tsc` clean — the
guarantee is real and switched off, which is the wrong way round for a framework
default.

Turning it on means the scaffold's example page calls `route('home')`. That
import has to resolve, and there are three places in the scaffold's own life
where the directory it resolves to does not exist:

- **A fresh clone.** The directory is gitignored, and the ignore file says why:
  it is derived from the Rust source, so committing it means two sources of
  truth for the same names. Open a freshly cloned project in an editor and the
  first thing the TypeScript server reports is an unresolved import on line one
  of the example page.
- **The image build.** `Dockerfile` stage 1 is `node:22-bookworm-slim`: it
  copies `resources`, runs `npm ci` and `npm run build`, and has no Rust
  toolchain to produce the directory with. `.dockerignore` excludes
  `resources/js/generated` outright, so a copy sitting on the developer's disk
  never reaches the build context either. This is not an oversight in either
  file — the three-stage split exists so that neither toolchain ends up in the
  runtime image, and the asset stage is first because assets change least often.
- **Any frontend-only job.** Same shape as the image build: Node in hand, no
  graph to read.

`arc build` has none of this problem, because it *is* the four stages in order —
graph, typegen, cargo, vite — and the Dockerfile deliberately does not use it.

So default-on needs one of two changes, and both cost more than the guarantee is
worth today.

**Commit the directory.** Then a clone has it, the asset stage has it, and the
editor is quiet. It also makes the guarantee unsound. `tsc` would be checking
call sites against a snapshot rather than against the graph, and a snapshot goes
stale silently: rename a route, forget to regenerate, commit, and `routes.ts`
still swears the old name exists. `tsc` passes and the application 404s at
runtime — precisely the failure the helper was bought to prevent, now with a
green check on top. Restoring soundness needs a `--check` gate in the shape of
`cargo fmt --check`, and a gate needs somewhere to run: `arc new` generates no
CI workflow, so there is no pipeline in a stock project to fail.

**Put Rust in the asset stage.** Then the least-changing stage depends on the
most-changing source, every frontend edit drags a Rust image layer behind it, and
the reason the stages were split in the first place is gone.

Neither is a change to make in passing while resolving a scaffold import.

What the scaffold does instead is make the opt-in reachable rather than merely
available. `tsconfig.json` already maps `@/generated` and `@/generated/*`, so the
import works the moment the directory exists. `justfile` already has `check-ts`,
which runs `arc typegen` and then `npx tsc --noEmit` — in that order, so the
sound path is one command and the file is always fresh when it is read. The
example page carries a comment saying what to import and when to switch.

## Cost

**The default is still the weaker one, and saying why does not change that.** A
developer who never reads the comment, never opens the guide and never runs
`just check-ts` gets no route-name checking at all, and will find out about a
renamed route from a 404. Every argument above is about the price of the fix,
not about the problem being small.

**The revisit condition is real and it is not scheduled.** When the scaffold
grows a CI workflow, the `--check` gate has a home, committing the directory
stops being unsound, and this record should be reopened — that is new evidence
in the sense `docs/decisions/README.md` means. Until then the decision rests on
an absence, which is the weakest thing a decision can rest on.

**Two ignore files now encode a design decision between them.** `.gitignore` and
`.dockerignore` each exclude `resources/js/generated`, for related but not
identical reasons, and nothing links them to this record. Someone deleting a line
from either one to fix a build will not find out what it was holding up.
