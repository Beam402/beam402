# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

Beam402 is an open source drag racing timing system: beam sensors, Christmas
tree, ET / 60ft / trap speed measurement, and race control software, built
from industrial off-the-shelf parts.

**The repository currently contains no code.** It is design documentation at
the pre-validation stage: architecture, a decision log, and a prototype BOM.
There is nothing to build, lint, or test — do not invent build commands or
claim that any subsystem works. `.gitignore` reserves space for the intended
stack (ESP-IDF/PlatformIO firmware, Python race control, Node web scoreboard,
KiCad hardware), but none of it exists yet.

Until bench validation passes, **the design documents *are* the project** — so
edits to them are the substantive work, not paperwork around it.

| File | Role |
|---|---|
| `docs/architecture.md` | Full system design, §11 = ranked list of unverified assumptions, §12 = deployment stages |
| `docs/decisions.md` | ADRs `D01`–`D15`: context → decision → why → what would change it |
| `hardware/BOM.md` | v0 prototype BOM (bench + parking-lot demo), organized by supplier basket |
| `README.md` / `README.ru.md` | English canonical, Russian overview |

## Design invariants

These are load-bearing. Firmware, docs, and hardware work must not quietly
contradict them; changing one means amending its decision record first.

- **No clock synchronization** (D04). The start node broadcasts a hardware
  pulse on a dedicated differential pair; each node runs a local counter from
  that pulse to its own beam edge, so every split is measured by one clock.
  Anything that reintroduces cross-node time transfer breaks the model.
- **The start pulse is a fixed 5 ms width, not a bare edge.** Nodes validate
  the width — that check is the defense against ignition noise coupled into
  400 m of cable.
- **Wire where time lives** (D01). Radio is acceptable for the spectator
  scoreboard and arbitration video, never for the pulse or timing data.
- **Timestamps come from hardware capture** (D13), never from polling in a
  main loop; radios are disabled in firmware.
- **Bus pass-through is independent of node electronics** (D09). Nodes are
  multi-drop taps, never repeaters — a dead or gutted node must not break the
  bus, and repeating would add per-node jitter.
- **Identity is bound to track position, not silicon** (D08). DIP switch = bus
  address; the factory MAC is inventory/logging only. The meaning of "node N,
  input M" lives solely in the mapping file on the race control PC — never in
  node flash.
- **One node design for every position** (D07). The Christmas tree and the
  operator console are separate modules by design; resist adding
  position-specific variants of the universal node.
- **Beams are the timing source; the camera is evidence** (D12). The camera
  never integrates with the bus — sync is optical via a marker LED in frame.
- **Fully functional with no internet.** Cloud features are strictly additive.

## Working rules

**Validation gates purchases and promises** (D15). No batch orders, no PCB
fabrication, no public timelines until the bench answers whether the full path
(beam → sensor → optocoupler → capture) holds < 1 ms total jitter. When
writing about the project, keep the honest register the README already uses —
status first, "nothing works yet" where that is true.

**Design changes go through the decision log.** To change a recorded choice,
amend or reverse the ADR rather than silently editing `architecture.md`;
record the evidence that moved it. Documented status values are **accepted**
and **revisit**. `CONTRIBUTING.md` promises contributors that decisions fall
to measurements, not opinions — hold your own proposals to that bar: a
datasheet, a scope trace, a field failure, or a price/availability check.

**Prefer the honest unknown.** §11 of `architecture.md` exists because
unverified assumptions are tracked, not smoothed over. New uncertainty belongs
there, ranked by risk, with the test that would settle it.

## Documentation conventions

- Repository documentation is written in **English**. `README.ru.md` is a
  Russian overview and must be updated alongside `README.md` when shared facts
  change (links, status, roadmap). Issue forms in `.github/ISSUE_TEMPLATE/`
  are bilingual EN/RU — keep both halves in sync.
- Prose is hard-wrapped at **≤ 80 columns**. Markdown tables and HTML blocks
  are exempt.
- Cross-reference by anchor and identifier: `docs/architecture.md` §2,
  decision `D08`. Decision IDs are permanent — never renumber them.
- Sourcing in `BOM.md` uses generic search terms, not store links, so the
  system is buildable from any regional distributor. Preserve that when
  editing.
- Units carry both systems where drag racing practice is imperial (`7 in
  (178 mm)`), because the numbers are quoted from established strip practice.

## Licensing

Code and documentation: MIT (`LICENSE`). Hardware design files: CERN-OHL,
landing with the first PCB commit. The Beam402 name and logo identify
project-verified builds — the design is free to reuse, the name is not.
