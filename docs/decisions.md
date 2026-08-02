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

The executable form of this decision — rig construction, per-test procedures,
expected values and pass/fail criteria — is
[`bench-validation.md`](bench-validation.md).

**Amended 2026-07-28 — five corrections to the method.**

1. **A reference detector is mandatory, not optional.** "Stable RPM" is doing
   load-bearing work it cannot carry: a motor's speed wanders, and with a
   single sensor that wander is indistinguishable from sensor jitter — you
   would be measuring the motor. Put a fast reference detector (photodiode +
   comparator, or a slotted opto) on the same disk and measure the
   **difference** between the two detectors on each pass. Common-mode speed
   drift cancels.

   Two construction constraints make that cancellation actually work, and
   "on the same disk" does not imply either of them:

   - **Same angular position, different radius.** Cancellation is only as good
     as the two detectors' angular separation. At 2650 rpm, detectors 90°
     apart see a 5.7 ms interval, so 1 % of speed drift moves it 57 µs — a
     sixth of the abort threshold, injected by the rig itself. At 5° apart the
     same drift contributes 3 µs. Mount the reference at the same clock
     position as the beam, on a smaller radius.
   - **Radial slot edges, and one slot.** A non-radial edge crosses different
     radii at different angles, which reintroduces exactly the separation the
     previous point removes. And multiple slots make slot-to-slot machining
     variation appear as sensor jitter — use a single slot, or index every
     slot and analyse per-slot.
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

---

## D20 — The start pulse resets the capture timer; one lane per MCPWM group

**Status:** accepted, *bench validation pending* (T3) · **Scope:** node firmware

D04 says each node "starts a local counter on that pulse and stops on its own
beam." The obvious implementation captures the pulse and the beam as two
timestamps and subtracts them. With two lanes that becomes four captures per
downstream node — two pulses and two beams — and the hardware does not
accommodate it cleanly: an ESP32 MCPWM group has **one capture timer feeding
three capture channels**, and channels in different groups latch from different
timers. Those timers share the APB clock so they never drift apart, but they
started at different moments, so any cross-group subtraction carries an unknown
constant offset. That is D04's own cross-clock problem, reproduced inside a
single node.

**Decision:** read D04 literally. The pulse **resets the capture timer** through
the MCPWM GPIO sync input; it does not consume a capture channel. One lane per
group:

| Group | Capture timer synced by | Channel on | Register holds |
|---|---|---|---|
| MCPWM0 | lane 1 start pulse | lane 1 beam | lane 1 split, directly |
| MCPWM1 | lane 2 start pulse | lane 2 beam | lane 2 split, directly |

**Why:**

- Two of six channels used, so interval, trap and finish nodes keep headroom.
- Each lane's measurement closes inside one group, so the cross-group offset
  never enters a result and needs no calibration.
- The timer is zeroed at every launch and a run is under 20 s against a 53.7 s
  wrap, so **64-bit accumulation is not needed for run timing.** It remains
  necessary only for the raw edge log (§6), which can run on a separate,
  coarser clock — millisecond resolution is plenty for dispute evidence.
- Requires one pulse pair per lane, which §5's pair table already reserves.

**Cost, and where it moves.** The two capture timers are now deliberately reset
at different instants, so the finish node **cannot determine who crossed
first** — and crossing order, not ET, decides the race: a slower car that
launched earlier wins. That comparison is recovered at the start, where both
pulses originate. One start-area node captures both pulses on a single timer and
reports the launch difference, so

```
margin = (pulse₂ − pulse₁) + ET₂ − ET₁
```

with every term measured inside one clock, and D04 intact. This is arguably the
correct division of labour: "who left first and by how much" belongs to the
start; "how long each took" belongs to the finish.

**Single-lane builds** use only group 0, where none of the above is
load-bearing. Implement it this way regardless, so adding a second lane is a
mapping-file change rather than a firmware change — D07's principle applied to
firmware.

**Pending verification:** sync latency is hardware and should be deterministic,
but T3 must measure it rather than trusting the reference manual. If it proves
to jitter, the fallback is to capture the pulse on a channel and pay for the
cross-group offset calibration — a measurable constant, since the two timers
share a clock source and cannot drift relative to each other.

**Amended 2026-07-30 — the API exists; its latency is still unmeasured.**
ESP-IDF drives capture-timer sync from a GPIO source directly
(`mcpwm_capture_timer_set_phase_on_sync()` with a GPIO sync source), so the
mechanism above is not a guess about what the silicon might permit. That check
is also what decided the firmware language — see **D22**. What stays open is
precisely what this record already said: an API existing says nothing about its
jitter, and T3 must measure it.

Confirmed in the same check, and load-bearing for the channel count above: a
capture channel takes `pos_edge` and `neg_edge` **together**, reporting which
fired. So the two channels cover each beam's break *and* make, not one of them —
which the project needs twice over, since §2 starts ET on a make and stops it on
a break, and T2 measures the asymmetry between the two.

**Would change it:** an MCU whose capture channels all share a single timer,
which removes the constraint that produced this decision.

---

## D21 — Centre trunk; receivers back-to-back in the centre island

**Status:** accepted · **Scope:** cable routing, node and sensor placement ·
**Supersedes:** "run along one side of the track (the receiver side)" in §5

§5 originally ran the trunk along one side, on the receiver side. That works for
one lane and breaks for two. Each lane needs its own beam, because a single beam
spanning both cannot say which car broke it — and spanning both fails anyway,
since a car in lane 1 would break lane 2's beam. With a side trunk, lane 2's
receiver must either sit across the track, sending signal over the racing
surface, or sit in the centre, sending signal across lane 1. Both are excluded.

**Decision:** the trunk runs down the **centre island** for the full length.
Both lanes' receivers sit in the centre, back to back, each facing outward
across its own lane. Emitters sit on the outer edges. Nodes sit in the centre
beside the receivers they serve.

**Why:**

- Nothing crosses the racing surface — not signal, not power, not the bus.
- Both spans stay at their minimum, roughly half the track width. That is where
  excess gain is highest and where D18's collimation depth is shallowest.
- **Cross-lane interference is largely dissolved by geometry.** The two
  receivers face opposite directions, so their acceptance cones point away from
  each other and each has its back to the other lane's emitter. §11 #9 stops
  being a sensor-selection problem. A side trunk would have pointed both
  receivers the same way and made it worse.
- Node placement becomes compact: one node per position serves both lanes on
  short sensor leads (§12).

