# Governance

Arcature has one maintainer. This document says what follows from that, because
a project that is honest about its bus factor is easier to depend on than one
that publishes an org chart nobody staffs.

## Current status: paused

As of **24 August 2026** development is on hold until there is someone to
carry it. Issues and pull requests may go unanswered. `0.1.3` stays on
crates.io and is not yanked, because it works for what it does; this is a
pause, not the wind-down described under
[If the maintainer disappears](#if-the-maintainer-disappears). The README
lists what is known broken.

The rest of this document describes how the project runs when it is running,
and is what a fork inherits.

## Who decides

**Maintainer: [@dismonjames](https://github.com/dismonjames).** Final say on
what the framework is, what it declines to be, what merges, and what is
released. There is no steering committee, no technical oversight board, and no
vote. Where you see "we" in this repository's prose, read "the maintainer, and
whoever showed up".

That is not a permanent structure, it is the current one. The sections below on
becoming a maintainer and on continuity exist so that it can stop being true
without the project stalling.

## How decisions get made

Most changes need no ceremony. Someone opens a pull request, it is reviewed
against `CONTRIBUTING.md`, and it merges or it does not. The interesting cases
are the ones where the disagreement is about direction rather than code:

**The framework's shape is the constraint.** Arcature is one crate with an
opinion, and the recurring judgement is not "is this good" but "does this belong
here". A proposal is weighed against what it costs the people who never use it:
compile time, a dependency, another feature flag, another concept in the
vocabulary, another paragraph in the guide. That question is on the feature
request template because it is the question that decides most of them.

**Disagreement is settled in public, in the thread.** Issues and Discussions are
where an argument gets made; nothing is decided in private mail. If the
maintainer overrules a contributor, the reason is written where the contributor
can read it, and "I prefer it the other way" is not one.

**Being right beats being the maintainer, when there is evidence.** The
decisions in `docs/decisions/` each name the cost they accept. Demonstrating
that a cost is worse in practice than the record claims is how a settled
decision gets reopened -- a benchmark, a reproduction, a real application that
hit the wall. A preference is not evidence.

**Silence is not consent.** An unanswered proposal has not been accepted. If a
thread has gone quiet for a fortnight, say so in it; the backlog is one person's
attention, and things fall off it.

## Decision records

A decision record -- an ADR -- is a short file in `docs/decisions/` that states a
choice, the context that forced it, and the cost paid. It exists so nobody has
to reconstruct the reasoning from a diff two years later, and so a settled
question stays settled without being re-argued from scratch every time a new
contributor meets it. There are five:

| # | Decision |
|---|---|
| [0001](../docs/decisions/0001-no-npm-package.md) | Arcature publishes no npm package |
| [0002](../docs/decisions/0002-xsrf-token-cookie.md) | The CSRF cookie is `XSRF-TOKEN`, not `__Host-csrf` |
| [0003](../docs/decisions/0003-one-tcp-port.md) | Exactly one TCP port, in development as well as production |
| [0004](../docs/decisions/0004-layer-order-contract.md) | Layer order is a written contract |
| [0005](../docs/decisions/0005-no-hidden-registry.md) | There is no hidden registry |

A record is **required** when a change:

- makes a choice that later code will be built on top of, so that reversing it
  later means reversing everything above it -- the layer order and the absence
  of a registry are both that shape;
- is a deliberate refusal. "Arcature will not do X" is invisible in the
  codebase, so a reviewer, or the maintainer in a year, has nothing to read but
  the absence;
- accepts a known cost on purpose, particularly a security-relevant one. Both
  hardening decisions named as out of scope in `SECURITY.md` are argued in a
  record, and that is what makes "this is deliberate" checkable rather than
  asserted;
- contradicts, narrows or amends an existing record. Amendment is normal;
  amendment without a written reason is how a project loses the plot.

A record is **not** required for a bug fix, a refactor that preserves behaviour,
or a feature that follows the existing grain. If you are unsure, open an issue
before writing the code -- deciding whether the choice is load-bearing is
usually faster than the code is.

Records are numbered sequentially, kept short, and written in the past tense of
a decision already taken. Add the row to `docs/decisions/README.md` in the same
pull request; a record nothing indexes is a record nobody finds.

## Becoming a maintainer

There is no application form and no probation period, because there is no
process worth calling one at this size. What actually happens:

Someone reviews other people's pull requests, and the reviews are good enough
that the maintainer stops re-reviewing behind them. Someone owns an area --
`macros/`, the job queue, the CLI -- across several changes rather than one.
Someone's judgement about what does *not* belong in the framework matches the
project's, which is harder and rarer than writing code that works.

At that point the maintainer offers commit access. It is an invitation, not a
reward for volume: a contributor with one excellent change is welcome, and is
not thereby a maintainer.

A new maintainer's name is added to `.github/CODEOWNERS` for the areas they own
and to the enforcement contact in `CODE_OF_CONDUCT.md`, and this document is
updated to stop saying "one maintainer". Those edits are the appointment; there
is nothing else to it.

Maintainers who go quiet for a long stretch are moved back to contributor status
without prejudice, and can have access back by asking. Access that nobody is
watching is a security problem, not a courtesy.

## If the maintainer disappears

This is the honest failure mode of a single-maintainer project, and it is worth
writing down while nothing is wrong.

**The licence already covers you.** Arcature is Apache-2.0. Anyone may fork it,
rename it, publish the fork and continue -- no permission needed, now or later.
That is the actual guarantee, and it does not depend on anyone answering their
email.

**Nothing here is locked to a person.** Every build and release step lives in
`.github/workflows/` and runs from the repository. The reasoning behind the
design lives in `docs/decisions/` and in the module headers rather than in one
person's head. A fork inherits a working project, not a puzzle.

**What a fork cannot inherit** is the `ArcatureLabs/Arcature` repository, the
`arcature` and `arcature-macros` names on crates.io once they are published, and
the `arcature.dev` domain. A fork needs its own names. Say plainly that it is a
fork, and do not publish it under a name that suggests it is this project
continued -- the licence permits it, and it would still be a lie to users.

**If the project is being wound down deliberately**, rather than silently, the
README says so, the crate is yanked or marked as unmaintained on crates.io, and
a successor fork is linked if one exists. Going quiet without doing that is a
failure by the maintainer, and this paragraph is here to make it a nameable one.

## Related documents

- `CONTRIBUTING.md` -- how to build, test and shape a change.
- `SUPPORT.md` -- where to ask what.
- `CODE_OF_CONDUCT.md` -- behaviour, and how it is enforced.
- `SECURITY.md` -- private reporting, scope, and what a reporter is promised.
- `docs/decisions/` -- the choices that are settled, and why.
