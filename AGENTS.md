# Working on Arcature as an agent

Rules for an AI agent making changes **to this repository**. If you are writing
an application *with* Arcature, read [`SKILL.md`](SKILL.md) instead.

[`.github/CONTRIBUTING.md`](.github/CONTRIBUTING.md) covers what a change
should look like — one file one responsibility, no hidden registry, document
the cost, `unsafe` forbidden. That still applies and is not repeated here.

This file is the other half: how to find out whether what you believe is true,
and which parts of this environment will lie to you. Almost every rule below
exists because it was learned the expensive way.

---

## 1. Verification: the rules that matter most

**A name existing is not a claim being true.** Checking that `Type::method`
resolves proves the sentence compiles, not that it is correct. Six guide
chapters passed a full mechanical check — every method resolved, every feature
flag real, `mdbook build` clean — and an adversarial read then found fifteen
false statements in them, including one that described a security property
backwards. Mechanical checks cannot catch a true-sounding sentence.

**Read the operator, not the intent.** A chapter said a clock failing to `0`
made every deadline expired. The code is `if now > expires_at`, so `0` makes
*nothing* expired: it failed open and the prose said closed. Work the
arithmetic with the real default values before describing a comparison.

**`let _ = x` drops immediately.** `_` is not a binding. Two separate bugs in
this repository were exactly that, both under comments claiming the value was
held. If you see `let _ = guard` or `let _span = span!(..)`, the thing is not
being held or entered.

**Grep the call site, not the definition.** A function that exists may never be
called. `Fluent::NUMBER` needs `add_builtins`, which nothing calls;
`DEFAULT_ENDPOINT` is declared and never passed. `MetricsLayer` existed for
months without the pipeline installing it.

**Open a CLI command; never read its name.** `arc queue work` sounds like it
drains the queue. It runs a *no-handler* worker that marks undispatchable jobs
dead.

**Check the re-export path in `mod.rs`.** `DENY_LIST` is at
`observe::redact::DENY_LIST`, not `observe::DENY_LIST`. An item is only
reachable where it is re-exported.

**A doc comment is a claim, not evidence.** Several comments in this repository
were wrong: the SSE guard "held for the lifetime of the stream" (it was
dropped), the limiter's critical section being "a hash lookup and three
arithmetic operations" (a full scan above 8192 keys), the `otel` feature
providing the Prometheus endpoint (it does not). When a comment and the code
disagree, the code is what runs.

### Never trust a subagent's report

Read the diff, the worktree, the actual files. Reports in this project have
been wrong in both directions, repeatedly:

- an agent reported work as unfinished that it had in fact completed;
- an agent's fact-check "corrected" a sentence that was already right, by
  mis-ordering an interval's first tick;
- a fact-check ran against a summary the agent returned rather than the file
  it had written, so its findings did not exist in the file;
- agents wrote files they were told only to return, and left a stray
  `tests/zz_tmp_check.rs` behind.

Check `git status` after any agent runs. Verify every claim you intend to act
on. A false alarm costs more than a missed finding, because somebody acts on it.

### Prove a test is not vacuous

A test that passes proves nothing until you know it can fail. For a regression
test, revert the fix and watch the test fail on the assertion you care about —
do not reason about it. Both bug fixes in `0.1.2` were verified that way, and
in one case the reasoning would have been right and the proof was still cheap.

Prefer a test that isolates: when the SSE test runs against the old code, the
inheritance assertion fails while the neighbouring assertion still passes.

---

## 2. This environment will lie to you

**Disk below ~8 GB produces false compile errors.** ENOSPC at link time
surfaces as errors that look like broken code. It has cost a full debugging
detour here. Check `df -h /c` before diagnosing a red build; clean
`target/debug/incremental` first (it is regenerable and often several GB).

**A code-integrity policy can reject a freshly linked test binary.** The
symptom is a single target failing that passes when re-run, with `Compiling
arcature` in the output — a relink produces a new hash and is allowed. Re-run
the single failing target before reading any source.

**`just` does not run on this machine.** Application Control blocks it. Every
gate in the documentation written as `just ci` must be run as the underlying
`cargo` commands. `docs/RELEASING.md` spells them out.

**`exit code 0` from a chained shell command is meaningless without `set -e`.**
The exit code is the *last* command's. A gate script that ran five steps and
reports 0 may have failed the first four.

**`cargo test` stops at the first failing target.** Everything after it,
including every doctest, does not run — which reads as "did not fail". Always
`--no-fail-fast`. This is not tidiness; a red early step has twice hidden the
question actually being asked.

**The Bash tool mangles command strings.** `\\` collapses, `\r` vanishes, and
apostrophes or backticks inside a quoted heredoc can break it outright. Both
have corrupted files here — once writing a real NUL byte into `CHANGELOG.md`.
For anything multi-line carrying backslashes, apostrophes or backticks, use the
Write or Edit tool, or write a script to a file and run it by path. In embedded
Python use `chr(92)`.