**Cost, and the part that needs care.** Equipment now sits in the middle of a
live drag strip. Reference practice already does this — D02 notes Compulink's
reflector block in a **foam housing** at the centre of the track — but a passive
foam-housed reflector is not the same object as a node carrying a battery.
Centre hardware must be low-profile and frangible.

This is also the one place where **D10's optional trunk power earns its keep**:
the trunk is already present, and moving mass out of the impact path is worth
more at the centre than battery independence is. Worth revisiting D10 for centre
nodes specifically, on evidence rather than now.

**Consequences elsewhere:**

- §5's pair 4 stops being spare — it carries the second lane's start pulse
  (D20).
- Emitter posts become **outer-edge** posts, two per track position instead of
  one: ten for a full two-lane build.
- Trunk length is unchanged, so the cable purchase is no longer blocked.

**Would change it:** a venue with no usable centre zone. The only remaining
option there is a side trunk with the far lane served by its own run, joined
beyond the shutdown area where crossing is safe — which costs a second
full-length cable.

---

## D22 — Node firmware in C on ESP-IDF, not Rust

**Status:** revisit (accepted for v1) · **Scope:** timing node and tree module
firmware

The timing model rests on three hardware mechanisms: a monostable outside the
MCU (**D16**), MCPWM capture channels taking both edges of a beam, and the
capture timer being reset from a GPIO sync source by the start pulse (**D20**).
Whatever language the firmware is written in must not stand between those
mechanisms and a scope trace. Rust on ESP32 is real and offers two routes —
`esp-hal` (bare-metal, `no_std`) and `esp-idf-hal` (bindings over ESP-IDF) — so
the question is a capability check, not a preference.

**Evidence, checked 2026-07-30** (dated deliberately: `esp-hal` is actively
developed and this is a snapshot, not a permanent property):

- `esp-hal`'s MCPWM module documents **"Capture Module (Not yet implemented)"**,
  and hardware sync / phase reload of the timer likewise as not implemented.
  Those are exactly the two mechanisms D20 rests on. MCPWM also sits behind the
  crate's `unstable` feature, where breaking changes land in minor releases.
- ESP-IDF exposes both as first-class API. A capture channel takes `pos_edge`
  and `neg_edge` **together**, reporting which edge fired in the event data; and
  `mcpwm_capture_timer_set_phase_on_sync()` accepts a GPIO sync source.
  Espressif ships a working capture example.
- `esp-idf-hal` wraps most IDF drivers, but for this peripheral the realistic
  path is `unsafe` FFI through `esp-idf-sys` into the same C driver.

**Decision:** C on ESP-IDF for v1 firmware.

**Why:**

1. The two APIs the timing model depends on are first-class in one option and
   unimplemented in the other. Bare-metal Rust would mean PAC-level register
   programming for the project's zero point — the one place where a bug is
   invisible, because a jittering zero corrupts every downstream number
   identically (**D16**).
2. Via `esp-idf-hal` it is the same C driver behind a binding layer. That is
   ESP-IDF with a layer added, and the layer sits precisely where **D01**'s
   "here is the trace, check it yourself" argument says not to add one.
3. Espressif's capture examples and the ESP-IDF issue history are the debugging
   corpus for a peripheral nobody on this project has used yet.
4. The firmware is small — capture, Modbus slave, DIP read, telemetry, flash
   log, CLI — and few people will touch it. The contributor-pool argument that
   favours a familiar language is much weaker here than on the PC side, where
   the surface is large and club-facing. **D23** goes the other way for exactly
   that reason.

**This deliberately splits languages, and that is not an inconsistency.** The
reasons for Rust on the operator's laptop — one static binary, no runtime to
install in a field with no internet — do not apply to firmware at all: firmware
is a flashed binary in every language. Deriving either choice from the other
would be the wrong reason to make both.

**Amended 2026-07-30 — the PAC route, and a correction to the reasoning above.**

The argument above leaned on `esp-hal` not implementing capture, which
overstated the obstacle. The `esp32s3` peripheral access crate exposes the MCPWM
block in full, and every register D20's mechanism needs is there, documented:

| Register / field | Documented behaviour |
|---|---|
| `CAP_TIMER_CFG.CAP_SYNCI_SEL` | "capture module sync input selection … 4: SYNC0 from GPIO matrix, 5: SYNC1 …, 6: SYNC2 …" |
| `CAP_TIMER_CFG.CAP_SYNCI_EN` | "When set, capture timer sync is enabled" |
| `CAP_TIMER_PHASE` | the value the capture timer is loaded with on sync — set it to 0 and the pulse zeroes the timer |
| `CAP_CH_CFG.MODE` | "When bit0 is set to 1: enable capture on the negative edge, When bit1 is set to 1: enable capture on the positive edge" |
| `CAP_STATUS` | which edge fired |

So D20's mechanism is four register writes plus a GPIO matrix route, not a
driver port. `esp-hal` supplies the GPIO matrix, clocks and interrupts, and a
PAC-based module for a peripheral the HAL has not wrapped is an ordinary way to
work, not a workaround.

**Two concessions this forces:**

1. Calling the PAC route a *cost* was rhetoric it cannot support at this size.
2. More importantly, register-level code is arguably **more** auditable than a
   vendor driver, not less. Four register writes tell a reader exactly what the
   silicon was told; `mcpwm_new_capture_channel()` does not. That is **D01**'s
   own verifiability argument pointing away from this decision, and it deserves
   to be recorded as such.

**What still decides it, and it is not the language.** The project has no
measurements at all yet. **T3** is the gating test for this whole path, and a
self-written capture module adds a third suspect to it — sensor, silicon, or our
own driver — where there were two. The vendor driver is not better code; it is
the *reference* against which a self-written path can be shown correct.

**Decision, restated:** C on ESP-IDF for the firmware that produces the T3
number, so that measurement carries as few unknowns as possible. Once T3 has a
number, a Rust node is admissible the moment it **reproduces** it on the same
rig — same disk, same reference detector, same pass count. That is the bar every
other choice in this project has to clear, and the firmware language has no
claim to an exemption.

**Status changed to revisit.** "Accepted" overstated it: the evidence above
weakens the original reasoning enough that the record should read as open, with
a test attached instead of an opinion.

**Amended 2026-07-31 — Tier A run: the ecosystem clears, with one correction to
the above.**

Tier A ([`software.md`](software.md) §7) audits whether the `no_std` Rust pieces
this firmware needs exist at all, before any hardware is involved. Run against
esp-hal 1.1.1, `esp32s3` PAC 0.35.2, rmodbus 0.12.2, esp-storage 0.9.0, with a
probe crate built and linked for `xtensa-esp32s3-none-elf`.

