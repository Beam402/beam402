# Beam402 — Decision Log

Short records of significant engineering decisions and their reasoning, in
ADR (architecture decision record) style. Status values: **accepted**,
**revisit** (accepted for v1, open to change with new evidence).

Format: context → decision → why → what would change it.

---

## D01 — Wired timing link, not wireless

**Status:** accepted (v1) · **Scope:** start pulse + data bus

Commercial wireless drag timing systems exist and work (RaceAmerica wireless
packages, Trackmate wireless, and other portable systems) — wireless is *not*
impossible. It requires either a deterministic RF link with fixed, calibrated
latency (proprietary protocol, not general-purpose WiFi/BT) or a shared clock
at both ends (e.g. GPS-PPS). General-purpose radio links have delivery jitter
of multiple milliseconds (retries, contention, interference), which lands
directly in ET.

**Decision:** copper for anything carrying time. Rationale:

1. Wire delivers guaranteed microsecond-class propagation for the cost of a
   cable drum and RS-485 transceivers; a trustworthy RF link is the single
   hardest subsystem to engineer and validate.
2. An open project earns trust through verifiability. "The pulse travels over
   copper — here's the scope trace" can be checked by anyone; "our RF protocol
   compensates jitter, trust us" cannot.
3. Event sites are the worst RF environment available (hundreds of phones,
   hotspots, radios) and it is not under our control. The cable is.

Non-timing data (spectator scoreboard, arbitration video) may use radio
freely — the rule is *wire where time lives*, not *wire everywhere*.

**Would change it:** a future, separately validated RF timing link (fixed-
latency protocol or GPS-PPS sync) as a v2+ project. Also acceptable sooner:
ESP-NOW as a degraded *data* backup if the trunk is cut mid-event (results
delivery only; timing pauses for the 5-minute field splice).

---

## D02 — Polarized retroreflective sensors, not through-beam

**Status:** accepted

Initial assumption was that through-beam (separate emitter/receiver) is "the
professional choice." Research corrected this: reference systems (Compulink,
as used by NHRA) run photocells against a reflector block mounted in the
center of the track in a foam housing. Retroreflective is the industry
architecture, not the budget option.

**Decision:** polarized retroreflective industrial sensors. One powered
device per beam, on one side of the lane; a passive prism reflector on the
other. Polarization rejects false triggers from glossy bodywork. Benefits:
half the powered hardware, no power across the track, forgiving alignment.

Requirements are in `architecture.md` §2. Hobby-grade IR modules are excluded
(10–50 ms floating response, blinded by sunlight).

**Amended 2026-07-28 — through-beam supersedes retroreflective for v1.**

Datasheet review before the first order found that polarized retroreflective
does not cross a lane within the industrial-automation catalogue: Autonics
`BJ3M-PDT` is rated 3 m with the bundled MS-2A reflector (4 m / 5 m with the
optional MS-2S / MS-3S), Omron `E3Z-R61` reaches 4 m. Through-beam reaches
15 m in the same family and price class.

Through-beam also **replaces** the reason polarization was chosen: with no
reflector there is no return path, so glossy bodywork cannot complete the beam
at all. Polarization is a mitigation retroreflective needs; through-beam does
not have the vulnerability.

Three arguments settled it even after the home lane turned out to be ~3 m wide
— a span where retro is arguably viable:

- **Burnout smoke is paid for twice by retro.** The light crosses the obscured
  air to the reflector and back, doubling attenuation at the one station that
  sits where cars burn out.
- **Excess gain.** At a 3.2 m span, retro with MS-3S has ~2.4× margin;
  `BJ15M-TDT` has ~22×. Nearly an order of magnitude more headroom against
  smoke, dust and rain.
- Reflector fouling at tire height is a failure mode through-beam does not
  have — anticipated by this decision's original "would change it" clause.

**Cost of the reversal:** the far side of the track is no longer passive. Each
emitter needs 12–24 V (2 wires, ≤ 20 mA), so far-side posts get their own
battery — see D10 and architecture §7. Adjacent emitters share one post and
one battery. Mutual interference prevention, which would have solved the
start-station beam cluster, exists in the BJ series **except for the
through-beam type** — see D18.

