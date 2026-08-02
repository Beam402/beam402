# Beam402 — Software Architecture

> Status: **design, not implemented.** No firmware and no race control code
> exist. This document says what will be built, and — more usefully — what each
> program is forbidden to do. Nothing here has run on hardware.

Software is not gated by **D15**: the gate stands on batch purchases, PCB
fabrication and public timelines. It does constrain what may be *claimed* —
every timing figure below is a design intent that **T3** has to confirm.

The wire contract between the programs (Modbus register map, mapping file
format) lives in [`protocol.md`](protocol.md). This document is the division of
labour; that one is the interface.

## 1. Three programs, and the boundary between them

| Program | Runs on | Language | Owns |
|---|---|---|---|
| **Node firmware** | ESP32-S3 | C / ESP-IDF (**D22**, *revisit*) | capturing instants, latching them, answering the bus |
| **Race control** | operator laptop | Rust (**D23**) | bus mastering, all race logic, UI, scoreboard, results |
| **Bench tooling** | developer machine | Python (§6) | reducing logic-analyzer captures to distributions |

The firmware language is settled only for the firmware that produces the **T3**
number, so that the project's first measurement of its own electronics has as
few unknowns in it as possible. A Rust node is admissible the moment it
reproduces that number on the same rig — **D22** carries the evidence and the
bar, including why the usual "the HAL doesn't support capture" argument does not
hold up.

The boundary that matters is between the first two, and it is deliberately
lopsided: **the node reports ticks, the master assigns meaning.** A node does
not know what an ET is, which lane it serves, or what a 60-foot split means.

That is not minimalism for its own sake. It is what keeps **D07** (one firmware
for every position) and **D08** (identity bound to track position, not silicon)
true once **D20** exists — see §2.

## 2. The node has no role

**D20** gives downstream nodes a per-lane MCPWM group binding and gives one
start-area node the job of capturing both start pulses on a single timer. Read
carelessly, that is position-specific behaviour — and **D08** says the DIP
address is a node's *only* configuration. Something has to give.

Nothing does, under one rule (**D24**): every node captures everything it can,
always, and publishes all of it. Concretely, every node — start, 60 ft, trap,
finish, spare on the shelf — runs the same loop:

- both edges of every populated input, on both lanes' capture groups;
- both lanes' start pulses, observed on a common timer, with their measured
  widths and their difference;
- telemetry, faults, live input state.

The master reads the registers it cares about for that address and ignores the
rest. "Which node's pulse difference is the launch margin" is a line in the
mapping file, not a mode in flash. Consequences worth stating:

- Firmware contains no `if (position == START)`. There is no start-node build.
- A spare node is a spare for any position with no reflash — **D11**'s promise
  extended to software.
- Adding the trap node (§12) costs one mapping-file line, as **D07** claims.
- Registers that are meaningless at a given position read as "not seen this
  run", not as an error. A finish node with no stage beam is not misconfigured;
  it simply never observes one.

The cost is a handful of registers per node that nobody reads. At two bytes
each, that is the cheapest thing in the system.

## 3. Node firmware

### The timing path, and what may not enter it

Everything in this subsection exists to keep firmware **out** of the interval
being measured. Per **D13** and **D16**:

- The 5 ms start pulse is produced by a monostable from the optocoupler output.
  Firmware observes it. Firmware never generates it.
- The pulse resets the capture timer through the MCPWM GPIO sync input
  (**D20**). Verified to exist as an API —
  `mcpwm_capture_timer_set_phase_on_sync()` with a GPIO sync source — but its
  latency and jitter are **T3**'s job, not the reference manual's word.
- Beam edges land in capture channel registers. No GPIO ISR is in the
  measurement path. A capture channel takes both edges (`pos_edge` and
  `neg_edge` together, edge reported in the event data), so one beam costs one
  channel for both its break and its make — which is what makes D20's
  "two channels of six" budget correct.
- Radios are disabled in build configuration, not at runtime.
- The Modbus task runs at a priority below capture handling and may be starved
  without consequence: results are latched, and a late poll loses nothing (§4).