| Item | Result |
|---|---|
| PAC capture + interrupt registers | present — `CAP_TIMER_CFG`/`PHASE`, `CAP_CH_CFG`, `CAP_CH`, `CAP_STATUS`, `INT_ENA`/`CLR`/`ST`/`RAW`; the whole D20 sequence type-checks |
| Peripheral clock and reset | `McPwm::new` creates a `PeripheralGuard`; `PwmPeripheral::block()` then exposes the capture registers through public API |
| Modbus slave | `rmodbus` builds `no_std` for xtensa — a frame processor plus register context, which is the shape this design wants |
| Flash log | `esp-storage` builds, and ships a host emulation mode useful for tier-1 tests |
| USB CLI | `UsbSerialJtag`, tx/rx split, blocking and async |
| 1-Wire (DS18B20) | crates exist, maturity mixed; off the timing path, so worst case is a bit-banged driver |
| Builds and links | 251 KB ELF, all of the above in one dependency tree |

**Correction to the amendment above.** It claimed "`esp-hal` supplies the GPIO
matrix, clocks and interrupts." Clocks: confirmed, in source and in the
disassembly. GPIO matrix: **wrong** — esp-hal 1.1.1 exposes no public path to
route a pad to an arbitrary peripheral input signal, so MCPWM0_SYNC0 needs
PAC-level `func_in_sel_cfg` plus the signal index from the TRM. Interrupts: they
were untested when the table above was written, and B1 below closes that.

**B1 — the capture interrupt, same day.** Binding through
`interrupt::bind_handler(Interrupt::MCPWM0, h)` plus
`interrupt::enable(…, Priority::Priority3)` compiles and links, with a
`#[handler]` function that reads `CAP_CH`, takes the edge from
`CAP_STATUS.cap0_edge` and clears `INT_CLR.cap0`. The handler symbol survives
into the 252 KB image. Three things worth having in writing:

- **Signal indices are documented, not guessed:** ESP-IDF's
  `soc/esp32s3/gpio_sig_map.h` gives `PWM0_SYNC0_IN_IDX` = 160 and
  `PWM0_CAP0_IN_IDX` = 166.
- **Capture inputs are GPIO-matrix routed too**, not only the sync pulse. Every
  beam input needs a matrix route — a wiring fact the firmware owns whatever
  language it is written in, and one the C path will meet identically.
- Two ergonomics, small but time-wasting: `esp_hal::pac` is `pub(crate)`, so the
  register-block type cannot be named and an accessor has to be a macro rather
  than a function; and `INT_CLR` fields are one-to-clear, so the method is
  `clear_bit_by_one()`, not `set_bit()`.

What B1 does **not** show, and no desk work can: that the handler fires, that
the sync actually zeroes the timer, and that those indices are right in silicon.
All three are Tier C.

**The trap worth recording, because it costs a day and looks like something
else.** Without `-C link-arg=-Wl,-Tlinkall.x` the link fails with ~97 undefined
interrupt-handler symbols (`DMA_IN_CH0`, `USB_DEVICE`, `CACHE_CORE0_ACS`, …). It
reads exactly like an ecosystem failure and is missing scaffolding. Lesser
versions of the same: `esp-backtrace` and `esp-println` each need exactly one
output feature *and* `default-features = false`, or their build scripts panic.

**A wrong finding, recorded because the method demands it.** Mid-investigation
this log was going to state that the `esp32s3` PAC must not be added as a second
dependency, on the evidence of that same link failure. Re-tested with
`linkall.x` present, the separate dependency links fine. Reaching the registers
through `PwmPeripheral::block()` is still preferable — one PAC instance, no
version skew — but that is a preference, not a requirement, and the difference
matters to anyone reading this as guidance rather than as a diary.

**Confidence, and what would lower it.** Ecosystem risk was judged 50–60% before
the run and ~80% after. What would move it back down: the capture interrupt not
binding cleanly through esp-hal (untested); `unstable` API churn breaking the
build across a minor esp-hal release; any discrepancy at all in the T3
comparison; or nobody taking the work on, which the estimate silently assumes.

None of this changes the decision, because the decision is about **sequencing**:
whichever firmware produces the T3 number should carry the fewest unknowns.

**Would change it:** a Rust node clearing T3 against the C reference. Not
`esp-hal` gaining a capture module — that would be convenient, but it was never
the real obstacle.

**Noted 2026-07-31 — race control is being built on the assumption this
reverses.** **D27** puts the register map in a shared `no_std` crate, which is
only shareable if the node is Rust. That does not change this decision or its
status: the bar above is still T3 reproduced on the same rig, and nothing has
been measured. It is recorded here so that a reader of D22 alone knows software
work is leaning on the outcome. If it does not reverse, D27's fallback is a
C-header emitter over the same map.

**Noted 2026-08-03 — a second reason appeared, and it is not about T3.**
**D31** describes a deployment where the tree is the bus master and the whole of
race control runs on it: a club with a tree, two nodes and a phone, and no
computer. If firmware stays C, that product needs the race logic implemented a
**second time in C** — and **D26**'s argument, one pure implementation replayed
against a simulator, dies for it. With a Rust node it is the same crates on a
smaller target.

This changes nothing about the bar, which is still T3 reproduced on the same
rig. What it changes is the price of failing it: when this decision was written
the cost of staying on C was a header emitter, and it is now a header emitter
plus a duplicate implementation of the rules for anyone who wants the small
product.

---

## D23 — Race control in Rust, one binary, scoreboard served in-process

**Status:** accepted · **Scope:** race control PC software ·
**Supersedes:** the Python/Node split reserved in `.gitignore`

`.gitignore` reserved Python for race control and Node for the web scoreboard.
Neither had a decision record behind it. The design priorities in
`architecture.md` put **field repairability second, immediately after
trustworthy timing** — and repairability is a property of what gets deployed,
not of what gets written.

**Decision:** race control in Rust, shipped as a single static binary that also
serves the scoreboard from the same process. The Node reservation is dropped.
The Python reservation survives for bench tooling only
([`software.md`](software.md) §6).

**Why:**

1. On a laptop at eight in the morning at a track with no internet, a virtual
   environment, an interpreter version and a package index are three things that
   can differ from the machine the software was built on. One binary is one
   artifact to copy.
2. "Fully functional with no internet" is a design invariant. Two runtimes and
   two dependency trees are two ways to violate it at the worst possible moment.
3. `tokio-modbus` covers RTU master over serial, so **D05**'s "mature libraries
   on both ends" holds without a custom stack.
