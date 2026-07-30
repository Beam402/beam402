# Beam402 — Prototype BOM (v0 bench + parking-lot demo)

Bill of materials for the **v0 prototype**: bench validation rig, plus enough
beams for a parking-lot demo (staging → tree → start pulse → ET on a laptop).
This is *not* the production BOM — quantities and grades are chosen to answer
the open questions in `docs/architecture.md` §11 with minimal spend.

Approximate total: **$500–600**, most of it sensors — an honest reflection of
where the system's accuracy lives.

> **Revised 2026-07-28.** The first version of this BOM named parts that would
> have missed the project's accuracy target by 20× and could not survive
> outdoors. See [What changed](#what-changed-and-why) before ordering, and
> `docs/decisions.md` D02, D13, D15, D17, D18, D19 for the reasoning.

Sourcing notes use part numbers and generic search terms rather than store
links: the project should be buildable from any regional industrial
distributor plus any large electronics marketplace. Prices below are Western
distributor references for comparison only — verify regionally, and note that
Autonics list pricing is quote-based at the manufacturer.

---

## What changed and why

Five findings from datasheet review before the first order:

| Finding | Consequence |
|---|---|
| `-FR` suffix is a **relay** output with **20 ms** response; `-DT` is DC solid-state at 1 ms | the previously named `BEN10M-TFR` misses the 1 ms target by 20× |
| The **BEN family is IP50** | cannot go outdoors at all |
| Polarized retroreflective tops out at 3–5 m in this catalogue | through-beam instead — **D02** |
| BJ interference prevention **excludes through-beam** | the start beam cluster needs mechanics, not a feature — **D18** |
| PC817 is slow, asymmetric and drifts with heat and age | fast 3.3 V-capable optocoupler — **D13** |

Also: `-T` variants carry an on-board 0.1–5 s delay timer that silently
destroys timing if enabled. Order the non-`-T` part rather than managing it
with a pre-round checklist.

---

## Basket 1 — Industrial distributor (order first; longest lead time)

| # | Item | Qty | Part / requirements |
|---|------|-----|---------------------|
| 1.1 | Through-beam sensor set | 3 | `BJ15M-TDT-C-P` — 15 m, max 1 ms, PNP, 12–24 V, IP67, emitter ≤ 20 mA. **One part number supplies both emitter and receiver.** Reference price $117–129/set |
| 1.2 | M8 4-pin connector cable, 5 m | 6 | Two per set. Autonics `CID408-5` / `CLD408-5` exist but are ~$32 each — **buy generic M8 4-pin socket cables at $5–8 instead**, they are electrically identical. Take 5 m, not 2 m: the sensor lives on a post, the node in a box |

**Confirm on the quote, in writing:** that `BJ15M-TDT-C-P` ships as a complete
set. The manufacturer's ordering table has an internal emitter/receiver digit
(`1`/`2`) marked *"no need to enter when selecting a model"*, and distributor
pages contradict each other on this point. A quote around $60 means someone is
selling halves.

**Also request pricing on**, to compare before committing:

- `BJ10M-TDT-C-P` — 10 m. Less excess gain, so less crosstalk (D18), but also
  less margin against burnout smoke. Excess gain can be trimmed with the
  built-in VR and cannot be added, which is why 15 m is the default choice.
- `BJ7M-TDT` — 7 m, ideal on gain grounds but **has no `-C` variant**, so IP65
  only. Disqualified for outdoor use.
- `BX15M-TDT-P` — the terminal-block family. IP66 and physically larger.
- `BJ3M-PDT-C-P` + `MS-3S` reflector — polarized retro, $101–111 + ~$15. Not
  cheaper than through-beam once cables are counted; kept on the list because
  it is the documented narrow-venue variant (D02) and worth one bench
  comparison.

**Ask every distributor one question:** is there a through-beam family with
**paired A/B modulation frequencies**? That is the textbook fix for adjacent
beams, it is not in the Autonics catalogue, and finding it would delete most of
D18.

---

## Basket 2 — Electronics (marketplace order, ~2–3 week lead)

### Sensors for comparison (D15)

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.1 | Through-beam clone sets | 2 | `through beam photoelectric sensor 20m NPN PNP IP67`, listed response ≤ 1 ms. Order the same day — longest shipping |
| 2.2 | Drag-specific beam + detector | 1 pair | Drag It Anywhere IR or laser units, $18.95–52.95 **each** (beam and detector sold separately). Same tier as the clones but purpose-built for drag geometry — edge-to-centre, 35 ft, brackets included, battery powered, solar on the laser version. **Ask what the output interface is** before buying; if it is proprietary to their timer, integration will cost more than it saves |

One of the three cheap sets goes on **pre-stage**, which is an indicator and
not a timing source — precision there is wasted money. Bonus: a different
manufacturer almost certainly modulates at a different frequency, which may
remove the pre-stage/stage interference for free (D18).

**Reference standard.** Borrow or rent **one** timing-grade photocell — Alge
`RLS3C`, Microgate `Polifemo` — as bench ground truth. No vendor in any
category publishes jitter, so the rig cannot validate a sensor against itself.
One unit, not eight; buying a set of these would break the project's premise.

### Bench rig

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.3 | Logic analyzer | 1 | `logic analyzer 8ch 24MHz` — required for jitter measurement (D15), crystal calibration (D13) and tree delay calibration |
| 2.4 | Reference detector | 2 | `BPW34 photodiode` + `LM393 comparator`, or a slotted opto-interrupter. **Not optional** — without differencing against a reference on the same disk, the rig measures the motor's speed drift, not the sensor (D15) |
| 2.5 | Small DC motor + disk material | 1 | A 100 mm-radius slotted disk at 2650 rpm gives 27.8 m/s edge speed = 100 km/h; 27 rpm gives staging creep. With a reference detector, a plain brushed motor is fine — no stepper needed |
| 2.6 | Heat gun / hot air source | 1 | Thermal drift run at ~60 °C (D19). A hair dryer and a cardboard box will do |

### Compute & bus

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.7 | ESP32-S3 DevKit | 4 | `ESP32-S3 DevKitC N16R8` — start node, finish node, tree module, spare. Note GPIO26–37 are consumed by flash/PSRAM on `N16R8`; avoid them for capture inputs |
| 2.8 | RS-485 transceiver module | 6 | Prefer `MAX13487` modules (auto-direction, true failsafe). Fallback: `MAX485 module TTL to RS485` + direction control in firmware |
| 2.27 | **USB-RS485 adapter, isolated** | 2 | The missing half of the bus: the race control PC is the *only* master (`architecture.md` §1) and nothing else in this BOM puts it on the line — the parking-lot demo ends with "ET on a laptop". Buy **FTDI-based** (`FT232RL` / `FT2232H`), not CH340: FTDI's latency timer is settable to 1 ms, and that timer is exactly what breaks Modbus RTU's 3.5-character inter-frame silence (~1.8 ms at 19,200) in a way that reads as line noise (`software.md` §8 #5). **Galvanically isolated**: the trunk is 450 m, its shield is grounded at the race control end only (§5), node inputs are optoisolated — without isolation the master's transceiver sits directly on the laptop's USB ground. Take **two**: the second listens while the first talks, which is the only practical way to debug bus framing, and it tests §8 #5 with no ESP32 at all |
| 2.9 | Fast optocoupler, 3.3 V-capable | 6 | `ACPL-M61L` or `ACPL-071L`. **Not PC817** (slow, asymmetric, CTR drifts) and **not 6N137** (needs a 5 V output rail the ESP32 lacks) — see D13 |
| 2.10 | Monostable | 4 | `74HC123` — hardware generation of the 5 ms start pulse (D16), keeping firmware out of the timing path |
| 2.11 | Temperature sensors | 6 | `DS18B20 waterproof` — node interior and sensor bracket telemetry (D19) |

Item 2.27 sits out of sequence on purpose. These numbers are referenced from
`docs/bench-validation.md` and the software documents, so they are appended
rather than renumbered — the same rule the decision log uses for `D` IDs.

### Power

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.12 | DC-DC buck 12→5 V | 4 | `Mini560 DC-DC step down 5V` or MP1584-based. Not linear regulators |
| 2.13 | 12 V LiFePO4 pack, sealed w/ BMS | 4 | 2 for nodes (6–12 Ah), 2 **smallest available** for far-side emitter posts — those need 0.5 Ah for a 3-day event, so capacity is irrelevant and the choice is about one battery type and one charger across the kit (D10). Often faster to source locally; lithium ships poorly by air. A 12 V 2 A bench PSU unblocks all bench work meanwhile |
| 2.14 | LiFePO4 charger 14.6 V | 1 | Chemistry-specific; a lead-acid charger will not do |
| 2.15 | Small solar panel, 12 V | 2 | Optional but cheap at 20 mA draw — removes emitter battery swapping from the field routine entirely |
| 2.16 | XT60 connector pairs | 10 | `XT60 connector pair` |
| 2.17 | Inline blade fuse holders + 3 A fuses | 7 | `blade fuse holder inline` — nodes and far-side posts |
| 2.18 | Reverse-polarity P-MOSFETs | 5 | `IRF4905` or logic-level equivalent |
| 2.19 | TVS diodes | 5+ | `SMBJ18A` |
| 2.20 | Power toggle switches, 5 A | 5 | Overnight-off switch in every battery lead, far-side posts included |

### Tree prototype & misc

| # | Item | Qty | Search terms / notes |
|---|------|-----|----------------------|
| 2.21 | WS2812B LED rings | 6 | `WS2812B ring 12 LED` — bench-scale tree; daylight-grade LEDs are a later, separate purchase after logic works |
| 2.22 | Perfboard (solderable) | 5 | v0 nodes live on soldered perfboard — not breadboards/jumper wires |
| 2.23 | DIP switches, 6-pos, 2.54 mm | 4+ | Node addressing + termination/service jumpers |
| 2.24 | Headers, sockets for DevKits | — | DevKit must be socketed, not soldered |
| 2.25 | Assortment kits | 1 each | `resistor kit 1/4W` (covers 120 Ω termination, 560 Ω–1 k failsafe bias, **820 Ω sensor input series resistor**, divider values), `capacitor kit`, `LED kit`, `heat shrink kit`, `Dupont connector kit` |
| 2.26 | Tactile buttons | 10 | Bench start-pulse trigger, alignment mode |

Marketplace practice: order everything the same day from few high-rating
sellers; buy 2× of anything under a dollar — a burned single buck converter on
a Friday costs a week of waiting.

---

## Basket 3 — Local hardware / construction store (buy any time)

| # | Item | Qty | Notes |
|---|------|-----|-------|
| 3.1 | Trunk cable | full track length + margin | Outdoor shielded FTP cat5e **or** field telephone wire (P-274 class). Buy the full length now — bus and pulse tests are only valid at real distance (§11 #5) |
| 3.2 | Power lead wire 2×0.75 mm² | 10 m | Battery-to-node and battery-to-emitter leads |
| 3.3 | Enclosure boxes, IP65–67 | 2 | ~120×160×90. v0 doesn't need real sealing, but packaging into the target box early informs the carrier-board layout |
| 3.4 | Cable glands PG9/PG11 + blanking plugs | 10 + 5 | Nylon with locknuts, not the cheapest grade |
| 3.5 | Screw terminal blocks, zip ties, heat shrink | — | |
| 3.6 | Post / mast materials | — | Near-side and **far-side** sensor stands plus tree mast; heavy bases (batteries double as ballast) |
| 3.7 | Hood stock — PETG or ASA filament, flat glass discs, O-rings | — | D18 hoods, if the bench says they are needed. **PLA will sag** (glass transition ~60 °C), and the hood must be **matte black inside, white outside** or it becomes a heater around the sensor |

---

## Explicitly NOT in the v0 purchase

Deferred until bench validation passes (D14/D15):

- M12 connectors (production node feature; v0 lives on terminal blocks)
- Gore-type vent membranes, conformal coating (production sealing)
- High-power daylight LEDs for the tree (after sequence logic works)
- Carrier PCB fabrication run (after the perfboard schema is proven)
- Batch sensor order (after the jitter rig picks the model)
- **Autonics-branded M8 cables** — ~$32 each against $5–8 generic, for a
  passive lead with a moulded connector
- **D18 hoods in quantity** — build one to test, not seven. The ~18 cm depth
  figure is geometry, not measurement

---

## Cost of the full build (reference, single lane)

Seven beams, for planning only — do not order this before the bench answers
§11 #1.

| Configuration | Sensors | Cables | Total |
|---|---|---|---|
| All brand, Autonics cables | 7 × $129 = $903 | 14 × $32 = $448 | **~$1350** |
| All brand, generic cables | 7 × $117 = $819 | 14 × $6 = $84 | **~$900** |
| Brand at start, clones elsewhere | 3 × $129 + 4 × ~$70 | ~$100 | **~$770** |
| Minimum system (3 beams) | 3 × $129 = $387 | 6 × $6 = $36 | **~$420** |

The cable row is the trap: branded M8 leads can reach half the cost of the
sensors they connect. Sensors are roughly half the cost of the whole system,
which is what makes clone validation (D15) the highest-leverage test in the
project — a passing clone changes the total by hundreds of dollars per venue.

---

## Tools checklist

Tools are not part of the per-build cost — they are bought once — but two of
them gate work that cannot proceed without them, and one is easy to overlook
because the original version of this document did not name it.

### Bench — measurement

| Tool | Why |
|---|---|
| **Oscilloscope, 2 channels** | **The gap most likely to be missed.** A logic analyzer only reports high or low; it cannot show ringing, reflections from bad termination, or the shape of ignition noise coupled into 400 m of cable — which is exactly what §11 #5 asks about. D01 also stakes the project's credibility on "the pulse travels over copper, here is the scope trace, check it yourself"; that promise needs an instrument that produces traces. Two channels minimum, to see a differential pair |
| Logic analyzer, 8ch 24 MHz | Item 2.3. 41 ns resolution against a 1 ms budget — ample. Free software: sigrok / PulseView |
| Bench PSU, 12 V, adjustable, **with current limit** | Current limiting is the point, not the voltage: it saves parts during bring-up. Also unblocks all bench work before the LiFePO4 packs arrive |
| Non-contact IR thermometer | D19 needs the temperature of the sensor *body*, not of the air. Cheap and exactly suited |
| Heat gun or hair dryer | Item 2.6 — the ~60 °C thermal drift run |
| Multimeter | |

**Data reduction is a tool too.** The rig produces thousands of edge
timestamps; the deliverable is a *distribution* — spread, and mean shift
between conditions. That cannot be read off a PulseView window by eye. Export
CSV and keep a short script that reports jitter and make/break offset per run,
so results from different days are comparable. Free, but it has to exist
before the first serious measurement.

### Field — layout and alignment

| Tool | Why |
|---|---|
| A way to set a **right angle** — tape measure by 3-4-5, or a laser square | D18 makes perpendicularity a *timing* requirement: a tilted beam converts the car's lateral position into a longitudinal offset. Without a method to set it, that decision is unenforceable |
| Laser distance meter | Beam positions are measured, not paced — 5 cm of error in the trap base is 0.25 % of speed (§2) |
| Short spirit / torpedo level | Beams are horizontal, one per post |
| Laser pointer on the sensor bracket | The emitter is 850 nm — **the beam is invisible**. Fine alignment goes by the receiver's green stability indicator, but nothing exists to aim the posts at each other roughly. A pointer on the mount solves it |

### Assembly

| Tool | Why |
|---|---|
| Temperature-controlled soldering station | |
| Flush cutters, tweezers, PCB vise or helping hands | v0 nodes live on soldered perfboard |
| **Ferrule crimper** | XT60 is soldered and M8 leads are moulded, so the crimper that actually gets used is the one for terminal-block ferrules |
| **Step (conical) drill bit** + deburring tool | Gland holes in plastic boxes; hole saws chip them |
| Wire strippers | |
| Access to **PETG or ASA** 3D printing — own or service | D18 hoods, if the bench says they are needed. **PLA will sag** near 60 °C, which is precisely the condition the hood exists in |

### Explicitly not needed

- **Tachometer** for the disk rig — the reference detector's signal gives the
  period directly on the logic analyzer.
- **Stepper motor and driver** instead of a brushed motor — differencing
  against the reference detector cancels speed drift, so a stable drive buys
  nothing (D15).
- **Function generator** — test pulses come from an ESP32 or a 555.