**Would change it (updated):** purpose-built sports-timing photocells do
reflection at 15 m (Alge RLS3C) and 30 m+ (Microgate Polifemo), so the range
ceiling above is a property of the industrial-automation category, not of
retroreflective sensing. If the project ever accepts that price class, retro
returns with a passive far side. Narrow venues (span ≤ 3.5 m) may also
document retro as a local variant — but the BOM targets the general case,
because a lane of 4–5 m is typical and the system must be reproducible by
clubs that have one.

---

## D03 — LED sensors at the start, laser optional at finish/trap only

**Status:** accepted

Laser photoelectric sensors (e.g. laser retroreflective) offer a millimeter
spot instead of a multi-centimeter one, sharpening the trigger edge — worth
~1 ms of spread at 100+ km/h crossing speeds. At staging speeds the car
creeps, so spot size is irrelevant.

**Decision:** standard polarized LED sensors everywhere in v1; laser
retroreflective considered as a finish/trap upgrade only. Reference systems
run infrared photocells, not lasers — LED is the industry standard, not the
poor version. Laser downsides: 2–4× price, tighter alignment sensitivity on
vibrating posts, thin beams are easier to break with dust/burnout smoke.

**Amended 2026-07-28 — the mechanism behind the "~1 ms of spread" figure.**
The beam spot has finite width, so the received light does not fall off a
cliff as the tire arrives; it descends a ramp whose duration is spot size
divided by crossing speed. A centimetre-scale spot at 27.8 m/s (100 km/h) is
a ramp of roughly 1 ms, and the sensor triggers wherever its threshold cuts
that ramp. A repeatable cut is a constant offset and calibrates out; a
wandering one is jitter and does not.

This sharpens the trade-off rather than changing the decision. The same wide
beam that gives `BJ15M-TDT` its generous ±80 cm of lateral alignment
tolerance (§2) is also a blunt ruler — **forgiving alignment and a sharp
trigger edge are opposing requirements.** v1 buys alignment tolerance
knowingly. If bench-measured spread at trap speed is what forces the issue,
laser at the finish/trap is the documented escape, not firmware.

---

## D04 — No clock synchronization; broadcast start pulse + local counters

**Status:** accepted

Synchronizing clocks across nodes 400 m apart is the classic hard problem of
distributed timing.

**Decision:** don't synchronize. The start node broadcasts a hardware pulse on
a dedicated differential pair; every node starts a local counter on that
pulse and stops on its own beam. Every split is measured by one clock.
Propagation delay over 450 m of copper (~2.3 µs) is three orders of magnitude
below the 1 ms target and is ignored. The pulse is a fixed 5 ms width; nodes
validate the width to reject ignition-noise spikes.

---

## D05 — RS-485 / Modbus RTU bus, not a custom protocol, not CAN, not Ethernet

**Status:** accepted

**Decision:** half-duplex RS-485 multi-drop with Modbus RTU master–slave
polling. Rationale: rated to 1200 m (we use ~450 m at ≤19.2 kbps — huge
margin); differential signalling rejects common-mode noise from ignition
systems by design; collisions are impossible under strict polling; forty
years of industrial deployment next to welders and VFDs; mature libraries on
both MCU and PC ends. Less of our own protocol code = fewer of our own bugs.

Accuracy is unaffected by polling latency: events are timestamped at capture;
the bus only moves the resulting numbers.

**Would change it:** nothing foreseeable at this scale. Ethernet/fiber belong
to a permanent-installation variant.

---

## D06 — Copper trunk, not fiber optics

**Status:** accepted

Fiber's benefits (EMI immunity, no ground potential issues) are already
covered cheaply on copper: differential RS-485, optoisolated inputs, shield
grounded at one end. Fiber's field costs are decisive: cannot be spliced
roadside without a fusion splicer, connectors fail from dust at the ferrule,
media converters per node. A run-over copper cable is stripped, twisted and
taped in 5 minutes and the event continues.

