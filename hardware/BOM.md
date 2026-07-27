# Beam402 — Prototype BOM (v0 bench + parking-lot demo)

Bill of materials for the **v0 prototype**: two timing nodes (start + finish),
one tree prototype, full-length trunk cable, bench validation rig. This is
*not* the production BOM — quantities and grades are chosen to answer the
open questions in `docs/architecture.md` §11 with minimal spend.

Approximate total: **$400–500**, roughly half of it sensors — an honest
reflection of where the system's accuracy lives.

Sourcing notes use generic search queries rather than store links: the
project should be buildable from any regional industrial distributor plus any
large electronics marketplace.

---

## Basket 1 — Industrial distributor (order first; longest lead time)

| # | Item | Qty | Requirements / search terms |
|---|------|-----|------------------------------|
| 1.1 | Polarized retroreflective photoelectric sensor | 3 | Range ≥ 5 m with reflector, response ≤ 1 ms, NPN NO, IP67, 12–24 V DC. E.g. Autonics BX series `-P` variants, Omron E3Z-R polarized, or equivalent. If polarized at the needed range is unavailable, through-beam (e.g. BEN10M-TFR) is acceptable for the prototype. |
| 1.2 | Prism reflector (manufacturer's, matched) | 3 + 1 large spare | Rated range is specified with the matched prism reflector. Reflective tape halves range — do not substitute. |

**Validation-first option:** additionally order 2–3 low-cost industrial-clone
polarized sensors (marketplace query: `polarized retroreflective
photoelectric sensor NPN`, listed response ≤ 1 ms) and run them through the
same jitter rig (decision D15). If a clone passes, the open BOM gets several
times cheaper. Do not build the whole prototype on unvalidated clones.

---

## Basket 2 — Electronics (marketplace order, ~2–3 week lead)

### Compute & bus

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.1 | ESP32-S3 DevKit | 4 | `ESP32-S3 DevKitC N16R8` — start node, finish node, tree module, spare. Native USB, hardware capture peripherals. |
| 2.2 | RS-485 transceiver module | 6 | Prefer `MAX13487` modules (auto-direction, true failsafe). Fallback: `MAX485 module TTL to RS485` + direction control in firmware. |
| 2.3 | Optocoupler input modules | 3 | `PC817 optocoupler module 4 channel` — beam inputs. |
| 2.4 | Logic analyzer | 1 | `logic analyzer 8ch 24MHz` — required for jitter measurement (D15) and tree delay calibration. |

### Power

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.5 | DC-DC buck 12→5 V | 4 | `Mini560 DC-DC step down 5V` or MP1584-based. Not linear regulators. |
| 2.6 | 12 V LiFePO4 pack, 6–12 Ah, sealed w/ BMS | 2 | Often faster to source locally (fish-finder / trolling-motor batteries are this exact form factor). Lithium ships poorly by air. A 12 V 2 A bench PSU unblocks all bench work meanwhile. |
| 2.7 | LiFePO4 charger 14.6 V | 1 | Chemistry-specific; a lead-acid charger will not do. |
| 2.8 | XT60 connector pairs | 10 | `XT60 connector pair` |
| 2.9 | Inline blade fuse holders + 3 A fuses | 5 | `blade fuse holder inline` |
| 2.10 | Reverse-polarity P-MOSFETs | 5 | `IRF4905` or logic-level equivalent |
| 2.11 | TVS diodes | 5+ | `SMBJ18A` |
| 2.12 | Power toggle switches, 5 A | 3 | overnight-off switch in the battery lead |

### Tree prototype & misc

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.13 | WS2812B LED rings | 6 | `WS2812B ring 12 LED` — bench-scale tree; daylight-grade LEDs are a later, separate purchase after logic works. |
| 2.14 | Perfboard (solderable) | 5 | v0 nodes live on soldered perfboard — not breadboards/jumper wires. |
| 2.15 | DIP switches, 6-pos, 2.54 mm | 4+ | node addressing + termination/service jumpers |
| 2.16 | Headers, sockets for DevKits | — | DevKit must be socketed, not soldered |
| 2.17 | Assortment kits | 1 each | `resistor kit 1/4W` (covers 120 Ω termination, 560 Ω–1 k failsafe bias, optocoupler and divider values), `capacitor kit`, `LED kit`, `heat shrink kit`, `Dupont connector kit` — cheaper and more future-proof than per-value purchases. |
| 2.18 | Tactile buttons | 10 | bench start-pulse trigger, alignment mode |

Marketplace practice: order everything the same day from few high-rating
sellers; buy 2× of anything under a dollar — a burned single buck converter
on a Friday costs a week of waiting.

---

## Basket 3 — Local hardware / construction store (buy any time)

| # | Item | Qty | Notes |
|---|------|-----|-------|
| 3.1 | Trunk cable | full track length + margin (~450–500 m) | Outdoor shielded FTP cat5e **or** field telephone wire (P-274 class). Buy the full length now — bus and pulse tests are only valid at real distance (D15, open question #3). |
| 3.2 | Power lead wire 2×0.75 mm² | 10 m | battery-to-node leads |
| 3.3 | Enclosure boxes, IP65–67 | 2 | ~120×160×90. v0 doesn't need real sealing, but packaging into the target box early informs the carrier-board layout. |
| 3.4 | Cable glands PG9/PG11 + blanking plugs | 10 + 5 | nylon with locknuts, not the cheapest grade |
| 3.5 | Screw terminal blocks, zip ties, heat shrink | — | |
| 3.6 | Post / mast materials | — | sensor stands and tree mast; heavy bases (batteries double as ballast) |

---

## Explicitly NOT in the v0 purchase

Deferred until bench validation passes (see decision D14/D15):

- M12 connectors (production node feature; v0 lives on terminal blocks)
- Gore-type vent membranes, conformal coating (production sealing)
- High-power daylight LEDs for the tree (after sequence logic works)
- Carrier PCB fabrication run (after the perfboard schema is proven)
- Batch sensor order (after the jitter rig picks the model)

## Tools checklist

Soldering iron/station with temperature control, multimeter, wire stripper,
crimper, laser distance meter (beam positions must be measured, not paced —
a 5 cm error in the trap base is a 0.25 % speed error), and the logic
analyzer from 2.4.