### Width validation must not delay the counter

**D16**'s trap, restated because it is the one bug in this design that would
pass every test that does not look for it: the counter starts on the pulse's
**leading edge**. Width validation completes 5 ms later and, if the width is
wrong, sets an invalidation flag on a run that has already been timing. Waiting
for the full pulse before starting adds exactly 5 ms to every measurement and
looks perfectly normal in isolation.

The node therefore publishes the *measured* width alongside the flag, so the
master can see a pulse drifting toward the rejection threshold before it starts
throwing runs away — §11 #5 is about noise on 400 m of cable, and a margin
trending down is the early warning.

### What else the node does

- **Boot:** read DIP (address), report factory MAC as a serial number, publish
  capability bits (populated inputs, capture channels).
- **Telemetry:** battery millivolts, interior temperature, sensor bracket
  temperatures (**D19** requires the bracket, not the air), receiver
  self-diagnosis lines (§6), fault flags.
- **Raw edge log:** every edge, timestamped, to flash — dispute evidence per
  §6. On a coarse millisecond clock, deliberately: **D20** notes run timing
  needs no 64-bit accumulation, and evidence does not need 12.5 ns. Pulled
  after a round, never in the live poll loop.

  **Never written during a run.** Flash operations on this part run with
  interrupts disabled — `esp-storage` puts them in a critical section by
  default, and ESP-IDF's writes disable the cache — so a log flush mid-run can
  stall the very path being measured. Buffer edges in RAM, flush between rounds.
  The constraint is the silicon's, not the language's.

- **Self-verification:** on command, drive the injection GPIO with a known
  interval into the node's own input path and capture it. §3 asks the system to
  prove itself before a round for the cost of one GPIO; this is that GPIO.
- **Service:** per-channel alignment mode, `identify` blink, USB CLI.

### The seam that makes host testing possible

The register layer takes capture events *as data* — input index, tick count,
edge direction — and returns the register image. On the device those events come
from MCPWM; on a host they are constructed. So the register image is a pure
function of (events, config), and the part that cannot be tested without silicon
shrinks to the capture wiring itself — which is exactly what **T3** measures.
§7 turns that into a build order.

## 4. Race control

### Layers

```
serial ─▶ Modbus RTU transport ─▶ poller ─▶ event stream
                                                │
                              mapping file ─────┤
                                                ▼
                                          race logic  (pure)
                                                │
                        ┌───────────────────────┼──────────────┐
                        ▼                       ▼              ▼
                 results store (SQLite)    operator UI     scoreboard
```

**The race logic is a pure function of the event stream and the mapping.** No
serial handles, no clock reads, no file I/O below that line. Two payoffs, both
practical rather than aesthetic:

1. All race-logic work proceeds today, against synthetic runs, with no
   hardware — which is the whole reason this document exists now.
2. A recorded bus session replays deterministically. For a project whose claim
   is a trustworthy number, "here is the session, replay it and get the same
   ET" is the software half of D01's verifiability argument.

### Poll for change, read on change

The arithmetic is unforgiving: 19,200 bps 8N1 is 1,920 characters per second,
so **100 ms buys about 192 characters for the entire bus.** Every figure below
prices the *whole* exchange — request, response, and the 3.5-character silence
Modbus RTU requires before each frame (~1.8 ms), because on a half-duplex trunk
that gap is bus time like any other.

A full two-lane run record is ~28 registers — a 69-character exchange, ~40 ms
for one node. Seven devices of that is over half a second. So the steady-state
loop cannot fetch records:

| Traffic | Size | When |
|---|---|---|
| Digest — run generations, faults, live input state | 4 registers, 24.5 chars, ~12.8 ms/device | every cycle (~89 ms for 7 devices) |
| Full run record | ~28 registers, ~40 ms/device | only when that lane's generation changes |
| Telemetry — battery, temperatures | ~6 registers | one device per cycle, round-robin |
| Raw edge log | pages | after a round, on request |