4. The race logic is a state machine whose failure mode is a *plausible wrong
   number*, not a crash. Exhaustive matching and types are worth more against
   that than iteration speed is.
5. The scoreboard displays latched numbers on a LAN page. It is not an
   application, and it does not justify a second runtime on the operator's
   machine.

**Cost, and how it is paid.** Rust narrows the contributor pool relative to
Python, and `CONTRIBUTING.md` courts club organizers and field practitioners,
not systems programmers. The mitigation is architectural rather than social:
everything a club plausibly wants to change ships as **data** — class and
bracket rules, dial-ins, tree modes, scoreboard and time-slip templates, and the
mapping file (**D08**). A club changing a class rule or a slip layout must never
see a compiler.

**Amended 2026-07-31 — the contributor-pool cost is smaller than stated, and
this decision is stronger for it.**

The cost above was written as though an unfamiliar language keeps contributors
out. With competent coding assistants that barrier is materially lower: someone
who knows Python can now make a targeted change in Rust. Conceded. Three
qualifications, because it does not go to zero and it moves in a direction worth
stating precisely.

1. **The barrier was never writing the code; it was knowing the change is
   right.** This codebase's failure mode is a plausible wrong number, not a
   crash. Assistants are strongest at "compiles and reads idiomatically" and
   weakest at "which invariant did that just violate." What makes assisted
   contribution safe *here* is **D26**'s replay fixtures plus the generated
   register contract ([`protocol.md`](protocol.md) §0) — mechanical checks that
   do not require the maintainer to read the contributor's Rust at all.
   Assistants plus that harness dissolve most of this cost; assistants alone do
   not.
2. **Field repairability is untouched.** This decision's primary argument is one
   binary at eight in the morning at a track with no internet — where assistant
   availability is exactly zero. So the argument reduces a *cost* without
   touching the *benefit*, which makes this decision stronger rather than
   weaker.
3. **A cost that was underweighted, and it points the other way.** With race
   control fixed on Rust, C firmware (**D22**) means two languages permanently —
   two toolchains, two test setups, two idioms, for a project with one engineer.
   A single-language codebase removes that, which is an argument for a Rust node
   having nothing to do with capture jitter. It raises the estimated likelihood
   of D22 eventually reversing from ~40% to ~55–65%. It does not change D22's
   sequencing, and D22 records what would move that number back down.

**Would change it:** the customization surface leaking into Rust in practice —
that is the measurable symptom, and it reverses this decision. Also: a
contributor arriving with working Python race control before this one exists.
Running code outranks the argument above.

---

## D24 — The node has no role; it publishes state, the master interprets

**Status:** accepted · **Scope:** node firmware, register map

**D07** says one firmware for every position. **D08** says the DIP address is a
node's *only* configuration. Then **D20** gives downstream nodes a per-lane
MCPWM group binding, and gives one start-area node the job of capturing both
start pulses on a single timer to produce the launch margin. That is
position-dependent behaviour, and it has three possible homes: node flash
(breaks D08), a configuration protocol from the master (the discovery-and-config
machinery D08 exists to avoid), or nowhere.

**Decision:** nowhere. Every node captures everything it can, always, and
publishes all of it — both edges of every populated input on both lanes' capture
groups, both start pulses observed on a common timer with their measured widths
and their difference, telemetry, faults and live line state. The master reads
what is meaningful for that address according to the mapping file and ignores
the rest. A register that is meaningless at a position reads "not seen this
run", which is data, not an error.

**Why:**

- No node knows its role, so there is no role to configure, mis-set, or lose in
  a field swap. **D08** stays literally true, and **D11**'s "a spare is a spare
  for any position" extends from the enclosure to the software.
- Firmware contains no `if (position == START)`. There is one binary to build,
  flash and trust.
- Interpretation stays in the one place that already owns it — the mapping file.
- The cost is a handful of registers nobody reads, at two bytes each.

**Would change it:** a capture-channel budget too tight to observe everything at
once. Under D20 two of six channels are used, so there is no pressure — and the
confirmation that one channel takes both edges (**D22**) is what keeps it that
way.

---

## D25 — Results latch in registers; the master polls a digest for change

**Status:** accepted · **Scope:** bus protocol, master poll loop

Nodes never transmit unpolled (§1), a poll cycle is tens of milliseconds, and a
run delivers its events at instants nobody schedules. Something has to guarantee
that no result is ever missed or counted twice. The obvious mechanisms — an
event FIFO with acknowledgement, or unsolicited reporting — both add protocol
state that then has to survive retries and reboots on 450 m of cable next to
ignition systems.

**Decision:** results latch. Per lane, a node holds a generation counter that
increments when its capture timer is synced, a flags word, and the captured
instants; the values stay until the next start pulse overwrites them. The master
polls a four-register digest every cycle and fetches a full run record only when
that lane's generation moves.

**Why:**

- Missing an event becomes **impossible rather than unlikely**. There is no
  queue to overflow and no acknowledgement to lose, and a poll that arrives
  seconds late reads exactly the same numbers.
- It fits §3's quiet window instead of fighting it. Polling stops during a run,
  and the unhurried moment to read results is the moment just after they exist.
- It fits the bus budget, which the naive design does not: 19,200 bps is about
  192 characters per 100 ms **for the whole bus**, while a full two-lane record
  is ~69 characters for a single node. Records cannot live in the steady-state
  loop, and here they do not need to.
- Retries are free, because nothing in a node changes as a result of being read.
  The raw-log cursor is moved by an explicit command for the same reason — a
  read-advancing cursor makes a retried read return different data, which is
  precisely what a noisy bus generates.
- Generation 0 means "no run since boot", and wrap goes 65535 → 1, skipping 0.
  The failure this design must not have is a rebooted node appearing to hold a
  valid split; distinguishing a wrap from a reboot is what prevents it.

**Would change it:** a result set too large to latch — which would mean the raw
edge log had migrated into the live path. It should not: the log is dispute
evidence, pulled after the round.

**Amended 2026-08-02 — the generation counts record changes, not syncs.** As
first written, the counter incremented "when its capture timer is synced". The
first end-to-end test of the master proved that unusable: the sync happens at
the launch and the beams are crossed seconds afterwards, so the master read a
record that was valid, current and **empty**, with nothing in the four-register
digest to say otherwise. The obvious alternative, `status_flags.run_complete`,
cannot serve either — on a node whose inputs are shared between lanes it never
sets, for the reason recorded in [`software.md`](software.md) §8 #7.

The counter therefore increments on **every change to that lane's latched
record**: the sync, and every capture that lands in it. Three things follow, and
none of them cost a register:

