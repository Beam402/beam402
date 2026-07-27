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

**Type:** polarized retroreflective photoelectric sensors (industrial, e.g.
Autonics BX/BEN series or equivalent). Emitter/receiver in one housing on one
side of the lane; a passive prism reflector on the other side. No power or
electronics across the track.

Requirements:

| Parameter | Requirement | Why |
|---|---|---|
| Response time | ≤ 1 ms | timing resolution target |
| Sensing type | polarized retroreflective | polarization rejects false triggers from glossy bodywork; only the prism reflector returns rotated light |
| Range | ≥ lane width + margin (5 m+) | typical lane 4–5 m |
| Output | NPN, NO | directly readable via optocoupler |
| Rating | IP67, 12–24 V DC | outdoor use |
| Reflector | manufacturer prism reflector (not tape) | rated range is specified with it; reflective tape roughly halves range |

Cheap hobby IR pairs (Arduino-style modules) are explicitly excluded: response
times of 10–50 ms with high jitter, and they are blinded by sunlight.
Industrial sensors use modulated light and are immune.

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

Timestamps are captured in hardware (ESP32 RMT/MCPWM capture or
high-priority GPIO interrupt with radios disabled), never by polling in the
main loop. *Capture jitter is unverified — see open questions.*

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
the budget-indestructible option), ~450 m, run along one side of the track
(the receiver side). Pair assignment:

| Pair | Function |
|---|---|
| 1 | Start pulse (differential, RS-485 transceiver driven) |
| 2 | Data bus (RS-485, Modbus RTU) |
| 3 | 12 V power for nodes near the source (start area) |
| 4 | Spare / second-lane start pulse |

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
production order.

1. **Sensor timing jitter.** Datasheet "≤1 ms response" specifies delay, not
   repeatability. Bench test: slotted disk on a motor breaking the beam at
   stable RPM, hundreds of cycles, measure timestamp spread. Target: < 1 ms
   total path jitter (beam → timestamp). This test decides the sensor BOM,
   including whether low-cost industrial clones are acceptable.
2. **ESP32 capture jitter.** GPIO interrupts under the Arduino framework can
   jitter tens of µs or worse; hardware capture (RMT/MCPWM) should reduce
   this to noise. Verify on the same bench.
3. **Start-pulse noise immunity** over 400 m near high-energy ignition
   systems, with 5 ms width validation. Verify with the full cable drum.
4. **Tree visibility in direct sunlight** and enclosure thermals.
5. **Cross-lane sensor interference** with the chosen sensor model.
6. Bus error rate over the full-length trunk (24 h soak test, CRC error
   count; mitigation: lower baud — 10× headroom available).

## 12. Deployment configurations

| Stage | Hardware | Capability |
|---|---|---|
| Minimum | start node + finish node + tree, 1 lane | RT, ET, full race |
| Standard | + 60ft node, + speed trap beam | 60ft, trap speed |
| Full | × 2 lanes, + 1/8 node, + arbitration cam | complete event |

The bus architecture makes every step additive: new positions are inserted
into the trunk with an address and a mapping-file line — no re-cabling.