An earlier revision of this table priced the digest at ~7 ms/device by counting
only the response frame. The corrected figure is ~1.8× that, and it is now
asserted against the poller rather than restated here — see
`a_digest_sweep_costs_what_the_arithmetic_says` in `software/crates/poller`.

This costs nothing, because records are latched and there is nowhere to be
late to: §3's quiet window stops polling from "both staged" until every node
has reported, a run lasts 10–20 s, and the next pair stages for minutes. The
unhurried moment to read results is exactly the moment after they exist.

The digest cycle therefore sizes only two things: liveness detection and
staging-lamp response. On staging: the beams are wired to the start node, the
lamps hang on the tree module, so a lamp change costs two poll hops — a full
cycle to see the beam and the top of the next to write the lamp, ~210 ms on a
seven-device bus. A driver creeping at 0.1 m/s covers ~2 cm in that time.

That is accepted for now with the number written down, rather than by wiring
staging beams to the tree and breaking §2's "beams land on nodes". It is not
accepted comfortably: 2 cm is visible to a driver inching onto the stage beam,
and §8 #9 carries the way out.

### Formats

Three shapes, and the rules that separate them live in one module because
**D23**'s promise is that a club changes a class rule without seeing a compiler:

| Format | Start | Breakout | Used for |
|---|---|---|---|
| Heads-up | together | none | grudge, heads-up classes, qualifying |
| Bracket | handicap from the two dial-ins | own dial | most club racing |
| Index | together | the class number | Super Comp and relatives |

Fouls are resolved **first or worst**: a driver who fouls loses; if both foul,
a red light outranks a breakout, two breakouts are separated by which ran
further under the dial, and two red lights by which car left first. That last
one is a question about the clock rather than about the two numbers, because
under a handicap the *smaller* red light can easily be the earlier one — the
tree answers it, since both greens and both pulses are its own registers.

### Race logic

- **Staging state machine:** idle → pre-staged → staged (both lanes) → armed →
  tree sequence → launched → running → complete or foul. Deep staging and
  guard-beam rejection (§2: stage and guard broken together is bodywork, not a
  tire) resolve here, from the start node's input state.
- **ET assembly.** ET's zero is the launch instant, and the launch instant *is*
  the pulse — hardware-derived from the tire leaving the stage beam (**D16**),
  which under **D17** is a rising edge at the node. So ET is not assembled from
  two clocks: it is the finish node's own capture register, read directly.
- **Splits** — 60 ft, 1/8, trap entry and exit — likewise, each a single
  register from the node that owns that beam, each measured against that node's
  own timer, zeroed by the same pulse. **D04** in one sentence.
- **Trap speed** = measured base ÷ (trap exit − trap entry), both on one node
  and one timer. The base comes from the mapping file, laser-measured (§2: 5 cm
  of error is 0.25 % of speed, which dwarfs the electronics).
- **Margin** — who won — is `(pulse₂ − pulse₁) + ET₂ − ET₁` per **D20**, with
  the first term read from whichever node the mapping file names as the margin
  source. Crossing order decides races, and ET alone cannot recover it.
- **Corrections** applied by the master, never by the node: per-MAC crystal ppm
  (**D13** — "passport, not job"), and temperature correction if **T4** finds a
  drift that is stable enough to calibrate (**D19**).
- **Event management:** registration, classes, qualifying, ladders, bye runs,
  time slips.

### What ships as data, not code

**D23** buys a single static binary at the cost of a narrower contributor pool
than Python would have. That cost is paid down by keeping everything a club
would plausibly want to change out of the language entirely:

- class and bracket rules, dial-ins, breakout handling — configuration;
- tree modes and delays — configuration, pushed to the tree at arm time;
- scoreboard and time-slip layout — templates and CSS;
- the mapping file — the only source of truth for track meaning (**D08**).

A club changing a class rule or a slip layout should never see a compiler. If
that stops being true, D23 is the decision to revisit.

### Storage and offline