- "Read on change" delivers a result instead of an empty record, which is what
  the decision claimed all along.
- The read becomes self-checking. The record carries its own generation, so a
  master can tell whether what came back is still current or whether more landed
  while it was in flight.
- Generation 0 is **not** left by a capture — only by a sync. An edge caught
  before the first pulse, or after a reboot, is recorded with the timer
  free-running and stays at 0. That is the same defence as before, and the
  amendment would have quietly removed it if the simulator's reboot scenario had
  not failed within the minute.

The evidence is the test named
`the_generation_moves_when_a_beam_lands_not_only_when_the_run_starts` in
`software/crates/node-core`, and the bracket round in `software/crates/race`.

**Challenged 2026-08-03 — "the tree and beams should be autonomous, and push
events to the console."** Recorded because it is the obvious question and it
will be asked again.

The principle behind it is right: *the system that measures must not depend on a
general-purpose computer being awake.* It does not. The pulse comes from a
monostable, the instants from hardware capture, and the results sit latched in
registers — the master is **already silent** from the arm to the finish, by
design (`architecture.md` §3). What its absence costs is knowing, not measuring:
a master that dies mid-round and restarts re-reads the same numbers and the
round still completes.

Two things cannot move into the devices, and neither reason is dogma:

- **The meaning of a beam.** For a tree to decide "both staged" on its own it
  must know that address 1 input 1 is lane 1's stage beam, which is exactly what
  **D08** keeps in the mapping file alone. The cost of duplicating it is
  concrete: today a dead node is replaced by copying DIP positions, with no
  reflash and no laptop. Once meaning is in flash it lives in two places, they
  drift, and the drift surfaces as a *plausible wrong result* rather than as a
  failure. Deep staging and guard-beam rejection would follow it, taking
  **D23**'s "logic in one place" and **D26**'s replayability with them.
- **A second talker.** Only the polled node transmits (**D05**), which is what
  makes collisions impossible on 450 m of half-duplex line beside high-energy
  ignition. Pushing adds a queue that can overflow, an acknowledgement that can
  be lost, and a transmission that can collide — recovered by a retry that can
  collide again.

What push would genuinely buy is latency, and there is exactly one place it
costs anything: the staging lamps, at ~210 ms, which is ~2 cm of creep
([`software.md`](software.md) §4). That has a cheaper answer already written
down — §8 #10's tiered cycle, which polls the start nodes every cycle and the
rest every third, and brings the lamp path to ~40 ms without giving up
collision-freedom.

**What would change it:** a requirement to run a session with *no computer at
all* — a club arriving for test-and-tune with a tree and beams and nothing else.
That is not an amendment to this record; it is a second product, a standalone
tree with a small display of its own, and it would contain a master too — a
small one, without a ladder.

---

## D26 — Race logic is a pure function; the simulator is the reference client

**Status:** accepted · **Scope:** race control architecture, test strategy

Hardware is on order, so software has to proceed without it. The more durable
reason is that race logic fails *quietly*: a wrong ET still looks like a number,
and no amount of manual testing at a track distinguishes it from a right one.

**Decision:** the race logic layer takes a timestamped event stream plus the
mapping file and returns state and results, with no I/O — no serial handle, no
clock read, no filesystem. The bus lives behind an interface whose second
implementation is a node simulator replaying scripted runs, including the ugly
ones: invalid pulse width, a node rebooting mid-run, a silent node, a beam that
breaks and never makes again, two cars launching 3 ms apart. A recorded bus
session is a test fixture and replays deterministically.

**Why:**

- It is what makes every piece of race-logic work possible before the DevKits
  arrive — the immediate reason, and the weakest of these.
- Determinism is the software half of **D01**'s verifiability argument. "Here is
  the session, replay it, get the same ET" can be checked by anyone; "trust the
  master" cannot.
- Disputes get a mechanism instead of an opinion: the session that produced a
  contested time slip can be re-run against the same logic.
- A simulator that replays only clean runs validates nothing. The failure list
  above is the specification, not a wish list.

**Would change it:** nothing foreseeable. If the pure core acquires I/O for
expedience, the replay property is gone — and this record is what was traded
away to get there.

**Implemented 2026-08-02.** A session is a text file of one line per
transaction, and replaying it is a third implementation of the same `Bus` trait
the simulator and the future serial port sit behind. Three properties were
worth the shape:

- The replay drives the **real** poller, staging machine and race logic. A
  harness that re-derived a result from a stored summary would prove the summary
  was consistent with itself, which is not what anybody asks about a disputed
  slip.
- The recording carries the mapping file and the pairing, so it answers "what
  was this a race between" with nothing else on the machine. Evidence that needs
  three other files is not evidence.
- A replay that is asked for a transaction the recording does not have **stops
  and says where**, rather than serving the nearest match. A divergence is a
  statement about the code, not about the race, and the useful half of the
  feature is that a changed poll schedule cannot come back as a quietly
  different time slip.

`beam402 sim … --record <file>` writes one; `beam402 replay <file>` re-runs it.
The equivalence is asserted by `a_recorded_session_replays_to_the_same_slip`.

---

## D27 — The register map lives in code; the documents are generated or checked

**Status:** accepted · **Scope:** wire contract, documentation workflow ·
**Amends:** [`protocol.md`](protocol.md) §0

[`protocol.md`](protocol.md) §0 established one machine-readable source —
`registers.toml` — with firmware headers, race control structs and §3's tables
all to be generated from it. Nothing generated from it, because no code existed.
Writing the master's half changed the shape of the problem twice over.

First, **D22**'s Tier A run put the `no_std` Rust node's ecosystem risk at
roughly 80 % clear, with only silicon outstanding. A crate shared verbatim by
both halves stopped being hypothetical.

Second, and more decisive: layout turned out to be the cheap half of this
contract. What produces a *valid number read wrong* is not an offset. It is a
generation compared with `>` instead of `!=`, a `Ticks(0)` standing in for
"never observed", an `input_state` bit read with the intuitive polarity, a run
counted while `invalidated` is set. A TOML file can carry all four as prose. It
cannot enforce one of them.

**Decision:** the map is the `beam402-protocol` crate — `no_std`, no
dependencies, shared verbatim by race control and node firmware.
`registers.toml` is generated from it in full; §3 keeps its prose and has its
numbers checked against it. Both guards run in CI.

**Why:**

1. The invariants become unwritable-wrong rather than documented. `Generation`
   is not `Ord` (**D25**); an unobserved instant is `None`, not zero;
   `beam_intact()` names **D17**'s polarity in the accessor; `is_timing_valid()`
   is `valid && !invalidated` (**D16**); `Millis` and `Ticks` cannot be mixed
   (**D20**). Each of those was a sentence somebody had to remember.
