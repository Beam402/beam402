<p align="center">
  <img src="assets/logo.png" alt="Beam402 — break the beam" width="140">
</p>

# Beam402

**Open source drag racing timing system** — staging beams, Christmas tree,
ET / 60ft / trap speed measurement, and race control software. Built from
industrial off-the-shelf components so that any club, anywhere, can run a
real drag event without a five-figure imported black box.

> **Status: prototype, week 1.** Nothing works yet. Architecture is designed,
> components are on order, bench validation is next. Follow the commits.

*Break the beam.*

## Why

This project started the day a national drag event was cancelled mid-finals:
the country's only timing system failed, and there was no second one. The
timing rig turned out to be a single point of failure for an entire racing
scene. Commercial systems exist — and cost like an engine build, ship for
months, and are serviced an ocean away.

Beam402 is the answer: an open, reproducible, field-repairable timing system
for grassroots drag racing — training days, junior events, alternative
leagues, and everything the big rigs never reach.

## What it is

- **Timing nodes** — identical boxes along the track (start, 60ft, 1/8,
  finish) that timestamp beam breaks with ≤1 ms resolution
- **Industrial through-beam photoelectric sensors** — 1 ms rated, IP67, with
  the range margin to survive burnout smoke; not hobby IR modules
- **A wired RS-485 trunk** with a broadcast start pulse — no clock sync, no
  radio jitter, verifiable with an oscilloscope
- **Christmas tree module** — full staging / AutoStart / pro & standard tree
  logic
- **Race control software** — classes, qualifying, ladders, time slips, and
  a local web scoreboard for spectators
- **Per-node battery power** — a cut cable loses data, never nodes

Design priorities, in order: trustworthy timing, field repairability by
non-specialists, low cost, incremental deployment (start + finish + tree is
already a complete working system).

## Documentation

| Doc | What's inside |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | Full system design: sensors, timing model, bus, power, nodes, tree — including the honest list of **unverified assumptions** that gate the project |
| [`docs/decisions.md`](docs/decisions.md) | Decision log (ADR): why wired and not wireless, why through-beam, why DIP addressing, why no photo finish, and what evidence would change each call |
| [`docs/bench-validation.md`](docs/bench-validation.md) | **The current stage.** How to build the rig and run the measurements that gate everything — procedures, expected values, pass/fail criteria, and what each failure would mean |
| [`docs/software.md`](docs/software.md) | Software architecture: what the node firmware, the race control software and the bench tooling each own — and what each is forbidden to do |
| [`docs/protocol.md`](docs/protocol.md) | The wire contract: Modbus register map, poll strategy, and the mapping file that gives the numbers their meaning |
| [`hardware/BOM.md`](hardware/BOM.md) | Prototype bill of materials with sourcing guidance |
| [`software/crates/protocol`](software/crates/protocol) | The register map itself, as code — `registers.toml` and the tables in `protocol.md` §3 are generated from it and checked against it (**D27**) |

## Roadmap

- [ ] **Bench validation** — sensor jitter rig (differenced against a
      reference detector), make/break edge asymmetry, thermal drift, hardware
      capture jitter, start-pulse noise immunity over full-length cable
      *(gates everything)*
- [x] Software design — architecture, bus register map, mapping file format
- [x] Wire contract as code — register map crate, documents generated and
      checked against it
- [x] Bench data-reduction script — [`bench/`](bench), VCD in, distribution out
- [x] Mapping file: load-time validation
- [x] Node and tree simulator, bus poller, race logic replayed against it —
      ET, splits, trap speed, reaction time, handicap starts and breakout
- [x] Time slips: `beam402 sim scenarios/bracket.toml --format bracket
      --dial 12.34,7.50` prints a full round against the simulator
- [x] Session recording and replay — `--record` writes every bus transaction,
      `beam402 replay` re-runs it through the same code and prints the same slip
- [x] `beam402 scope` — one self-contained page showing the strip, the tree, the
      beams, the bus and the events on a scrubbable timeline
- [x] Spectator scoreboard as a pixel frame at a declared resolution (**D29**),
      so the page previews a board instead of becoming a second design
- [ ] Two perfboard nodes (start + finish) + tree prototype
- [ ] Parking-lot demo: staging → tree → start pulse → ET on a laptop
- [ ] Carrier PCB (KiCad) + fab-assembled batch
- [ ] Field enclosures (IP67 assemblies), full two-lane configuration
- [x] Event layer: classes as data, qualifying, pro and sportsman ladders, byes
- [x] HTTP server, written not depended on (**D32**) — `beam402 serve` puts
      the round, the board and the scope on the LAN with zero dependencies
- [x] Live race control (**D30**): the bus on its own thread, an operator page
      under a control token, and a scoreboard — all from `beam402 serve`
- [x] Entry sheets and a meeting derived from an append-only result log —
      `beam402 event events/club-day.toml`
- [x] Eliminations run off the ladder — `beam402 serve --event <sheet> --log
      <file>` pairs the cars, the operator records, the ladder advances
- [x] The registration desk — `beam402 sheet entries.csv --event season.toml`
      turns the spreadsheet a club already has into an entry sheet (**D34**)
- [x] Carrying a day to a server — `beam402 push` and `beam402 host`, live or in
      bulk that evening, resumable and idempotent (**D33**)
- [x] Write authority on the receiver — the first writer claims an event, and
      `deploy/` is the reference way to put one on the internet
- [x] A read contract a league can build its own front end on — CORS on reads,
      an events index, and a pass-through key for their own ids (**D35**)
- [ ] Qualifying over the bus: time trials are a queue of single cars, not a
      smaller version of an eliminator
- [x] TLS on the push client — rustls behind a cargo feature, so a build that
      never leaves the track still has no dependencies (**D36**)
- [ ] Tree-hosted deployment (**D31**): a tree, two nodes and a phone — arm and
      read every run with no computer at the track
- [ ] A reference receiver actually deployed, so the chain runs end to end
- [ ] First real event

## Contributing

The project is young and the best time to influence it is now. Especially
welcome: eyes on the architecture and decision log (tell us where we're
wrong — with evidence), experience with photoelectric sensors and RS-485 in
the field, and later — firmware and race-control software contributions.
Open an issue; English or Russian both fine. Ground rules and how to
challenge a decision: [CONTRIBUTING.md](CONTRIBUTING.md).

Community chat on Telegram: [t.me/beam402](https://t.me/beam402) — mostly
Russian, English welcome. Conclusions that affect the design come back to
issues; the project's record lives on GitHub.

Related projects worth knowing: [MajicDesigns/DragLights](https://github.com/MajicDesigns/DragLights)
(tree lamp logic this project builds on) and
[RotorHazard](https://github.com/RotorHazard/RotorHazard) (the FPV drone
timing project whose open-community architecture is a reference for ours).

## License

Code: MIT. Hardware design files: CERN-OHL (added with the first PCB commit).

The Beam402 name and logo identify builds verified by the project — the
design is free to use, the name is not a free-for-all. A self-built system is
the builder's responsibility; a verification checklist will ship with v1.

---

*Документация проекта ведётся на английском. Краткий обзор на русском:
[README.ru.md](README.ru.md). Полевые регламенты и правила серии — на
русском отдельно.*