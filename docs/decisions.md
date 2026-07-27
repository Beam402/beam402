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

**Would change it:** field problems with reflector contamination/alignment at
the finish/trap could justify through-beam there specifically.

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
see D01). This is open question #2 in `architecture.md` and gates the MCU
choice.

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