2. One source for both halves with no generator to write and maintain. §0's
   argument was against *transcription*, not in favour of TOML specifically.
3. Encode and decode sit together, so the node's register layer and the master's
   parser cannot disagree about layout. [`software.md`](software.md) §3's "pure
   function of (events, config)" gets one implementation instead of two.
4. The documents keep guards rather than good intentions: `render-map check` for
   the generated file, `render-map check-tables` for §3.

**The premise, stated plainly.** This is available only if both halves are Rust,
which **D22** has not settled — it stands at *revisit*, and its bar is T3
reproduced on the same rig. So this record rests on an assumption where every
other decision in this log rests on a measurement or a datasheet. It should be
read that way, and it is the owner's call rather than a finding.

**Cost, and why the bet is cheap:**

- If a Rust node never clears T3, the crate stays the source and gains a
  C-header backend — the same walk over the same table. An emitter, not a
  redesign. That is the entire exposure.
- A register move is now a Rust diff rather than a TOML diff, legible to fewer
  of the contributors `CONTRIBUTING.md` courts. Mitigated, not removed:
  `registers.toml` is still committed, so the change still shows up in a form a
  non-Rust reader can check.
- §3 is checked rather than generated, so its *prose* can still drift from its
  tables. Smaller and slower than a wrong offset, and accepted knowingly.

**Would change it:** a firmware toolchain that cannot consume a generated
header, which would put the source back in a neutral file with two backends.
Note what would **not** change it: **D22** failing to reverse. That case is
priced in above.

---

## D28 — The tree runs one cascade per lane

**Status:** accepted · **Scope:** tree module, register map, race logic

Bracket racing is the format most clubs actually run, and it is the reason a
1,200 kg street car and a dragster can meet in a final. Each driver predicts an
ET; the slower car leaves first by the difference between the two predictions;
running **quicker** than the prediction loses. Two drivers who both hit their
dial exactly cross the finish line together.

The system had no way to express it. `tree_arm` carries a mode and a random
delay bound and nothing else, the tree lit one set of ambers for both lanes, and
`tree_state` and `lamp_state` were value spaces only the simulator knew.

**Decision:** the tree runs **two independent cascades**, one per lane, offset
by a per-lane handicap in milliseconds. The handicap is written with a new
`tree_handicap` opcode before `tree_arm`, which latches it; the tree echoes the
armed value in two new registers, so the master can verify it before a car
stages. The ambers, green and red move into a per-lane `lamp_flags` word, and
`tree_state` becomes an enumeration in the shared crate rather than a
convention.

**Why:**

- **Nothing downstream changes, and that is the finding.** ET's zero is still
  that car's own launch pulse, so a car that waited four seconds on the line
  measures exactly what a car that did not would (**D04**). Reaction time is
  still `t_pulse − t_green` on the tree's own clock — against *that lane's*
  green, which is why the map has had `t_green_l1` and `t_green_l2` from the
  start. **D20**'s launch margin needs no term added: the handicap *is* part of
  the difference between the two pulses, so `(pulse₂ − pulse₁) + ET₂ − ET₁` is
  the finish order in a bracket exactly as it is heads-up.
- A shared amber column cannot render a handicap start. The two lanes are
  genuinely in different places — one car is on its second amber while the other
  is dark — and an operator display that cannot show the race it is watching is
  worse than none.
- The handicap is **volatile per round**, latched by the arm and cleared from
  pending, so **D08**'s "the DIP switch is the node's only configuration"
  survives. A spot forgotten from the previous pair fails to a heads-up start,
  which everyone can see, rather than to a stale head start, which nobody can.
- Doing it now is nearly free. The tree block grew from 13 registers to 15 and
  no firmware exists to break. The same change after a season in the field costs
  a `protocol_version` bump and a flash of every tree.

**Cost:** two registers, one opcode, and a `lamp_flags` word that did not exist.
The register map is the most expensive thing in this project to change later,
and this spends some of that budget on a format the hardware had not been asked
about.

**Would change it:** a club that runs only heads-up carries the two registers
and never writes the opcode — the right shape for an addition that most
installations use and none are burdened by.

---

## D29 — The scoreboard is a frame of pixels at a declared resolution

**Status:** accepted · **Scope:** spectator scoreboard, race control

**D23** put the scoreboard on a LAN page reached by a QR code and called it
"latched numbers on a page, not an application". That is still true and this
does not change it. What it did not say is what shape those numbers have, and a
web page free to lay itself out will grow a layout no LED panel can render.

That matters because a real drag strip's board is an LED matrix, and the day one
exists here the served page should be its **preview and its fallback** — a club
without a board casts the page to a television and sees the same thing. Two
independent layouts would have to be reconciled later, and "later" means after
somebody has bought panels.

**Decision:** race control renders the scoreboard to a **monochrome frame of
pixels** at a declared resolution. The page draws that frame as diodes, on the
diode pitch, with the unlit ones drawn too. An LED panel, if one is ever built,
takes the same bytes. The reference geometry is **128 × 32 per lane**, which is
a whole number of the 32 × 16 module every LED sign is assembled from.

**Why:**

- **The constraint is the point, not the sharing.** "Does it fit" becomes a test
  that runs at a desk instead of a discovery made in front of a supplier. Two of
  them fail today if a line grows by one character.
- It makes the cost of a field visible. A band spends 7 + 14 + 7 rows plus a
  separator — 29 of 32. There is **no fourth line**, so adding 60 ft or a
  driver's name buys a taller band and therefore more panels, and that trade is
  now written down rather than discovered.
- Nothing is decided twice. Who won, what broke out, which split is missing all
  arrive settled from the race logic; a board that reasoned would be a second
  implementation of the rules, which is one more than anybody can keep right.
- It keeps **D23**'s single binary honest. Drawing pixels needs no web
  framework, and the page fetches nothing.

**No board has been bought or specified**, and **D15** gates that until the
bench answers. 128 × 32 is a plausible geometry to design against, not a
purchase order.

**Cost:** the page cannot reflow, so on a phone it is a picture of a board
rather than a responsive document. That is intended — the scoreboard's audience
is a grandstand, and a spectator who wants to read a slip on a phone is asking
for the operator UI, which is a different thing with a different job.

**Would change it:** a club that runs entirely without a physical board and
wants the page to be the product. The frame would stay — it is what the race
logic hands over — and the page would gain a second renderer beside the diode
one, rather than the frame being abandoned.

---

## D30 — Race control is an appliance; everybody else is a client

