# Beam402 — System Architecture

> Status: **prototype / pre-validation**. This document describes the intended
> architecture. Sections marked *unverified* have not been confirmed on a test
> bench yet. See [Open questions](#open-questions--unverified-assumptions).

Beam402 is an open source timing and race-control system for amateur drag
racing: staging beams, Christmas tree, ET / 60ft / speed trap measurement, and
race management software. Design goals, in order: **trustworthy timing**
(±1 ms), **field repairability** by non-specialists, **low cost** using
industrial off-the-shelf components, and **incremental deployment** (a minimal
start/finish setup must be a complete, working system).

## 1. System overview

```
[Start node]──[60ft node]──[1/8 node]──[Finish node]     (trunk cable, ~450 m)
     │              │            │            │
  staging        interval     interval    finish +
  beams ×3        beam         beam      speed trap
     │
[Tree module]  [Race control PC (bus master)]
```

- **Timing nodes** — identical "dumb" timer boxes along the track. Each node
  timestamps beam events against a local counter and reports on request.
- **Shared trunk** — a single outdoor twisted-pair cable carrying the start
  pulse, the RS-485 data bus, and 12 V power for near nodes.
- **Tree module** — a separate device on the same bus that drives the
  Christmas tree lights and staging logic.
- **Race control PC** — the only bus master. Owns all race logic, class /
  bracket management, and the operator UI. Nodes never talk unprompted.

Roles live in software, not hardware: any node can serve any position on the
track by changing its DIP address and the mapping file on the race control PC.

## 2. Beam sensors

**Type:** industrial **through-beam** photoelectric sensors. Separate emitter
and receiver facing each other across the lane; the receiver sits on the trunk
side, the emitter on the far side with its own battery (§7). Reference part:
Autonics `BJ15M-TDT-C-P` — one part number supplies both units.

Retroreflective was the original choice and was reversed before the first
order; see **D02** for the reasoning and **D18** for what through-beam costs at
the start station.

Requirements:

| Parameter | Requirement | Why |
|---|---|---|
| Response time | ≤ 1 ms | timing resolution target; note this is a *max delay* spec, not repeatability — see §11 #1 |
| Sensing type | through-beam | no return path exists, so glossy bodywork cannot complete the beam; crosses smoke once, not twice |
| Range | ≥ 4× the span | rated range is clean-optics best case; margin is what survives smoke, dust and rain |
| Output | PNP, Light ON | fail-safe under wiring faults — see **D17** |
| Rating | IP67, 12–24 V DC | outdoor use. Autonics IP67 requires the connector (`-C`) variant; cable variants are IP65 |
| Ambient light | ≥ 11,000 lx at the receiver | datasheet ceiling; direct sun is ~100,000 lx — see **D19** |
| Ambient temp | −25…55 °C | *air* spec; a dark body in sun exceeds it — see **D19** |
| Emitter current | ≤ 20 mA | sizes the outer-edge post battery (§7) |

Cheap hobby IR pairs (Arduino-style modules) are excluded: response times of
10–50 ms with high jitter. Industrial sensors use modulated light with
synchronous detection, which rejects ambient light **up to the datasheet's
illumination ceiling** — not without limit. The earlier claim that they are
simply "immune" to sunlight was wrong; low sun in the receiver's axis is a real
failure mode, addressed by geometry in D18/D19.

Suffix traps that cost 20× the accuracy target if missed: in the Autonics BX
and BEN families `-FR` is a **relay** output with a **20 ms** response, while
`-DT` is DC solid-state at 1 ms. The BEN family is **IP50** and cannot go
outdoors at all. `-T` variants add an on-board 0.1–5 s delay timer, which
silently destroys timing if enabled — order the non-`-T` part rather than
managing it with a checklist.

**Beam layout** (matching established drag-strip practice):

- Pre-stage and stage beams 7 in (178 mm) apart along the direction of travel,
  ~15–20 cm above ground (to catch the tire, not body overhang).
- **Guard beam** 13 3/8 in (340 mm) downtrack of the stage beam. If stage and
  guard are broken simultaneously, the obstruction is bodywork (splitter, lip),
  not a tire, and the event is ignored. Mandatory for cars with aero.
- Interval and finish beams at ~13 cm (5 in) height — the height reference
  systems converged on for best repeatability.
- ET starts when the front tire **exits** the stage beam (rollout), not when
  the green lights. Reaction time and ET are independent.
- Speed trap: two beams with a measured base (~20 m) before the finish;
  speed = base / interval. Beam positions must be laser-measured; a 5 cm error
  in the base is a 0.25 % speed error — larger than the electronics'
  contribution.
- With two adjacent lanes, offset the beams of each lane 10–20 cm
  longitudinally and verify the sensors' mutual-interference suppression to
  avoid cross-lane blinding.
- **All receivers on the trunk side, all emitters on the far side.** The
  receiver produces the signal, so it must reach the node without crossing the
  racing surface. Alternating sides to separate adjacent channels is therefore
  not available — it would drag a signal conductor across the lane.
- **Beams must stay perpendicular to the direction of travel.** A tilted beam
  turns the car's lateral position into a longitudinal trigger offset — see
  **D18**. This is a timing requirement, not a mounting preference.
- Beam count for a single lane: 3 for a complete minimum system (pre-stage,
  stage, finish), 4 with guard, 6 with 60 ft and trap, **7** fully built out
  with 1/8. Each beam is one through-beam set, i.e. two devices on two posts.

**Detection is not the hard part; repeatability is.** A 30-inch tire presents a
~573 mm chord at a 130 mm beam height, so the beam stays broken for 20.6 ms at
100 km/h and 6.9 ms at 300 km/h — against a 1 ms response, a sevenfold margin
in the worst case. What is uncertain is whether the *instant* is reported the
same way every time (§11 #1).

**Beam cone and trigger sharpness are different widths, and only one of them
diverges.** The illuminated cone does: the parallel-shifting characteristic of
`BX15M-TDT` reaches ±200 cm at 15 m, a half-angle of ~7.5°. That is where both
the generous alignment tolerance and the adjacent-beam crosstalk (D18) come
from.

The width that sets *trigger sharpness* does not. For an opaque edge crossing
at distance *a* from an emitter of aperture Dₑ, with a receiver of aperture Dᵣ
at total span *L*, the edge must travel

```
W = Dₑ·(L − a)/L  +  Dᵣ·(a/L)
```

to go from unobstructed to fully blocked. At the receiver that reduces to Dᵣ,
at the emitter to Dₑ, and **for equal apertures W = D everywhere** —
independent of the span, and independent of where in the lane the car crosses.
Ramp duration is W divided by crossing speed: a ~12 mm lens at 27.8 m/s gives
~0.43 ms, and that is the window inside which the threshold crossing has to
stay repeatable (D03).

Three consequences, none of them obvious:

- Setting posts further back to house a collimator (D18) costs excess gain by
  the inverse-square law but **does not blunt the trigger edge**.
- A car on the left of the lane and one on the right get the same edge
  quality — unlike a tilted beam, where lateral position becomes longitudinal
  error.
- The D18 aperture stop narrows Dᵣ, so it mildly *sharpens* the edge on top of
  its other jobs.

This is geometric optics with a uniformly illuminated aperture. A real lens
falls off toward its edges, which softens the transition's shape without
changing its scale — the scale is the aperture, not the distance.

## 3. Timing model

The central trick: **no clock synchronization anywhere.**

1. The start node detects the tire leaving the stage beam and drives a
   **start pulse** onto a dedicated differential pair of the trunk cable.
2. The pulse propagates to every node essentially simultaneously (~2.3 µs over
   450 m of copper — three orders of magnitude below the 1 ms resolution
   target, ignored).
3. Each node starts its **local** hardware counter on the pulse and stops it
   (or timestamps) on its own beam edge.
4. Every split (60ft, 1/8, ET, trap interval) is therefore measured by a
   single local clock. Crystal tolerance (±20–40 ppm) contributes ~0.4 ms on a
   10 s run — acceptable; recheck if the trap base is ever shortened.

**Start pulse validation:** the pulse is a fixed-width 5 ms pulse, not a bare
edge. Nodes accept only pulses of the correct width; sub-millisecond spikes
(ignition noise coupled into 400 m of cable) are rejected.

Validation must **not** postpone the counter: nodes start on the leading edge
and retroactively invalidate the run if the width proves wrong. The naive
implementation — wait for the whole pulse, then start counting — adds exactly
5 ms to every measurement.

**The pulse is generated in hardware, not by firmware** (**D16**). A
firmware-driven pulse inserts software latency and its jitter between the
physical event and the shared zero of every node's measurement, where nothing
downstream can see or subtract it. A monostable triggered from the input does
not.

Timestamps are captured in hardware (ESP32 RMT/MCPWM capture or
high-priority GPIO interrupt with radios disabled), never by polling in the
main loop. *Capture jitter is unverified — see open questions.*

Crystal tolerance (±20–40 ppm ≈ 0.4 ms over a 10 s run) is **per-silicon
systematic, not noise**: measure each board once and store the correction on
the race control PC keyed by MAC — "passport, not job", per D08. A TCXO removes
it in hardware if the bookkeeping proves tiresome.

**Bus quiet window.** The start pulse and the data bus share one cable on
separate pairs. Width validation rejects short spikes but not a sustained
transmission burst, so the master stops polling from "both staged" until all
nodes have reported. One rule in the master, zero cost.

**End-to-end field verification.** The system must be able to prove itself
before a round: the master injects a known interval into every node's input
path and checks that all nodes return the same number. For a project whose
claim is verifiability, self-verification is not a luxury feature — it costs
one GPIO.

## 4. Data bus

- **Physical:** RS-485 half-duplex over one pair of the trunk, multi-drop,
  strict linear bus topology (no stars; stubs ≤ 2 m). 120 Ω termination
  jumpers enabled only on the two physical end nodes. Failsafe biasing at the
  master (or use true-failsafe transceivers such as MAX13487, which also
  handle direction control in hardware).
- **Protocol:** master–slave polling — **Modbus RTU** (addresses, CRC16,
  timeouts; mature libraries on both ends). Only the polled node transmits;
  collisions are impossible by discipline. A full poll cycle of ~10 nodes at
  19,200 bps takes ~50–100 ms.
- Polling latency does not affect accuracy: events are timestamped at capture
  time; the bus only transports the resulting numbers.
- Timeout + retry marks silent nodes for the operator — free liveness
  monitoring every poll cycle.
- RS-485 is rated for 1,200 m; at 450 m and ≤ 19.2 kbps the line is used at a
  small fraction of its capacity.
- **Explicitly rejected for timing:** any radio link (jitter, dropouts) and
  fiber optics (unsplicable in the field, connector contamination, cost —
  see decisions log).

## 5. Trunk cable

Outdoor shielded twisted pair (FTP cat5e outdoor, or field telephone wire as
the budget-indestructible option), ~450 m, run down the **centre island** of the
track — see **D21**. Both lanes' receivers sit in the centre, back to back,
facing outward across their own lane; emitters sit on the outer edges; nodes sit
in the centre beside the receivers they serve. Nothing crosses the racing
surface. Centre hardware must be low-profile and frangible, since it stands in
the impact path.

Pair assignment:

| Pair | Function |
|---|---|
| 1 | Lane 1 start pulse (differential, RS-485 transceiver driven) |
| 2 | Data bus (RS-485, Modbus RTU) |
| 3 | 12 V power for nodes near the source (start area) |
| 4 | Lane 2 start pulse — each lane's ET has its own zero (D20) |

Cable shield grounded at one end only (race control side). Node inputs are
optoisolated. The trunk passes **through** each node box on two connectors,
but the pass-through is plain copper traces independent of the node's power or
CPU — a dead or even gutted node never breaks the bus. Pre-cut and label trunk
sections to the measured inter-node distances (start→60ft, 60ft→1/8,
1/8→finish) so field deployment is "unroll numbered sections and connect."

## 6. Universal timing node

One PCB, one firmware, for every track position.

- **MCU:** ESP32-S3 DevKit socketed on a simple carrier board (KiCad,
  2-layer). The DevKit is a field-replaceable consumable. Radios disabled in
  firmware.
- **Inputs:** 4 optoisolated beam inputs. Start position uses 3
  (pre-stage / stage / guard), finish uses 2 (finish / trap), interval nodes
  use 1. Unused inputs are spare.
- **Input chain (part of the timing path, not plumbing).** Sensor M8 pinout is
  ① brown +V, ② white N.C., ③ blue 0 V, ④ black output; a through-beam emitter
  uses only ① and ③, so the far side carries power and nothing else. The PNP
  output drives the optocoupler LED through a series resistor to 0 V: **820 Ω**
  holds 8.0–13.0 mA across the whole LiFePO4 range (10.5 V → 14.6 V) with the
  PNP's 2.5 V max residual accounted for. A 24 V supply would need 1.8–2.2 kΩ,
  which belongs silkscreened on the board.
- **Optocoupler: fast and 3.3 V-capable** — `ACPL-M61L` / `ACPL-071L` class.
  PC817 is unsuitable (slow, asymmetric, CTR drifts with heat and age) and
  6N137 wants a 5 V output rail the ESP32 does not have. See **D13**. The
  optocoupler output needs a Schmitt/hysteresis input, not a bare GPIO, or a
  slow edge will double-capture on noise.
- **Pin selection:** on ESP32-S3 `N16R8` modules GPIO26–37 are consumed by
  flash and octal PSRAM; avoid those, the strapping pins (0, 45, 46), and
  19/20 if native USB is in use. On classic ESP32-WROOM-32, avoid GPIO6–11
  (SPI flash) and the strapping pins (0, 2, 12, 15 — GPIO12 sets flash voltage
  at boot and is the dangerous one). GPIO34–39 are input-only with no internal
  pull-up, which suits optocoupler outputs, since those need an external
  pull-up anyway.
- **Capture channel budget.** An MCPWM group provides one capture timer and
  three capture channels; the classic ESP32 and the S3 both have two groups, so
  six channels over two independent time bases. Under **D20** the start pulse
  resets a group's capture timer instead of occupying a channel, and each lane
  is bound to its own group — so a two-lane interval, trap or finish node needs
  only two channels and keeps four spare. Beams that do not measure time
  (pre-stage as a staging indicator, guard as a validity check) need no capture
  channel at all and can run on ordinary interrupts.
- **Free telemetry from the sensor:** the receiver's self-diagnosis output
  (green = stable operation) is an alignment aid and a health signal. With a
  narrow acceptance cone (D18) it stops being a convenience and becomes the
  primary alignment instrument. Wire it to a spare input.
- **Addressing:** DIP switch = bus address. The address is the node's *only*
  configuration; the meaning of "node 5, input 2" lives in a single mapping
  file on the race control PC. Rationale: swapping a dead node in the field
  means copying DIP positions — no laptop, no discovery, no config edits.
  The MCU's factory MAC is reported as a serial number for inventory/logging
  only, never used for addressing.
- **Power path:** XT60 input → blade fuse at the battery → reverse-polarity
  P-MOSFET → TVS → ideal-diode ORing (battery / trunk 12 V / optional 48 V
  input) → buck to 5 V. Battery voltage divider on ADC for telemetry; node
  reports low voltage on the bus before shutting down.
- **Service features:** per-channel alignment LED mode, raw event log to
  flash (every edge, timestamped — dispute evidence), `identify` bus command
  (blink LED), USB CLI for live logs and self-test, termination and power
  source jumpers.
- **Enclosure:** identical IP65–67 boxes for all nodes, drilled from one
  template for the maximum configuration; unused entries closed with blanking
  plugs. Trunk connections on panel-mount M12; sensor and power connections as
  permanently glanded pigtails with M12 / XT60 ends (sensor and power
  connectors are mechanically incompatible by design). Node swap = unplug,
  swap box, set DIP, plug in — minutes, no tools.
- **Environment:** breathable vent membrane against condensation cycling,
  silica gel, conformal coating on boards. Heat is the bigger enemy than
  water: light-colored boxes, shade-side mounting, on-board temperature
  telemetry. Field IP integrity is a property of the *assembly* (glands,
  torque, seals), not the box rating — dunk-test assembled empty boxes before
  trusting them.

## 7. Power

Per-node 12 V LiFePO4 packs (sealed IP67 modules with integrated BMS, 6–12 Ah
depending on position), XT60 connectors, inline fuse, physical power switch
for overnight shutdown.

- Node consumption ≈ 0.2 A @ 12 V → 10 Ah covers a 3-day / 8h-per-day event
  with margin for aging and overruns.
- The finish node powers the arbitration camera over USB (~0.3 A extra) and
  therefore gets the largest battery.
- One charged spare battery in the field kit replaces any node's supply in
  seconds.
- Central 48 V trunk power (PoE-style, isolated 48→12 buck per node) is a
  supported alternative for a permanent installation; rejected as the default
  for a portable system (single point of failure, 30+ kg of extra copper per
  deployment). The ORing input keeps both options open.

**Outer-edge emitter posts.** Through-beam (D02) puts a powered device on the
far side of each lane; with the centre trunk (D21) that means both outer edges
of the track. It is the simplest assembly in the system — battery, switch,
inline fuse, emitters — with no MCU, no bus and no configuration.

- Emitter draw is ≤ 20 mA, so a 3-day × 8 h event needs **0.5 Ah**. Capacity is
  not the constraint; voltage is (12 V means a 4S pack, not one cell).
- Use the **same 12 V LiFePO4 module type as the nodes**, smallest capacity —
  one battery type and one charger in the field kit, and the mass doubles as
  post ballast, which the far post needs anyway.
- Adjacent emitters share one post and one battery: pre-stage, stage and guard
  fall within 518 mm. One post per track position per lane — **five** for a
  single lane (start, 60 ft, 1/8, trap, finish), **ten** for two lanes, one on
  each outer edge at each position.
- A small solar panel at 20 mA removes battery swapping from the field routine
  entirely; commercial drag beam units already ship this way.

Nothing crosses the racing surface: the far side needs power, and it brings its
own.

## 8. Christmas tree

Separate module on the same bus — deliberately **not** a universal node (many
LED outputs, sequence logic; would pollute the common board).

- Sequence logic per standard practice: pre-stage / stage per lane; AutoStart
  (random delay after both cars staged, no operator button in the final
  version); standard tree (500 ms cascade) and pro tree (400 ms) modes;
  red-light detection from stage beam + tree state.
- Sequence delays must be calibrated to *include LED turn-on time* —
  uncompensated trees systematically red-light experienced drivers. Calibrate
  once with a logic analyzer or high-speed camera.
- Lamp logic builds on the open source
  [MajicDesigns/DragLights](https://github.com/MajicDesigns/DragLights)
  project (NeoPixel/FastLED).
- Daylight visibility is a real problem: high-power LED modules with hoods
  are required, which makes the tree the only module with a genuine thermal
  design question. *Unverified.*

## 9. Race control software

Runs on a laptop at the start area; the bus master.

- Owns the node mapping file (address + input → beam meaning), poll loop,
  liveness and battery/temperature dashboards.
- Race logic: staging state machine, rollout, reaction time, ET/interval
  assembly, red light, win/breakout.
- Event management: driver registration, classes, qualifying, ladder
  generation, bye runs.
- Local web scoreboard (LAN + QR code) for spectators and drivers; time slips.
- The system must be fully functional with no internet connectivity; any
  cloud features are strictly additive.

## 10. Arbitration video

A finish-line camera (e.g. an action camera at 240 fps) aimed along the
finish line, powered from the finish node battery. It is **not** a timing
source — beams are. Synchronization is optical: the finish node drives a
marker LED placed in the camera frame, flashing on beam break. Disputes are
resolved frame-by-frame against the LED. Line-scan photo-finish was evaluated
and rejected: beams already exceed the required precision (see decisions log).

## 11. Open questions / unverified assumptions

Ordered by risk. These gate the project — bench validation comes before any
production order. Procedures, expected values and pass/fail criteria for the
ones the current stage can answer are in
[`bench-validation.md`](bench-validation.md).

1. **Sensor timing jitter.** Datasheet "≤ 1 ms response" specifies delay, not
   repeatability, and no vendor in any category publishes repeatability —
   industrial, sports-timing or drag-specific. Bench test per **D15**: slotted
   disk on a motor, hundreds of cycles, **differenced against a reference
   detector on the same disk** so motor speed drift cancels. Both speed
   regimes (100 km/h edge speed and staging creep). Target: < 1 ms total path
   jitter, beam → timestamp. Decides the sensor BOM, including whether
   low-cost clones are acceptable.
2. **Edge asymmetry between make and break.** §2 starts ET when the tire
   *exits* the stage beam and stops it when the tire *breaks* the finish beam
   — opposite transitions through a sensor whose hysteresis makes the two
   thresholds deliberately unequal. This yields a **systematic ET offset that
   cancels nowhere**, unlike jitter. Calibratable *if* stable, which couples
   it to #4. The rig must report make-delay, break-delay and their difference
   as separate numbers.
3. **ESP32 capture jitter.** GPIO interrupts under a general-purpose framework
   can jitter tens of µs or worse; hardware capture (RMT/MCPWM) should reduce
   this to noise. Same bench. Note the input stage is part of this path —
   optocoupler choice matters as much as the capture peripheral (**D13**).
4. **Sensor thermal drift.** Operating spec is −25…55 °C of *ambient air*, but
   a dark body in direct sun exceeds that while the air stays in spec. The
   failure mode is drift, not death: right in the morning, systematically
   shifted by mid-afternoon, nothing visibly broken. Measure mean-delay shift
   (not just spread) between ~25 °C and ~60 °C. See **D19**.
5. **Start-pulse noise immunity** over 400 m near high-energy ignition
   systems, with 5 ms width validation. Verify with the full cable drum.
6. **How much angular rejection the start cluster needs.** *Whether* adjacent
   beams interfere is no longer open — the datasheet's parallel-shifting
   characteristic puts a neighbouring emitter at 178 mm well inside the
   receiver's ~7.5° acceptance, and BJ's interference-prevention function
   excludes through-beam types. What is open is how much of it the sensitivity
   adjuster alone removes, and therefore how deep the **D18** hood must be.
   The estimate there (~18 cm at a 10 mm aperture for 3.2°) is geometry, not
   measurement.
7. **Sunlight in the receiver axis.** The 11,000 lx ceiling is reachable when
   low sun aligns with the beam axis. Mitigated by track orientation and hood
   geometry (**D19**); open is whether the sensitivity adjuster helps at all,
   which depends on whether the limit is front-end saturation.
8. **Tree visibility in direct sunlight** and enclosure thermals.
9. **Cross-lane sensor interference** — largely answered by geometry rather
   than by sensor choice, now that the centre trunk (**D21**) puts the two
   lanes' receivers back to back facing outward, each with its back to the other
   lane's emitter. What remains to confirm on the bench is the residual: two
   receivers mounted within centimetres of each other, and whether the 10–20 cm
   longitudinal offset recommended in §2 is still needed once they face apart.
10. Bus error rate over the full-length trunk (24 h soak test, CRC error
    count; mitigation: lower baud — 10× headroom available).

## 12. Deployment configurations

| Stage | Hardware | Beams (1 lane) | Far posts | Capability |
|---|---|---|---|---|
| Minimum | start node + finish node + tree | 3 (+1 w/ guard) | 2 | RT, ET, full race |
| Standard | + 60ft node, + speed trap beam | 5–6 | 4 | 60ft, trap speed |
| Full | + 1/8 node, + arbitration cam | 7 | 5 | complete single lane |
| Two-lane | × 2 of the above | 14 | 10 | complete event |

One beam = one through-beam set = two devices on two posts. Far-side posts are
counted separately because adjacent emitters share a post and a battery (§7).

**Node allocation for a full two-lane build** — six nodes, 14 of 24 inputs used:

| Position | Nodes | Inputs each |
|---|---|---|
| Start | **2**, one per lane | 3 — pre-stage, stage, guard (1 spare) |
| 60 ft | 1, shared | 2 — one per lane |
| 1/8 | 1, shared | 2 |
| Trap | 1, shared | 2 |
| Finish | 1, shared | 2 |

Two nodes at the start rather than one because two lanes × three beams exceeds
four inputs, and splitting per lane leaves each a spare input — useful if the
fourth start beam reference systems run (Compulink's "stage lock") turns out to
be needed.

The trap gets its own node rather than a second input pair on the finish node,
even though the input count would allow it: the trap sits ~20 m upstream, and a
20 m unshielded sensor run beside high-energy ignition systems is a worse
trade than one more box. D07 and D08 make adding a node cheap — an address on a
DIP switch and a line in the mapping file.

The bus architecture makes every step additive: new positions are inserted
into the trunk with an address and a mapping-file line — no re-cabling.