**Would change it:** permanent buried installation, or a high-voltage line
running the length of the track.

---

## D07 — Universal timing node; roles live in configuration, not hardware

**Status:** accepted

**Decision:** one PCB, one firmware, for every track position. 4 optoisolated
inputs per node (start uses 3, finish 2, intervals 1 + spares). "Pairing" of
co-located functions is achieved by multiple inputs on one node, not by any
node-to-node protocol. A spare node can replace any position.

The Christmas tree is deliberately **not** a universal node — many LED
outputs and sequence logic would pollute the common board. It is a separate
module on the same bus, in its own enclosure. Same for the operator console
if one is ever built.

---

## D08 — DIP-switch bus address, not MAC-derived addressing

**Status:** accepted

The MCU's factory MAC is unique and could serve as a bus address. Rejected:
MAC binds identity to the silicon, while the system needs identity bound to
the *track position*. Field swap with DIP = copy switch positions from the
dead node, plug in, done — no laptop, no discovery, no config edits between
rounds. With MAC, every hardware swap becomes a configuration change.

DIP is readable on an unpowered board; addresses are static and known in
advance (dumb, predictable polling). The MAC is still reported over the bus
as a serial number for inventory and per-board fault history — "who you are
by passport" vs "where you work" are both useful, and must not be conflated.
This mirrors industrial practice (Modbus, DMX): field-replaceable devices get
manually set addresses.

The meaning of "node N, input M" lives in one mapping file on the race
control PC — the single source of truth. No role configuration is stored in
node flash.

---

## D09 — Pass-through trunk independent of node electronics

**Status:** accepted

The trunk enters and exits each node box, but the pass-through is plain
copper traces on the carrier board, independent of the node's power, CPU, or
socketed MCU module. Nodes attach to the bus as taps (multi-drop), not as
repeaters.

Why: a dead, unpowered, or even gutted node must never break the bus; the
start pulse must reach all nodes simultaneously (repeating would add
per-node jitter); firmware stays simple. Termination (120 Ω) by jumper,
enabled only on the two physical end nodes.

---

## D10 — Per-node LiFePO4 batteries, not central trunk power

**Status:** accepted (portable deployments)

12 V over 400+ m of affordable cable drops too much voltage at our currents;
central power done right means 48 V PoE-style distribution with isolated
48→12 bucks per node. That works, but for a portable system it adds ~30+ kg
of copper per deployment and creates a single point of failure: one damaged
power cable downs half the track. Batteries localize failures — a cut cable
loses data, not nodes.

**Decision:** sealed IP67 LiFePO4 modules (integrated BMS) per node, 10 Ah
class for multi-day events, XT60 + inline fuse + physical off switch.
LiFePO4 over AGM/lead: 3× lighter, deep-discharge protected by BMS, ~full
usable capacity. The carrier board keeps an ORing input for optional 48 V
trunk power, so a future permanent installation needs no redesign.

Battery voltage telemetry over the bus is mandatory, not optional — the
operator must see all node batteries on one panel.

**Amended 2026-07-28 — far-side emitter posts.** Through-beam (D02) puts a
powered device on the far side of the track. Consumption is trivial: the
`BJ15M-TDT` emitter draws ≤ 20 mA, so a 3-day × 8 h event needs **0.5 Ah**.
Capacity is therefore not the constraint — voltage and standardisation are.
A single LiFePO4 cell is 3.2 V and the emitter needs 12 V, so a 4S pack is the
floor; a boost converter from one large cell also works arithmetically but
adds a second battery type, a second charger and a converter inside a sealed
box nobody will open mid-round.