**Status:** accepted · **Scope:** deployment, race control software, operator UI

`architecture.md` §9 said race control "runs on a laptop at the start area", and
`software.md` §1 named the operator laptop as the machine. That was written when
the only human in the picture was one operator.

A real event has several: a starter, a tower, an entry desk, and spectators. And
a laptop is somebody's *personal* machine — it sleeps, it gets closed, its owner
goes to lunch, its battery runs out. The bus master disappearing mid-round is
the worst thing that can happen to a timing system, and a machine with a lid is
a poor place to put it.

**Decision:** race control runs on a small dedicated machine at the start area —
a Raspberry Pi, a mini PC, a Mac mini, whichever is at hand — and serves every
human interface over the LAN. Starter, tower, entry desk and scoreboard are all
**clients**. Nobody's laptop is load-bearing.

The machine is not a decision and is deliberately not specified. What is
specified is what it has to do:

- **Not sleep, and boot into the application.** No lid, no login, no desktop.
- **Survive losing power** without losing an event — which the results database
  and the session log already provide for.
- **Be on the trunk.** It holds the USB-RS485 adapter (`BOM.md` 2.27), so it is
  a device on the bus: a stub of ≤ 2 m, or one of the two terminated ends. It is
  the *only* participant with that constraint — every other human is on Wi-Fi
  and can be anywhere.

  Note what that does **not** say. It does not say "at the start line". The
  trunk is already 450 m of cable running the length of the strip (**D21**), and
  which building it ends at is a choice. Routing it to end where the organisers
  actually sit — a tower, a caravan, an office — puts the machine under a roof,
  on mains, beside the people who restart it, and still on copper. More cable is
  the cheapest thing in this system.
- **Need no internet, ever.** Unchanged, and the reason this is one binary.

**Why this is barely a change:**

- **D23** already serves the scoreboard from the same process. Extending that to
  the operator's own screen adds a page, not an architecture. There is still one
  binary, one runtime, one dependency tree.
- **D25** makes the failure survivable, and this is what it was for. Results
  latch in the nodes; nothing is queued and nothing waits on an acknowledgement,
  so a master that dies and restarts re-reads the same numbers. What an outage
  costs is the display and the ladder, never a measurement.

**What it forces, and this is the substantive part.** Several clients means
several people who can act, and two people arming the tree is worse than
nobody arming it. So the server holds a **single control token**: exactly one
client at a time may arm, abort or advance a round, and it is visible on every
screen who holds it. This is not authentication — a club's LAN is not a threat
model — it is the same discipline **D05** applies to the bus, where only the
polled node may transmit. Collisions are prevented by construction rather than
by etiquette.

Roles differ enough to be different pages rather than one page with permissions:
starter (staging, arm, abort), tower (results, ladder, time slips), entry desk
(registration, dial-ins), scoreboard (read-only).

Which device holds which *architectural* role — master, store of record, control
client, relay — is one table for both deployments, in
[`software.md`](software.md) §4. The short of it: the master is always the
device on the bus, and a phone is never it.

**Cost:** one more box to own, power and not lose. Against that, the operator's
laptop stops being equipment, and a club can run an event from phones.

**If the cable genuinely cannot get there** — a temporary venue, a tower on the
far side of everything — a Wi-Fi bridge on the trunk is permitted: **D01** bars
radio from the pulse and from timing data, and this is neither. It carries one
requirement that is easy to miss. The bridge must **own the RS-485 timing**
rather than tunnel bytes. A transparent serial-over-TCP link hands the Modbus
timeout to the master, so Wi-Fi's tail latency — retries, contention, beacons —
arrives as a node that appears to have gone silent. That is not cosmetic: a
silent node costs the full response timeout times the retries, ~300 ms of bus
time every cycle, which is more than the entire healthy sweep. A gateway that
performs the transaction on the copper side and reports either an answer or a
real timeout turns radio jitter into latency instead of into a fault that did
not happen.

None of this touches accuracy. `architecture.md` §4: polling latency does not
affect it, because events are timestamped at capture and the bus only transports
the resulting numbers. Copper or radio, the ET is the same — so this is a
reliability and convenience choice, and it can be made when a real site is in
front of somebody rather than now.

**Would change it:** nothing about the box. What *would* change this record is
the server acquiring any part of the timing path. **D01** and **D04** put time
in the nodes, and the rule that keeps this decision cheap is that race control
displays and decides but never measures. A server outage must remain an outage
of the screen.

---

## D31 — A tree-hosted deployment: no PC, arm and read from a phone

**Status:** accepted, *gated on §11 #12* · **Scope:** deployment, tree firmware

**D25**'s record already named the case that would need a second product: a club
arriving for test-and-tune with a tree and beams and nothing else. That
requirement now exists. Runs are started from a phone and each run's numbers are
read on a phone, and there is no computer at the track.

**Decision:** in this deployment the **tree is the bus master**. It holds the
mapping file, polls the two or three nodes, runs its own sequence as it always
did, serves a page over its own Wi-Fi, and holds the latched results for the
day. `architecture.md` §12 carries it as a configuration beside Minimum, because
that is the hardware it runs on — start node, finish node, tree.

There is no ladder, no class, no qualifying and no scoreboard. Somebody arrives,
stages, launches, and reads their ET. That is the whole product.

**Wi-Fi, not Bluetooth, and the reason is on the phone rather than in the
radio.** BLE cannot serve a web page: reaching it needs a native app, because
Web Bluetooth does not exist in Safari on iOS and is not coming. That is half
the phones at a track unable to use the product without an installable, and for
an open-source project whose contributors are club organisers, maintaining two
native apps is an order of magnitude more commitment than serving a page. With
Wi-Fi somebody joins a network and opens an address — and it is the *same* page
the full deployment serves, so there is one implementation rather than two.

A **computer driving a monitor** settles it beyond argument. Putting results on
a screen means a browser open full-screen, which over Wi-Fi is a URL and over
BLE is Web Bluetooth — Chrome only, absent from Safari, awkward on the desktop —
or a third native application. And BLE's one real advantage, that a phone keeps
its own connectivity while talking to the tree, disappears the moment the tree
is a station on a router: there the phone has the tree *and* the internet on one
network.

**Prefer a station on a small router to being an access point.** A softAP on
this part carries few clients and a chip antenna reaches tens of metres, which
is short if the operator is standing anywhere but beside the tree. A travel
router and a power bank fix range, client count and the "network has no
internet" prompt at once, and the tree's radio then works less. SoftAP stays as
the fallback for a venue with nothing at all, where a phone usually keeps its
cellular data alive alongside — usually, being an operating system's behaviour
rather than a guarantee. An external antenna connector — the `-1U` module
variant — is a choice to make later and probably unnecessary beside a real
router.

