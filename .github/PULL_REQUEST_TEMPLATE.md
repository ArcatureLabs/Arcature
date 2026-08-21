## What this changes

<!--
Describe the defect or the gap, then the change. Write it the way the commit
messages in this repository are written: prose that explains why, naming the
thing that was wrong rather than the symptom it produced. `git log` has the
examples.
-->

## Why

<!--
The reasoning. If the change is a fix, say what the code did that its own
documentation said it would not. If it is a feature, say what could not be
built without it.
-->

## The cost

<!--
What this costs someone who never uses it: compile time, a dependency, a
feature flag, a new concept in the vocabulary, a paragraph in the guide, a
narrower escape hatch. Every change here is paid for by somebody. If the answer
is genuinely "nothing", write "nothing" -- an empty section reads as an
oversight.
-->

## Deviations

<!--
Anything you did differently from the issue, the plan, or the surrounding
convention, and why. A deliberate deviation stated here is a decision; the same
deviation discovered in review is a defect.
-->

## Checklist

- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --all-targets` is clean with `RUSTFLAGS="-D warnings"`
- [ ] `cargo test` passes (needs PostgreSQL; see CONTRIBUTING.md)
- [ ] `cargo hack check --each-feature --no-dev-deps` passes, if features changed
- [ ] New behaviour has a test that fails without the change
- [ ] Behaviour that could fail silently is pinned by a test, not only by a comment
- [ ] Public items are documented, and the documentation matches what the code does
- [ ] `CHANGELOG.md` updated under `## [Unreleased]`, if the change is user-visible
- [ ] No new npm package or JavaScript runtime shipped from this repository
  (`docs/decisions/0001-no-npm-package.md`)
- [ ] No global registry, link-time collection, `TypeId` map or thread-local
  added (`docs/decisions/0005-no-hidden-registry.md`)
- [ ] No `unsafe` (the crate forbids it)

## Related

<!-- Issue numbers, ADRs, prior pull requests. -->