One SQLite file per event, plus the raw bus session log beside it for replay
and disputes. No network dependency anywhere in the path from beam to time
slip: the scoreboard is served from the same process on the LAN, reachable by
QR code, and cloud features remain strictly additive.

## 5. Reaction time and red light belong to the tree

The tree module is a bus device like any other (**D07** keeps it off the
universal board), and it owns something no other device does: **the instant the
green lit.**

Reaction time is the interval between green and launch. Assembling it from the
tree's green and the start node's pulse means subtracting two clocks — exactly
what **D04** forbids. But the tree sits in the start area on the trunk, so it
sees both start pulse pairs, and under **D24** it observes them like everyone
else. So:

```
reaction_time = t_pulse − t_green        both on the tree's own clock
red light     = reaction_time < 0        the driver left before green
```

One clock, no bus latency in the number, and a foul is not a special case —
it is a negative RT.

Two consequences that are easy to miss:

- **The green instant must be captured in hardware too**, from the lamp driver
  output looped back into a capture input — not taken at the moment firmware
  calls the LED write. Otherwise firmware latency lands in a number handed to
  the driver, which is D16's mistake in a different device. §8 already requires
  calibrating sequence delays to *include* LED turn-on time; this measures it
  instead of trusting the calibration.
- The master arms the sequence; the tree runs it. AutoStart's random delay
  lives in the tree with bounds pushed at arm time — volatile per-round
  settings, not flash configuration, so **D08**'s rule survives.

### One cascade per lane (D28)

Bracket racing holds the quicker car's cascade back by the difference between
the two dial-ins, so the tree runs **two independent sequences** and there are
two green instants rather than one. The handicap is written with
`tree_handicap` before `tree_arm`, which latches it, and the tree echoes the
armed value so the master can verify it before a car stages.

What is worth stating is how little of the timing model this touches, because
that is the part that says the model was right:

| | Heads-up | Handicap |
|---|---|---|
| ET's zero | that car's launch pulse | unchanged — the four seconds spent waiting are not in it |
| Reaction time | `t_pulse − t_green` | unchanged — against *that lane's* green |
| Launch margin (**D20**) | `pulse₂ − pulse₁` | unchanged — the handicap *is* part of that difference |
| Lamps | shared column | per lane, because the lanes are genuinely in different places |

The winner is whoever crosses the stripe first, which the margin gives directly:
`(pulse₂ − pulse₁) + ET₂ − ET₁`. In a bracket the quicker ET usually belongs to
the car that lost, and a system that recorded only ET would confidently print
the wrong name.

## 6. Bench tooling

`bench-validation.md` §5 requires a data-reduction script, and `BOM.md` is
blunt about the deadline: it has to exist **before the first serious
measurement.** Given: a CSV export from PulseView, it reports pass count, mean,
σ, peak-to-peak and 99th percentile of Δt, split by edge direction, with the
run's speed and body temperature — the same numbers for every run so different
days and different sensors compare.

This is Python, not Rust, and deliberately not an ADR: nothing downstream
depends on it, it never leaves a developer's machine, and it will be rewritten
against the first real capture that surprises us. Standard library only —
`csv`, `statistics` — so it runs on any laptop at the bench.

It is also the one piece of software on the critical path right now. Everything
else here waits on DevKits; this waits on nothing, and **T1** cannot be
believed without it.

## 7. Build order

Firmware splits into three tiers by what is capable of testing it, and the split
decides the order:

| Tier | What | Tested by |
|---|---|---|
| **1 — pure core** | register image, run-record snapshot, generation semantics, wrap accumulation, DIP decode, flag words, width-validation *logic*, log paging | an ordinary host compiler. No ESP-IDF dependency, so there is nothing to mock and no mocking framework needed |
| **2 — orchestration** | FreeRTOS tasks, Modbus UART transport, response timing | CMock, or the ESP-IDF Linux target, where `driver` and `esp_hw_support` are mocked |
| **3 — silicon** | MCPWM capture, GPIO-sync latency, ISR timing, the 5 ms width in real time | **T3**, and nothing else |

