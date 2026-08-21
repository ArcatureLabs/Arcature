# Support

Four places, and which one you want depends on what you have. Picking the
wrong one is not a disaster, but it does mean waiting for someone to move it.

## "I think this is a bug"

Open a [bug report](https://github.com/ArcatureLabs/Arcature/issues/new?template=bug_report.yml).

Arcature treats a disagreement between code and its own documentation as a
defect in whichever of the two is wrong, so you do not have to decide which
before filing. You do have to say what you read and what it said -- that is the
whole difference between a report someone can act on and a report someone has
to reproduce from scratch.

"Version" means a released version -- `arcature 0.1.0` -- or, if you are
following `main` ahead of a release, the commit SHA you built from.
Feature flags matter more here than in most crates: they change which code
exists at all, and a report without them is frequently unreproducible.

## "The documentation is wrong, or missing"

Open a [documentation issue](https://github.com/ArcatureLabs/Arcature/issues/new?template=documentation.yml).

Same standard as a bug, separate template because the fix lands somewhere else.
A doc comment that promises a behaviour the code does not have is a defect and
is worth reporting as one.

## "How do I ..." and "why is it like this"

Ask in [Discussions](https://github.com/ArcatureLabs/Arcature/discussions).

Usage questions, design questions, and "is this the intended way" all belong
there. Two reasons: an issue that turns out to be a question is closed and lost,
while a discussion stays searchable for the next person with the same question;
and a question that reveals the guide is unclear is worth turning into a
documentation issue afterwards, which is easier from a thread than from a closed
ticket.

Before asking, the answer may already be written down. [The guide](https://arcaturelabs.github.io/Arcature/)
covers the intended path; `docs/decisions/` records the choices that are settled
and why, which is usually what "why is it like this" is asking about.

## "Arcature should do something it does not"

Open a [feature request](https://github.com/ArcatureLabs/Arcature/issues/new?template=feature_request.yml).

Read the template's preamble first. Most proposals are declined, and the
template asks the questions that decide it -- in particular what the feature
costs the people who never use it. If you are unsure whether the idea fits the
framework's shape, a discussion first will save you writing the proposal.

## "This has a security consequence"

Do not open an issue, a pull request, or a discussion. Follow
[SECURITY.md](SECURITY.md): the preferred route is GitHub's private
vulnerability reporting, on the repository's **Security** tab.

Anything that lets someone reach data or an action they should not is in this
category, including a case you are not sure about. A misfiled security report
is public the moment you press the button, and that cannot be undone.

## What support is not

Arcature is maintained by one person (see [GOVERNANCE.md](GOVERNANCE.md)).
There is no commercial support, no service-level agreement, and no guaranteed
response time outside the targets written into `SECURITY.md` for
vulnerabilities. Pre-release means `main` breaks without notice.

The most useful thing you can attach to any of the above is the smallest
reproduction you can manage. A failing test is ideal; a handler and the builder
calls around it is usually enough.