**Wi-Fi Direct is not a candidate.** iOS has no usable form of it, Android's is
an awkward API that needs an application, and against softAP it buys nothing.

**Results travel by store-and-forward, not by luck with a signal.** The tree
holds the day; a client syncs what it does not have and uploads when it can. No
internet at the track loses nothing, because everything is still in the tree.
One phone with a signal carries the day home. Several clients all hold it, and
whichever reaches the network first uploads.

The relay is a **browser tab**, not an application. The mixed-content rule that
forbids a hosted site from calling a local address runs one way only: it blocks
an `https` page loading `http`. The tree's `http` page calling an `https` server
is an upgrade and is allowed, CORS in that direction is the server's to grant,
and Private Network Access restricts public→private rather than the reverse. So
the page that shows a run uploads it too, from a tab somebody already has open.
The four roles and which device holds each are tabulated in
[`software.md`](software.md) §4.

That requires a run to have a **stable identity**, assigned by the tree and not
by the client, or two phones uploading the same round produce two rounds. The
subtlety is a tree that restarts mid-day: numbering must not begin again, so the
identity carries `boot_count` or a session counter that survives a reboot. None
of this touches the wire contract — identity is assigned by the master when it
assembles a round, and the register map is unchanged.

**Derived, not random.** The identity is `MAC : session : run` — the factory MAC
this project already uses as a serial number (**D08**, **D13**), a session that
survives a reboot, and a run counter. Not a UUIDv4, for three reasons and the
second is the one that matters:

- The part's random number generator is only properly random with the **radio
  on**, which is true in this deployment and false in every other one, where
  **D13** turns it off. An identity scheme available in only one configuration
  is not an identity scheme.
- A random identifier **cannot be re-derived**. **D26**'s argument is "here is
  the session, replay it, get the same ET", and it should extend to *and the
  same run number*. A derived one is reconstructible from the latched registers
  and the log; a random one is not, and the same physical run gets a different
  identity after a restart.
- It is readable in a log, which matters at exactly the moment somebody is
  arguing about a round.

A database that wants a UUID column can hash the string into a UUIDv5 on the
server, deterministically, and the tree stays dumb.

**People are not the tree's business.** The same human across venues and seasons
is a cloud entity; a registry of them belongs on a server with a keyboard
attached, not in a device that has neither. The tree records a lane, a run, and
at most a label somebody typed on a phone.

The upload is **strictly additive**, which is not a preference but the project's
standing invariant: fully functional with no internet. Nothing waits on it,
nothing is lost without it, and a club that never configures a server sees no
difference at the track.

**Half of this requirement needs no radio at all**, and separating the halves is
what keeps the product from resting on an unmeasured assumption. Reading a run's
numbers is a **scoreboard frame**, which already exists (**D29**): a small panel
on the tree shows the last run, driven by the same monochrome frame a full board
takes. The radio then carries *arming from a phone* and convenience, and arming
in the worst case is a button.

That distinction is what §11 #12 is allowed to cost. With the panel, a radio
that disturbs the tree's sequence timing degrades a feature. Without it, the
same result deletes the product.

**Why it is nearly free.** Race logic is a pure crate, the poller is a crate,
and the bus is a trait with three implementations already. Running them on an
ESP32-S3 instead of a small machine is a **target change, not a second
implementation** — which is the only reason this is a deployment rather than a
fork. The tree is already a module of its own (**D07**) with its own block and
its own sequence machine (**D28**).

**What it changes, and none of it quietly:**

- **The tree becomes the master.** **D05** says exactly one device may talk
  unpolled, and here that device is the tree. Consistent rather than violated —
  but the tree's firmware then contains a bus master, which in the full
  deployment it does not.
- **The mapping lives on the tree.** **D08**'s substance survives: a *timing
  node* still carries nothing but its DIP address, the mapping is still one
  editable artifact rather than meaning compiled into flash, and a dead node is
  still replaced by copying DIP positions. What moves is where that artifact
  sits — uploaded over Wi-Fi to the tree instead of living on a PC.
- **A radio runs inside the device that captures the green.** This is the
  collision with **D13**, and it is why this record is gated. §11 #12 is the
  measurement.
- **Replay degrades.** **D26**'s session log is ~10 MB an hour against 16 MB of
  flash. The latched records for a day of racing fit easily — a run record is 28
  registers — but the full bus session does not. "Here is the session, replay
  it, get the same ET" becomes "here are the latched records", which is still
  evidence and less of it. An SD card on a carrier PCB would restore it.

**The consequence worth reading twice.** This raises the cost of **D22** not
reversing. If node firmware stays C, this product needs the race logic written a
second time, in C, and **D26**'s argument — one pure implementation, replayed
against a simulator — dies for it. With Rust firmware it is the same crates on a
smaller target. That is evidence for the Rust node which has nothing to do with
**T3**, and it did not exist when **D22** was written.

**What it adds to the software.** The tree serves two representations of the
same thing: HTML for a person, and a JSON export for a client that is syncing.
One server, one set of numbers, and the export is what makes a phone a relay
rather than only a viewer.

**Both, from the same origin — an API alone does not work.** The tempting shape
is a hosted site into which somebody types the tree's local address, and it is
the one arrangement browsers actively prevent: a page served over `https` may
not call `http://192.168.4.1`, mixed content is blocked hard, and nothing the
tree does fixes it. Serving the page over plain `http` makes the site itself
insecure; serving `https` from the tree needs a certificate no authority will
issue for a private address; a native application avoids all of it and costs two
app stores.

A page served **by the tree** is same-origin: plain HTTP to a local address, no
CORS, no mixed content, nothing to install. It is what every appliance with a
local interface does, and it is why the HTML is not a compromise. An application
can still be written later — it would use the same API — but it stops being the
condition for seeing a result at all.

Memory is not the constraint it looks like. The `N16R8` part in `BOM.md` 2.7 has
16 MB of flash and 8 MB of PSRAM; the scoreboard page this project already
generates is 16 KB and the `scope` page with an entire session embedded is
120 KB. The constraint is the session log, at ~10 MB an hour, and it is recorded
above.

**Would change it:** §11 #12 failing — a tree whose sequence timing is disturbed
by its own radio. The fallback is not subtle and costs one part: a second
ESP32-S3 beside the tree, holding the radio and the mapping and the master role,
with the tree left as the slave it is in every other deployment. The panel keeps
working either way, which is the point of having it.