**No emulator covers tier 3.** Wokwi's feature table lists MCPWM as not
implemented and its RMT as transmit-only; Espressif's QEMU does not list MCPWM
among emulated peripherals. Neither claims cycle accuracy. A green test suite on
an emulator before T3 would be the worst available outcome, because it would
look like evidence.

The rule that follows: push everything that fits into tier 1, because the size
of tier 3 *is* the project's risk surface.

Order, chosen to need no hardware until tier 3:

1. **Bench reduction script**, with synthetic captures as its tests. Blocks T1.
2. **The register map**, as the `beam402-protocol` crate — see **D27** and
   [`protocol.md`](protocol.md) §0. *Done.* `no_std` and dependency-free so both
   halves share it verbatim; `registers.toml` is generated from it and §3 is
   checked against it. Layout was the cheap part: what it mainly buys is that
   **D25**'s generation rule, **D16**'s `valid`/`invalidated` pair, **D17**'s
   polarity and "never observed ≠ zero" are enforced rather than remembered.
3. **Tier 1 core and its host tests**, driven by synthetic capture events.
   Deliberately language-independent in shape — that is what keeps **D22** cheap
   to reverse.
4. **Node simulator**: a Modbus RTU slave replaying scripted runs, including the
   ugly ones — invalid pulse width, a node rebooting mid-run, a silent node, a
   beam that breaks and never makes again, two cars leaving 3 ms apart.
5. **Race logic** against the simulator: staging, ET, splits, margin, fouls.
6. **Scoreboard and time slips** from recorded sessions.
7. **T3 harness** when the DevKits land — capture, sync, marker output, nothing
   else. The first real number this project produces about its own electronics.
8. **Tier 2, then the node firmware proper**, then the parking-lot demo.

Step 3 does not wait for steps 4–6; it is the firmware half and runs in
parallel. Step 4 is the load-bearing one for race control: a simulator that
replays only clean runs validates nothing, and the failures listed there are
the specification.

**The order actually being worked, and what it costs.** Steps 3, 7 and 8 are
deferred: no firmware is being written until there is silicon to write it
against. That leaves the simulator as the only executable model of node
semantics — latching, generation wrap, snapshot atomicity, the **D16** rule —
so it is built as a *node model* with its own API rather than as a test double.
Under **D27**'s shared crate that model is tier 1: the same code compiled for
the host in tests and for xtensa in firmware, which returns most of step 3 as a
by-product rather than losing it.

Two consequences worth stating rather than discovering. A simulator speaking
decoded blocks rather than RTU frames tests nothing about framing — §8 #5 is a
measurement against real adapters, and no amount of PTY work substitutes. And a
node model that is *also* the firmware's tier 1 must be held to that standard
from the first commit: it is the reference the silicon will be compared against,
not a convenience for the master's tests.

## 8. Open questions (software)

Ranked, in the spirit of `architecture.md` §11, with the test that settles
each:

1. **MCPWM GPIO-sync latency and jitter.** The mechanism under every number in
   this document. **T3**; **D20** carries the fallback if it jitters.
2. **Run-record atomicity.** A capture can land while the Modbus task is
   assembling a response. The record must be snapshotted whole, or the master
   will occasionally read a split from one run with a generation from the next.
   Settled by construction plus a deliberate test: capture at maximum rate
   while polling continuously, and assert every record read is self-consistent.
3. **Digest poll cycle on the real trunk.** ~89 ms for 7 devices is
   arithmetic; at 450 m with retries it is a measurement. Same soak test as
   §11 #10. The arithmetic also ignores what a *silent* node costs, and that
   term dominates: one dead device burns the response timeout once per attempt,
   300 ms at the current `retries = 2`, more than three times the whole healthy
   sweep. Whether that is tolerable or the timeout needs shortening once a
   device is known to be dead is a measurement, not a preference.
4. **Tree reaction-time path.** Requires both pulse pairs and the looped-back
   green at the tree, and a hardware capture on each. Verifiable on the bench
   with a logic analyzer before any tree exists at a track.
