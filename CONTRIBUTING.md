# Contributing to Beam402

Beam402 is at the earliest possible stage: the architecture is designed,
nothing is validated yet. This is the best moment to influence the project —
a good argument today is worth more than a thousand lines of code next year.

Issues and discussions are welcome in **English or Russian**. Repository
documentation is kept in English (see the note at the bottom of the
[README](README.md)).

## What helps most right now

1. **Review of the architecture and decision log.** Read
   [`docs/architecture.md`](docs/architecture.md) and
   [`docs/decisions.md`](docs/decisions.md) and tell us where we're wrong.
2. **Field experience.** Photoelectric sensors outdoors, RS-485 in noisy
   environments, IP-rated enclosures that actually survived a season,
   drag-strip officiating — first-hand stories are engineering data here.
3. **Bench validation help.** The open questions in `architecture.md` §11
   gate everything. If you can run one of those tests, that's a top-tier
   contribution.
4. Later phases: ESP32 firmware, race control software, KiCad boards. Watch
   the roadmap in the README.

## How to challenge a decision

Every significant choice is recorded in [`docs/decisions.md`](docs/decisions.md)
with its reasoning and an explicit "what would change it" section. To argue
against one:

- Open an issue referencing the decision (e.g. `D01`).
- Bring evidence: a datasheet, a scope trace, a field failure story, a
  price/availability reality check. "I'd have done it differently" is an
  opinion; "here's a measurement" is a contribution.
- If the argument stands, the decision gets amended or reversed — with your
  evidence recorded in the log.

Decisions marked **revisit** are explicitly open; even **accepted** ones fall
to good data.

## Issues and pull requests

- **Open an issue first** for anything non-trivial — the project is moving
  fast and a heads-up prevents wasted work.
- Documentation fixes (typos, clarity, broken links) — a direct PR is fine.
- Code and hardware PRs will make sense once the corresponding phase starts;
  until bench validation passes, the design documents *are* the project.

## Licensing of contributions

By contributing you agree that your contribution is licensed under the
project's licenses: [MIT](LICENSE) for code and documentation, CERN-OHL for
hardware design files (added with the first PCB commit). Note that the
Beam402 name and logo are reserved for project-verified builds — see the
README's license section.

## Conduct

Be direct about engineering, decent to people. Details:
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