**A file written by a script has not been formatted.** Run `cargo fmt --all`
after. The fmt gate is the first CI step and the last place you want to learn
this; it has caught the same mistake twice.

**`python3` does not exist here.** Only `python`. `python3` hits a Windows
Store stub that prints one line and exits without running anything.

**Four logical cores.** Two concurrent agents is the ceiling. The same `cargo
clippy` took 49m54s with four agents compiling beside it and 3m34s idle. Never
take a timing measurement while anything else is building.

---

## 3. Repository invariants

**`--all-features` is designed to fail.** Three mutually exclusive database
drivers. The 22 compile errors are the invariant working. The honest
equivalent is one `cargo check` per driver over every other feature.

**Powerset numbers are measured, never calculated.** `cargo hack` prunes
mutually exclusive combinations, so combinatorics gives the wrong answer
(C(26,2)+27 = 352 against a real 263). Re-measure after any feature change:

```
cargo hack build --feature-powerset --depth 2 --features db-postgres \
  --skip database,db-sqlite,db-mysql --exclude-all-features --keep-going \
  --print-command-list | grep -c .
```

The numbers live in `ci.yml` (4 places), `justfile` (4) and `codeql.yml` (1).
The documented feature count is total `[features]` entries minus `default` and
`fullstack`.

**The pipeline order is a contract — but check what it actually asserts.** The
test pins *relative* order (production stages wrap user layers), not a fixed
list or the number 23. Adding an opt-in layer is additive and allowed; adding
a numbered *stage* renumbers a table in four places, so prefer sharing an
existing stage when the new layer genuinely belongs at that depth.

**Patch releases are additive only.** Under `0.x` the minor is the breaking
bump. No removals, no signature changes, no changed defaults. A new public
enum or struct gets `#[non_exhaustive]` from the start. A new capability gets a
new feature flag, off by default.

**A behaviour change in a patch goes in `### Fixed` with the change spelled
out.** "A fixed bug that alters what an unchanged application does is still a
behaviour change." If a reader upgrading by patch could be surprised, the entry
is not finished.

**The CHANGELOG has an anchoring trap.** `### Added`, `### Fixed` and
`### Security` each appear twice — once under `Unreleased`, once under a
released version. Anchor on the first heading *after* the section you are
editing, or you will rewrite history. Section order: Added, Changed,
Deprecated, Fixed, Security, Performance, Documentation.

**Every commit carries a CHANGELOG entry, or says why not.** "No CHANGELOG
entry: a CI baseline, with nothing in it for a reader weighing an upgrade" is a
complete answer. Silence is not.

**One logical change per commit.** All-in-one commits are not acceptable here.
The commit message explains the *why* and the evidence, not the *what* — the
diff already says what.

**Prose style differs between source and guide.** Source comments and this file
use `--`. The guide under `docs/src/` uses a real em dash, because mdBook's
smart punctuation turns `--` into an *en* dash and half the book would disagree
with the other half. Ten chapters got this wrong once; 305 lines had to be
converted.

---

## 4. Commands that actually work here

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast
cargo deny check
cargo doc --no-deps --features fullstack
cd docs && mdbook build

# one driver at a time; never --all-features
cargo check --no-default-features --features "<feats>,db-postgres" --all-targets

# off-by-default features have tests nothing else runs
cargo test --no-fail-fast --features crypt,signed-urls,uploads,views,i18n,...

# database tests read this, not DATABASE_URL, and skip without it
ARCATURE_TEST_DB_URL=... ARCATURE_REQUIRE_TEST_DB=1 cargo test --lib

# the load suite is opt-in and takes minutes
ARCATURE_LOAD=1 cargo test --features test-kit --test load_profile -- --test-threads=1
```

Releases: follow [`docs/RELEASING.md`](docs/RELEASING.md) exactly. It carries
steps that are not obvious, including that the unsafe baselines contain the
version string and go red on the *next* push if you forget them.

---

## 5. What not to do

- Do not invent an API, a feature name, a default or a number. Say you do not
  know.
- Do not soften a wrong statement you find. Replace it, and say what it said.
- Do not delete a wrong entry silently when it survived a release — record that
  it was wrong. `SECURITY.md` does this and it is the most useful thing that
  table has said about itself.
- Do not touch `src/application/pipeline.rs` stage *order* without stopping and
  surfacing it. Adding an opt-in layer is fine; reordering is not.
- Do not run `cargo test --all-features` or `cargo clippy --all-features`.
- Do not push a workflow you have not reasoned through end to end. A green
  dependency graph on paper hid a real ordering bug here until a slow job
  exposed it.
- Do not measure anything while another build is running.
- Do not report work as finished without running the gate. "Tests pass" means
  you ran them and read the output, not that they should.

---

## 6. A pattern worth repeating

Both bugs fixed in `0.1.2` were found by **writing documentation that described
what the code does rather than what it was meant to do**, and then reading the
source when the prose and a code comment disagreed. Both times the comment was
wrong.

If you are documenting a subsystem and find yourself writing something that
contradicts a comment beside the code, do not resolve it in the prose. Go and
find out which one is true.