**Decision:** far-side posts use the *same* 12 V LiFePO4 module type as the
nodes, in the smallest capacity offered. One battery type and one charger in
the whole field kit, any pack fits any position (D11's logic), and the mass
serves as post ballast, which the far post needs anyway.

Adjacent emitters share one post and one battery: pre-stage, stage and guard
sit within 518 mm and mount together. A full single-lane build has **five**
far-side posts — start, 60 ft, 1/8, finish, trap. Each gets a battery, a
switch and an inline fuse; no MCU, no bus, no configuration.

Worth copying from commercial practice: Drag It Anywhere ships solar panels
with their battery-powered beam units. At 20 mA a small panel removes battery
swapping from the field routine entirely.

---

## D11 — Identical enclosures for all nodes

**Status:** accepted

One box type, drilled from one template for the maximum I/O configuration;
unused entries get IP-rated blanking plugs. Position identity = sticker +
DIP + mapping file. Rationale: a spare must be a spare for *any* position;
a second box type reintroduces the zoo the universal node eliminated.
Exceptions: tree module and operator console (single-instance devices,
interchangeability not required).

Connector policy (hybrid): trunk on panel-mount M12 (always mated, holds the
box); sensors and power as permanently-glanded pigtails ending in M12/XT60
(gland sealing is factory-permanent; a damaged connector is re-terminated on
the cable without opening the box). Sensor and power connectors are
mechanically incompatible by design.

---

## D12 — Arbitration camera, not line-scan photo finish

**Status:** accepted

A finish-line camera (action cam, 240 fps) aimed along the finish line is an
*evidence* source, not a timing source. Sync is optical: the finish node
drives a marker LED in frame on beam break; disputes are resolved
frame-by-frame against it. The camera never integrates with the bus — the
most reliable integration is none.

Line-scan photo finish (athletics-style, kHz line rate) was evaluated and
rejected: in athletics it is the *primary* timer because a chest can't break
a beam repeatably; here beams are primary (as in reference drag systems) and
already exceed the needed precision. Building a line-scan camera would be
months of work to duplicate an existing measurement.

**Would change it:** if bench-measured sensor jitter (open question #1) turns
out unacceptable — though the fix would be better sensors, not a second
measurement system.

---

## D13 — Timestamping in hardware capture, radios disabled

**Status:** accepted, *bench validation pending*

GPIO interrupts under a general-purpose framework can jitter by tens of
microseconds or worse under load. Beam edges are captured by hardware
(RMT/MCPWM capture on ESP32-class parts) or highest-priority interrupts;
WiFi/BT stacks are disabled in firmware (the radio is unused by design —
see D01). This is open question #3 in `architecture.md` §11 and gates the MCU
choice.

**Amended 2026-07-28 — the input stage is part of the timing path.**
Capturing in hardware is pointless if the signal reaching the capture pin has
already been smeared. Two corrections to the prototype BOM:

- **PC817 is the wrong optocoupler here.** Its edges are slow and asymmetric,
  and its CTR degrades with temperature and age — so the propagation delay you
  calibrate in spring is not the one you have in a hot box in August. Thermal
  drift is the systematic-error enemy across this whole path (see also D19).
  Use a fast digital optocoupler instead.
- **6N137 does not fit a 3.3 V MCU.** Its output side wants Vcc 4.5–5.5 V
  while ESP32 GPIOs are 3.3 V. Use a 3.3 V-capable part of the same speed
  class — `ACPL-M61L` / `ACPL-071L` — which keeps the LED input, so the sensor
  still drives it through a single series resistor.

A slow edge into a plain GPIO also risks **double capture** on noise; the
input needs hysteresis (Schmitt) rather than a bare pin. That is a
correctness problem, not an accuracy one.

Crystal tolerance is a third term and the cheapest to remove. The ±20–40 ppm
that costs ~0.4 ms over a 10 s run (§3) is *per-silicon systematic*, not
noise: measure each board once against the logic analyser already in the BOM
and store the correction **on the race control PC, keyed by MAC** — "passport,
not job", exactly D08's distinction. A TCXO solves it in hardware for a couple
of dollars if calibration bookkeeping proves annoying.

---

## D14 — Prototype on socketed dev modules; production PCBs fab-assembled

**Status:** accepted

v0 nodes: ESP32-S3 DevKit + transceiver/optocoupler modules on soldered
perfboard — enough for bench validation and the first parking-lot demo, not
for a season. v1: a simple 2-layer carrier board (KiCad) with the DevKit
socketed as a field-replaceable consumable; fab-assembled SMD (consistent
machine soldering across the batch beats hand-soldered variability —
uniformity is reliability for a batch of identical safety-relevant boxes).
Enclosure drilling outsourced to a shop with the template; final assembly
(boards into boxes, glands, pigtails, labels) done in-house — that's where
field knowledge is built and where assembly quality must stay accountable.

Production files (gerbers, BOM, position files) are committed to the repo —
the same artifacts a fab needs are the ones that make the project genuinely
reproducible.

---

## D15 — Sensor validation gates the project

**Status:** accepted (process decision)

No batch purchases, no PCB orders, no public timelines before the bench
answers: does the full path (beam → sensor → optocoupler → capture) hold
< 1 ms total jitter? Test rig: slotted disk on a motor at stable RPM breaking
the beam hundreds of times; measure timestamp spread with a logic analyzer.
The same rig compares industrial-brand sensors against low-cost industrial
clones — if a clone passes, the open BOM gets dramatically cheaper, which
matters for a project whose point is reproducibility.

**Amended 2026-07-28 — five corrections to the method.**

1. **A reference detector is mandatory, not optional.** "Stable RPM" is doing
   load-bearing work it cannot carry: a motor's speed wanders, and with a
   single sensor that wander is indistinguishable from sensor jitter — you
   would be measuring the motor. Put a fast reference detector (photodiode +
   comparator, or a slotted opto) on the same disk and measure the
   **difference** between the two detectors on each pass. Common-mode speed
   drift cancels.
2. **Report two edges, not one number.** §2 starts ET when the tire *exits*
   the stage beam and stops it when the tire *breaks* the finish beam — two
   opposite transitions, through a sensor whose hysteresis makes the make and
   break thresholds deliberately unequal. Any asymmetry is a **systematic ET
   offset that cancels nowhere.** The rig must output make-delay, break-delay
   and their difference separately.
3. **Test both speed regimes.** A 100 mm-radius disk at 2650 rpm gives an edge
   speed of 27.8 m/s — 100 km/h, finish-line conditions; 27 rpm gives staging
   creep. One rig covers both on a motor voltage knob, and the trigger ramp
   (D03) is speed-dependent, so both matter.
4. **Add temperature.** Run at ~25 °C, then again at ~60 °C under a box with a
   heat gun, and watch the **shift in mean delay**, not only the spread. See
   D19.
5. **Detection is not in question and does not need measuring.** A 30-inch tire
   presents a ~573 mm chord at a 130 mm beam height, so the beam stays broken
   for 20.6 ms at 100 km/h and 6.9 ms at 300 km/h against a 1 ms response — a
   sevenfold margin at the worst case. The rig exists to measure *repeatability
   of the instant*, not whether the wheel is seen.

**Abort criteria — what result means "wrong sensor" rather than "more
firmware".** Sensor jitter above ~400 µs (over half the < 1 ms path budget on
its own), or a make/break offset above ~500 µs that also drifts with
temperature. Either sends the project to laser at the finish/trap (D03) or to
the sports-timing category (D02) — not to a software correction.

**Ground truth.** No vendor in any category publishes jitter or repeatability
— not Autonics, not Alge, not Microgate; all publish range, output and at best
a worst-case response time. The number this project needs is in nobody's
datasheet, which is the whole justification for this decision. It also means
the bench cannot validate a sensor against itself: borrow or rent **one**
timing-grade photocell (Alge, Microgate) as the reference standard. One unit,
not eight.

---

## D16 — The start pulse is generated in hardware, not by firmware

**Status:** accepted · **Scope:** start node

§3 requires beam timestamps to be captured in hardware but says nothing about
how the start pulse itself is *produced*. If the start node detects the
stage-beam edge and firmware then drives the 5 ms pulse, software latency and
its jitter sit between the physical event and the pulse — and enter every ET
on every node simultaneously. Hardware capture on the receiving side can
neither see nor compensate for it.

**Decision:** derive the pulse in hardware from the input — a monostable
(74HC123 class) triggered from the optocoupler output. Firmware observes the
pulse; it does not create it. If firmware generation is ever retained instead,
the start node must timestamp the *emission* of the pulse with the same
counter it uses for the beam edge and publish that delta on the bus for the
master to subtract.

**Why:** the pulse is the project's zero. A jittering zero corrupts every
downstream number identically and invisibly — the one error that no split
comparison can reveal, because all splits share it.

**Implementation trap, same subject:** width validation must not postpone the
counter. Nodes start counting on the leading edge and retroactively invalidate
the run if the width is wrong. The naive reading — wait for the full pulse,
then start — adds exactly 5 ms to every measurement.

**Would change it:** an MCU peripheral that generates the pulse directly from
an input-capture unit with no CPU in the path satisfies the same requirement.

---

## D17 — PNP sensor output, configured Light ON, for fail-safe wiring

**Status:** accepted · **Supersedes:** the "NPN, NO" line in architecture §2

Output polarity is baked into the part number (`BJ15M-TDT-C` vs `-C-P`) and
cannot be changed once a batch is bought, so it is a decision, not a detail.

**Decision:** PNP, with the receiver's mode switch set to **Light ON**, so
that "beam intact" = output active.

**Why:** trace the wiring faults under that configuration. A **cut cable**
reads as "beam broken" on either polarity — loud and instantly visible. A
signal conductor **shorted to ground** reads as "beam intact" on NPN, so the
system silently stops seeing cars; on PNP it reads as "beam broken", loud
again. For a project whose entire claim is a trustworthy number, a fault that
quietly loses events is the worst failure mode available. The optocoupler
input is indifferent to polarity, so this costs nothing.

**Consequence for firmware:** a beam break is a falling edge at the node, and
the tire *leaving* the stage beam is a rising edge — which is the edge that
starts ET under §2.

**Would change it:** a sensor family where a needed function exists only in
NPN.

---

## D18 — Collimated, shaded receiver hood

**Status:** revisit — accepted in principle, dimensions pending bench

Two facts collide at the start station. The BJ series' mutual interference
prevention **excludes the through-beam type** (stated twice in the datasheet),
and the reason is structural: a through-beam emitter is a two-wire device with
no link to its receiver, so there is nothing to negotiate a modulation channel
with. Meanwhile the receiver's acceptance is wide — the parallel-shifting
characteristic reaches ±200 cm at 15 m, about 7.5° of half-angle. An adjacent
emitter 178 mm away at a 3.2 m span sits ~3.2° off axis, comfortably inside
that cone.

**Crosstalk at the start station is therefore documented, not hypothetical.**
The failure mode is a *missed* event rather than a false one: the receiver
keeps seeing a neighbour's light while its own beam is blocked. The same wide
cone is what makes the 11,000 lx illumination limit reachable — the sun only
has to be near the beam axis.

**Decision:** receivers get a recessed, collimated, shaded hood. Rejection
scales as depth ≈ aperture ÷ tan θ: ~3.2° needs roughly 18 apertures of depth
(≈18 cm at a 10 mm aperture); the 1.7° case at a 6 m span needs ~35 cm.
**That depth lives in the setback outside the lane** — posts already stand back
from the lane edge, so nothing intrudes on the racing surface. Attenuation
below threshold is enough, so shallower hoods help proportionally.

Construction rules that are not self-evident:

- Matte black inside, with two or three ring baffles. A bare tube channels
  off-axis light along its walls and returns part of what the depth bought.
- **White outside.** A black hood in the sun becomes a heater wrapped around
  the sensor and makes D19 worse than no hood at all.
- PETG or ASA, not PLA — PLA softens near 60 °C and will sag.
- Front window as a replaceable consumable: flat glass in an O-ring, tilted
  slightly to shed water and throw low-sun glint off the axis. Glass rather
  than polycarbonate because the enemy is haze from wiping grit, and haze at
  the aperture scatters sunlight into the very cone the hood exists to narrow.
  Two air-glass surfaces cost ~8% — irrelevant against ~22× excess gain.
- Vent, do not seal. A sealed cavity fogs on the inside where no cloth
  reaches: breather plus a drain hole, which doubles as the convection inlet.
- **Collimate the receiver hard, the emitter lightly.** Discrimination happens
  at the receiver; squeezing the emitter shrinks the far-side spot and makes
  alignment vicious for no gain. The wanted target is angularly tiny anyway —
  the emitter subtends 0.36° at 3.2 m against a 0.53° solar disc — so
  narrowing acceptance to 1–2° still admits the beam with room to spare.

**Rejected: tilting the pairs off perpendicular.** It looks like free angular
separation and is not. A non-perpendicular beam converts the car's lateral
position in the lane into a longitudinal trigger offset: 3° across a 5 m lane
is 262 mm of spread, about 9 ms at trap speed — an order of magnitude beyond
the entire error budget. **Perpendicularity is a timing requirement, not a
workmanship preference.**

Cheaper measures to exhaust before committing to the depth above, which is an
estimate and not a measurement:

- The receiver's built-in sensitivity adjuster. Crosstalk exists only because
  a 15 m sensor runs at 3.2 m with ~22× excess gain; the VR attacks that
  surplus directly and costs nothing. The same surplus is the margin against
  burnout smoke, so the bench sets the balance.
- A different vendor for pre-stage. Pre-stage is an indicator, not a timing
  source, so it can be a clone — and another manufacturer almost certainly
  modulates at a different frequency, which may remove the interference for
  free.
- A shorter-range model at the start station specifically, where less excess
  gain is a feature rather than a loss.

**Would change it:** a through-beam family offering paired A/B modulation
channels. Worth asking distributors for — though the industry limit is two
channels (Omron: "up to 2 sets" mounted closely), and a three-beam start
cluster would still leave the pre-stage/guard pair 518 mm apart to solve
mechanically.

---

## D19 — Sensor thermal drift is a measured quantity, not an assumption

**Status:** accepted, bench validation pending

The BJ series operating range is −25…55 °C (storage −40…70 °C, so hot
transport is not a concern). But the spec is *ambient air*, and what heats is
the body: a small dark plastic housing under ~1000 W/m² of sun at 35 °C air
can plausibly reach 55–70 °C. The limit is therefore reachable on an ordinary
summer race day while air temperature stays comfortably inside spec.

**The failure mode is drift, not death.** PBT and PMMA tolerate far more than
55 °C; what moves is the threshold, the response delay and the emitter's
output. The symptom is correct ETs in the morning and systematically shifted
ETs by mid-afternoon, with nothing visibly broken — the same class of problem
as optocoupler CTR drift (D13), and the worst kind for this project.

**Decision:**

- **Shade the receiver with an air gap** — a light-coloured plate above and
  behind, standing off so convection carries heat away instead of conducting
  it in (the Stevenson-screen principle). In practice this is the same part as
  the D18 hood, which is why D18 mandates a white exterior.
- **Choose the receiver side of the track for sun, not convenience.** The beam
  is horizontal at 13–20 cm, so sun is only a threat when it sits near the
  horizon *and* aligns in azimuth with the beam axis: sunrise and sunset on a
  north–south track, never on an east–west one. For evening racing, put
  receivers on the west side looking east. This costs nothing and is decided
  at layout. Only the receiver matters — the emitter is a bare LED and does
  not care about sunlight.
- **Report temperature at the sensor bracket over the bus**, alongside battery
  voltage (§6, §9). Without that number you cannot tell "hot day" from "sensor
  lying", and you cannot apply a correction even after measuring one.

The sensitivity adjuster is *not* expected to help against sunlight: the
11,000 lx limit is most likely front-end saturation by the DC component, and
trimming gain downstream does not undo saturation. Geometry physically
excludes the light and is the reliable lever. Both are bench-measurable.

**Would change it:** a sensor family rated to 70 °C, or measured drift across
25–60 °C small enough to ignore.