5. **USB-RS485 adapter framing.** Modbus RTU needs 3.5-character inter-frame
   silence (~1.8 ms at 19,200). Cheap adapters with large latency timers break
   framing in ways that look like bus noise. Measure before blaming the cable.
6. **Whether the capture interrupt actually fires.** Binding a handler compiles
   and links (**D22**, B1), and `INT_ST` reports which channel fired — but
   nothing has run. This decides how a second edge arriving before the first is
   read gets detected, which is what sets `run_flags.overflow` honestly rather
   than by assumption. Same silicon session as **T3**.
7. **Which `(input, lane)` pairs a node can capture, and how the master learns
   it.** §2 says every node captures every populated input "on both lanes'
   capture groups". Read literally that is four inputs × two groups = **eight**
   capture channels; `architecture.md` §6 has two MCPWM groups of three, so
   **six** exist. Some allocation policy therefore has to live in firmware, and
   the protocol does not publish it: `capture_channels` (0x0018) reports a
   count, not a map, while the register layout offers four input slots in
   *each* lane's run record — so it can express combinations the silicon cannot
   produce. The cost lands in [`protocol.md`](protocol.md) §5, where load-time
   validation cannot check that a mapped `(address, input, lane)` is
   capturable at all. A lane typo then
   reads "not seen this run" — which is data, not an error — and the run quietly
   loses a split instead of failing to load. That is the failure class this
   project refuses, arriving through the cheapest route again. Settled by
   publishing the allocation as a per-lane input bitmap in the identity block's
   reserved space — additive, so `protocol_version` does not move — plus a tenth
   validation rule. The allocation *rule* is a firmware decision and waits on
   firmware; the register to publish it in does not.
   One consequence is already load-bearing: because the node cannot tell which
   input belongs to which lane, `run_flags.complete` and
   `status_flags.run_active` never set on a node shared between lanes, so
   neither can serve as "the record is ready". **D25**'s amendment routes around
   that — the run generation now moves on every capture — so this gap no longer
   blocks results. It still leaves two flags in the map that are unusable at
   most positions, which is its own reason to close it.
8. ~~**What the master does with a partial set of run records.**~~ **Settled,
   in code.** The rule: an ET that is present and timing-valid **is** a run.
   Intermediate splits that produced nothing print as "—" with a named reason
   attached — node silent, node restarted, run disowned, beam not broken — and
   are never dropped silently and never invented. No ET is **no time**, whatever
   else the round produced.
   Refusing the whole round for a missing 60 ft split would throw away the
   number the driver came for because of a beam that does not decide anything;
   printing a zero would be a lie with a plausible shape. Naming the absence is
   the only option that leaves the slip both complete and honest.
   Implemented as `Missing` and `Gap` in `software/crates/race`, with
   `a_missing_split_names_its_reason_and_the_slip_is_still_issued` as the test.
9. **Nothing in the digest says the tree moved.** The tree block carries a
   `sequence_gen` and [`protocol.md`](protocol.md) schedules it "on generation
   change", but the four-register digest has no bit that carries it — so the
   cheap read a master does every cycle cannot tell it the green lit. The poller
   works around this by reading the tree's 13 registers outright from the arm
   onward, which costs ~9 ms on one device and only during a round. Two ways to
   settle it: spend a reserved `status_flags` bit on "tree sequence advanced",
   or accept the workaround and say so in the contract. The first is additive
   and does not move `protocol_version`.
10. **Whether every device deserves a digest every cycle.** The staging lamps
   are the only thing the cycle time actually gates, and they depend on the
   *start* nodes alone: a beam read from the start node in one cycle becomes a
   lamp written to the tree at the top of the next, ~210 ms on a seven-device
   bus, ~2 cm of creep. Polling the start nodes and the tree every cycle and the
   downstream nodes every third would cut that to ~40 ms while leaving liveness
   detection at three cycles — around a second, which is far inside what an
   operator notices. It is a tiering rule, not a protocol change, but it makes
   the cycle non-uniform and liveness detection position-dependent, so it is
   written down as a choice rather than taken quietly.
