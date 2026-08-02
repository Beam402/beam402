# Beam402 — Bus Protocol and Mapping File

> Status: **design, not implemented.** Nothing speaks this protocol yet. The
> register map is a proposal to be reviewed *before* either side is written —
> it is the one artifact both the firmware and the race control software must
> agree on, and the most expensive thing in the project to change later.

Scope: the wire contract between the race control PC (the only bus master) and
every device on the trunk, plus the format of the mapping file that gives the
numbers their meaning. Division of labour and the reasoning behind it are in
[`software.md`](software.md); the physical bus is `architecture.md` §4.

Not covered here: the scoreboard's HTTP interface and the node USB CLI. Neither
is a contract between independently-built halves, so neither is versioned.

## 0. Source of truth

**The register map is the `beam402-protocol` crate
(`software/crates/protocol`).** [`registers.toml`](registers.toml) is printed
from it and §3's tables are checked against it, so an offset or a flag bit
exists in exactly one place.

This is not tidiness. A map maintained by hand in a document and transcribed
into two codebases drifts, the drift is silent, and what it produces is a *valid
number read from the wrong register* — the exact failure class this project
cannot tolerate, arriving by the cheapest possible route.

The map moved into code, rather than staying a neutral file with two generators,
under a **working assumption that both halves are Rust** — a `no_std`
dependency-free crate shared verbatim by race control and node firmware. That
assumption is not a reversal of **D22**, which stands at *revisit* with its bar
written as a measurement: a Rust node is admissible once it reproduces the
**T3** number on the same rig. It is stated here because the arrangement below
depends on it, and because the fallback is cheap rather than hypothetical — the
same walk over the map emits a C header, so a node that stays on C costs an
emitter, not a redesign.

Edits therefore go to the crate first, and two commands hold the documents to
it:

- `render-map check registers.toml` — regenerating changes nothing. This file
  is generated in full; a hand edit to it does not survive.
- `render-map check-tables protocol.md` — every address, width and flag bit in
  §3 exists in the crate, saying the same thing.

§3 is **not** regenerated wholesale, and deliberately: its paragraphs explain
why a register reads the way it does, with cross-references that no doc comment
would carry as well. What drifts is the numbers, so the numbers are what is
checked. If a document and the crate ever disagree, **the crate wins**.

## 1. Link layer

Per **D05**: half-duplex RS-485 multi-drop, Modbus RTU, master–slave polling.
Nodes never transmit unpolled.

| Parameter | Value | Note |
|---|---|---|
| Baud | 19,200 | §4 keeps ~10× headroom; raising it is the documented escape for bus errors (§11 #10) |
| Framing | 8N1 | see below |
| Addresses | 1–63 | 6-position DIP (**D08**); address 0 read = fault, not broadcast |
| Function codes | FC3 read, FC6/FC16 write | holding registers only |
| Inter-frame silence | 3.5 chars ≈ 1.8 ms | see the adapter warning below |
| Response timeout | 100 ms | covers the largest transaction (~40 ms) plus turnaround |
| Retries | 2, then mark silent | silent nodes surface on the operator panel (§4) |

**8N1 rather than Modbus's traditional 8E1.** CRC16 is the integrity mechanism
and it covers the whole frame; parity duplicates that weakly on a per-character
basis. 8N1 is also what cheap USB-RS485 adapters handle most predictably. This
is a one-line change if the field ever argues otherwise.

**Holding registers only, no coils and no input registers.** Splitting
read-only telemetry into input registers is the purist layout and it buys a
second address space to get wrong, in two codebases, for no gain. One space,
one function code for reads. In D05's spirit: less of our own protocol.

**No broadcast in v1.** Modbus address 0 suppresses the response, so a
broadcast command cannot be acknowledged. Every command is addressed and every
command is confirmed by reading back its sequence number (§4).

**Adapter warning.** RTU framing depends on that 1.8 ms of silence. USB-serial
adapters with a large latency timer coalesce characters and break framing in a
way that looks exactly like bus noise. Set the adapter latency to 1 ms and
prefer a known-good chipset; measure before blaming 450 m of copper.

## 2. Conventions

- **Word order: high register first.** A `u32` at address *A* has its high 16
  bits at *A* and its low 16 at *A*+1. Stated because getting it wrong is a
  whole class of bug that reproduces only above 65,535 ticks.
- Signed values are two's complement.
- **Ticks** are capture-clock counts — 80 MHz, 12.5 ns — and wrap at ~53.7 s
  (**D20**). Nodes report ticks. Converting to seconds, applying the per-board
  crystal correction and dividing distances is the master's job, always.
- Temperatures are 0.1 °C signed. Voltages are millivolts.
- Reserved registers read as 0. A master must **ignore** them, never validate
  them — that is what makes additive protocol changes free.
- Unimplemented addresses return exception 02 (illegal data address).
- **A lane's run record must be read in a single FC3 transaction.** The node
  snapshots it whole; splitting the read across two transactions can pair a
  split from one run with a generation from the next.

### Protocol versioning

`protocol_version` is 1. Adding registers in reserved space does not change it.
Moving, resizing or repurposing a register does. A master that reads a version
it does not know refuses to use the node for timing and says so — it does not
guess.

## 3. Register map

Blocks are laid out so the cheapest and most frequent read is also the first.

### 0x0000 — Digest (read every poll cycle)

Four registers, a 13-character exchange. Everything the master needs to decide
whether anything happened.

| Addr | Type | Name |
|---|---|---|
| 0x0000 | u16 | `run_gen_l1` |
| 0x0001 | u16 | `run_gen_l2` |
| 0x0002 | u16 | `status_flags` |
| 0x0003 | u16 | `input_state` |

`input_state` bit *N* = input *N* line active = **beam intact**. Under **D17**
(PNP, Light ON) an active line means nothing is happening; a zero means a beam
is broken or a cable is cut. Both faults are loud, which is the entire point of
that decision.

`status_flags`:

| Bit | Meaning |
|---|---|
| 0 | `run_active` — capture timer synced, run in progress |
| 1 | `run_complete_l1` |
| 2 | `run_complete_l2` |
| 3 | `fault_present` — read 0x0023 |
| 4 | `pulse_invalid_l1` — width validation failed this run |
| 5 | `pulse_invalid_l2` |
| 6 | `self_test_ready` |
| 7 | `log_wrapped` |
| 8 | `battery_low` |
| 9 | `temp_warning` |
| 10 | `alignment_mode_active` |
| 11–15 | reserved |

**Run generation semantics.** 0 means "no run since boot". It increments on
**every change to that lane's latched record** — the capture-timer sync that
starts a run, and every capture that lands in it — and on wrap goes 65535 → 1,
**skipping 0**, so a wrap can never be mistaken for a reboot. The master
compares for *inequality*, never for greater-than.

Two consequences follow from "every change" rather than "every sync", and both
are load-bearing (**D25**):

- A master that reads on generation change gets the **filled-in** record, not
  the empty one the sync produced. The pulse arrives at the launch and the
  beams are crossed seconds later; without this there is nothing in the digest
  to say the numbers arrived, and `run_complete` cannot say it either on a node
  shared between lanes ([`software.md`](software.md) §8 #7).
- The record carries its own `run_gen` at +0x00, so a read is **self-checking**:
  if it comes back older than the digest said, more landed while it was in
  flight and the master reads again.

It never leaves 0 on an edge. An edge captured before the first pulse, or after
a reboot, is recorded with the timer free-running and stays at generation 0 —
which is the master's whole defence against reading a number as a split. Only a
capture-timer sync lifts a lane out of 0.

### 0x0010 — Identity (static after boot)

| Addr | Type | Name |
|---|---|---|
| 0x0010 | u16 | `protocol_version` |
| 0x0011 | u16 | `firmware_version` — major << 8 \| minor |
| 0x0012 | u16 | `device_class` — 1 = timing node, 2 = tree module |
| 0x0013 | u16 | `dip_address` — as read at boot |
| 0x0014–0x0016 | u48 | factory MAC, high word first |
| 0x0017 | u16 | `input_present` — bitmap of populated inputs |
| 0x0018 | u16 | `capture_channels` |
| 0x0019 | u32 | `tick_hz` |
| 0x001B | u16 | `log_capacity_runs` |

The MAC is a serial number for inventory, per-board fault history and the
crystal-correction key (**D08**, **D13**). It is never an address.

### 0x0020 — Status and counters

| Addr | Type | Name |
|---|---|---|
| 0x0020 | u32 | `uptime_s` |
| 0x0022 | u16 | `boot_count` |
| 0x0023 | u16 | `fault_flags` |
| 0x0024 | u16 | `bus_frame_errors` |
| 0x0025 | u16 | `bus_crc_errors` |
| 0x0026 | u16 | `command_seq_echo` |
| 0x0027 | u16 | `command_status` — 0 = none since boot, 1 = accepted, 2 = rejected |
| 0x0028 | u16 | `sensor_health` — receiver self-diagnosis bitmap |

`fault_flags`:

| Bit | Meaning |
|---|---|
| 0 | `dip_invalid` — address 0 read from the switch |
| 1 | `sensor_health_lost` — a receiver's stability output dropped |
| 2 | `temp_sensor_missing` |
| 3 | `battery_critical` |
| 4 | `capture_config_failed` |
| 5 | `log_flash_error` |
| 6 | `self_test_failed` |
| 7 | `unexpected_reset` |

`sensor_health` is the free telemetry of §6: the receiver's green stable-
operation output, which under **D18**'s narrow acceptance cone becomes the
primary alignment instrument rather than a convenience.

### 0x0030 — Telemetry (slow rotation, one device per cycle)

| Addr | Type | Name |
|---|---|---|
| 0x0030 | u16 | `battery_mv` |
| 0x0031 | i16 | `temp_interior` |
| 0x0032–0x0035 | i16 × 4 | `temp_bracket[0..3]` |

Bracket temperature is mandatory, not decorative: **D19** requires the
temperature of the sensor *body*, because without it you cannot distinguish a
hot day from a lying sensor, and you cannot apply a correction even after
measuring one.

### 0x0040 — Pulse observation

Present on **every** device, per **D24**. Both lanes' pulses are observed on one
common timer, which is what makes their difference meaningful.

| Addr | Type | Name |
|---|---|---|
| 0x0040 | u16 | `pulse_flags` |
| 0x0041 | u16 | `pulse_gen_l1` |
| 0x0042 | u16 | `pulse_gen_l2` |
| 0x0043 | u16 | `pulse_width_l1_us` |
| 0x0044 | u16 | `pulse_width_l2_us` |
| 0x0045 | i32 | `launch_margin_ticks` — t(pulse₂) − t(pulse₁) |
| 0x0047 | u32 | `t_pulse_l1` — raw, for audit |
| 0x0049 | u32 | `t_pulse_l2` — raw, for audit |

| Bit | `pulse_flags` |
|---|---|
| 0, 1 | `seen_l1`, `seen_l2` |
| 2, 3 | `width_valid_l1`, `width_valid_l2` |
| 4 | `margin_valid` — both pulses seen this run on the same timer |
| 5, 6 | `width_marginal_l1`, `width_marginal_l2` — within 20 % of the rejection threshold |

The measured widths and the two `marginal` bits are the early warning for
§11 #5: ignition noise on 400 m of cable degrades the margin before it starts
rejecting pulses outright. A width trending toward the threshold is visible to
the operator a round before it costs anybody a run.

`launch_margin_ticks` is computed on the node because both terms come from one
timer — the same class of arithmetic as reporting a split, and **D20** already
specifies the node reporting the launch difference. Which node's value counts
is a mapping-file line (§5), not a mode in flash.

### 0x0050 / 0x0080 — Run records, lane 1 and lane 2

28 registers each, stride 0x30. Read in one transaction (§2).

| Offset | Type | Name |
|---|---|---|
| +0x00 | u16 | `run_gen` |
| +0x01 | u16 | `run_flags` |
| +0x02 | u16 | `input_mask` — inputs that contributed |
| +0x03 | u16 | reserved |
| +0x04 + 6*i* | u16 | `edge_count[i]` |
| +0x05 + 6*i* | u16 | `edge_flags[i]` |
| +0x06 + 6*i* | u32 | `t_break[i]` — first break, ticks from pulse |
| +0x08 + 6*i* | u32 | `t_make[i]` — first make, ticks from pulse |

for *i* = 0..3.

| Bit | `run_flags` |
|---|---|
| 0 | `valid` — counter started from a width-valid pulse |
| 1 | `invalidated` — width proved wrong *after* the counter started (**D16**) |
| 2 | `timer_wrapped` — run exceeded 53.7 s |
| 3 | `overflow` — more edges than the record holds |
| 4 | `complete` — every populated input reported a break |
| 5 | `synthetic` — produced by self-test injection, not by a beam |

| Bit | `edge_flags` |
|---|---|
| 0 | `break_valid` |
| 1 | `make_valid` |
| 2 | `multi_edge` — more than one break seen |
| 3–15 | reserved |

**Both edges of every beam, always.** One capture channel takes `pos_edge` and
`neg_edge` together, so this costs nothing extra — and it is required twice
over: §2 starts ET when the tire *exits* the stage beam and stops it when the
tire *breaks* the finish beam, and **T2** needs make and break as separate
numbers to measure the asymmetry between them.

`synthetic` exists so a self-test result can never be mistaken for a race.

Note what is absent: no ET, no split, no speed, no lane identity, no beam
meaning. The node does not know what it measured.

### 0x00C0 — Tree module only (`device_class` = 2)

| Addr | Type | Name |
|---|---|---|
| 0x00C0 | u16 | `tree_state` — 0 = idle, 1 = armed, 2 = sequencing, 3 = green |
| 0x00C1 | u16 | `tree_mode` — 0 standard (500 ms), 1 pro (400 ms) |
| 0x00C2 | u16 | `lamp_flags` |
| 0x00C3 | u16 | `sequence_gen` |
| 0x00C4 | u16 | `foul_flags` |
| 0x00C5 | u16 | `handicap_l1_ms` — ms this lane's cascade is held back |
| 0x00C6 | u16 | `handicap_l2_ms` |
| 0x00C7 | i32 | `reaction_time_l1` — ticks from **this lane's** green, negative = red |
| 0x00C9 | i32 | `reaction_time_l2` |
| 0x00CB | u32 | `t_green_l1` — captured from the lamp driver output |
| 0x00CD | u32 | `t_green_l2` |

`lamp_flags`:

| Bit | Meaning |
|---|---|
| 0 | `prestage_l1` |
| 1 | `stage_l1` |
| 2 | `amber1_l1` |
| 3 | `amber2_l1` |
| 4 | `amber3_l1` |
| 5 | `green_l1` |
| 6 | `red_l1` |
| 7 | `prestage_l2` |
| 8 | `stage_l2` |
| 9 | `amber1_l2` |
| 10 | `amber2_l2` |
| 11 | `amber3_l2` |
| 12 | `green_l2` |
| 13 | `red_l2` |

Reaction time is measured by the tree because the tree owns the green instant
and, under **D24**, also observes the launch pulse — so both terms sit on one
clock and **D04** is not violated to produce a number handed to a driver. A red
light is not a special case; it is a negative reaction time. See
[`software.md`](software.md) §5, including why the green instant must be
captured from the driver output rather than taken when firmware writes the LED.

**The staging lamps are written, not sensed.** The beams land on the start
nodes and the lamps hang here, so the master reads one and writes the other:
`tree_staging` carries the four pre-stage and stage bits in their `lamp_flags`
positions. A write that reaches for any other bit is **refused**, not masked —
the cascade lamps belong to the tree's own sequence, and a master that thinks it
can light the green should hear about it. The round trip costs two poll hops,
which [`software.md`](software.md) §4 prices and §8 #10 offers a way to shorten.

**Two lanes, two of everything (D28).** A handicap start holds the quicker car's
cascade back by the difference between the two dial-ins, so the lanes are
genuinely in different places: the ambers and the green are per lane, and there
are two green instants rather than one. Reaction time is measured against *that
lane's* green. The handicap is written with `tree_handicap` before `tree_arm`,
which latches it — pending values do not survive an arm, so a spot forgotten
from the previous pair cannot silently apply to the next one.

Nothing downstream changes: ET's zero is still that car's own launch pulse, and
**D20**'s launch margin already carries the handicap, because the handicap *is*
part of the difference between the two pulses.

### 0x0100 — Commands (FC6 / FC16)

| Addr | Type | Name |
|---|---|---|
| 0x0100 | u16 | `opcode` |
| 0x0101 | u16 | `arg0` |
| 0x0102 | u16 | `arg1` |
| 0x0103 | u16 | `command_seq` — master increments |

The node echoes `command_seq` at 0x0026 and the result at 0x0027, so a command
is confirmed by a subsequent read rather than by the write's acknowledgement.
Retrying a write with an unchanged `command_seq` is therefore safe.

| Opcode | Command |
|---|---|
| 1 | `identify` — blink, arg0 = seconds |
| 2 | `alignment_mode` — arg0 = input mask, arg1 = seconds |
| 3 | `self_test` — arg0 = interval in µs, injected into the node's own input path (§3) |
| 4 | `clear_faults` |
| 5 | `clear_run` — arg0 = lane mask |
| 6 | `log_seek` — arg0/arg1 = record index, high word first |
| 7 | `reboot` — arg0 = magic |
| 16 | `tree_arm` — arg0 = mode, arg1 = random delay bound in ms (tree only) |
| 17 | `tree_abort` |
| 18 | `tree_lamp_test` |
| 19 | `tree_handicap` — arg0 = lane (1\|2), arg1 = ms that lane is held back |
| 20 | `tree_staging` — arg0 = pre-stage and stage bits in `lamp_flags` positions |

### 0x0200 — Raw log page (read)

16 records of 4 registers: `t_ms` (u32), input index, flags. The cursor is set
by `log_seek` and **is not advanced by reading** — a read-advancing cursor makes
a retried read return different data, which is exactly what a noisy bus
produces. Idempotent reads, explicit seeks.

The log is dispute evidence (§6) on a coarse millisecond clock (**D20**), pulled
after a round. It never appears in the live poll loop.

## 4. Master behaviour

Rules that belong to the master and nowhere else.

- **Poll for change, read on change.** Digest every cycle; a full run record
  only when that lane's generation moves; telemetry round-robin, one device per
  cycle. The arithmetic behind this is in [`software.md`](software.md) §4 —
  19,200 bps buys ~192 characters per 100 ms for the whole bus, which a
  28-register record does not fit.
- **Quiet window** (§3): stop polling from "both staged" until every mapped
  node's generation has advanced or timed out. The start pulse and the bus share
  one cable on separate pairs, and width validation rejects spikes but not a
  sustained transmission burst.
- **Nodes may be polled arbitrarily late.** Records latch until the next pulse,
  so a delayed poll loses nothing. Anything that relies on polling promptly to
  avoid missing an event is a bug in the master, not a tuning problem.
- **Reboot detection.** `boot_count` change or `run_gen` = 0 invalidates
  anything held for that node. A rebooted node must never appear to hold a
  valid split.
- **Corrections are applied here**: crystal ppm by MAC (**D13**), temperature
  correction if **T4** finds one worth applying (**D19**).
- **Self-test before a round** (§3): inject the same interval into every node
  and compare what comes back. Results carry `synthetic`.

## 5. The mapping file

The single source of truth for what the numbers mean (**D08**). TOML, one file
per venue, versioned in the club's own repository — never on a node.

```toml
[venue]
name  = "Example Strip"
lanes = 2

# All distances laser-measured, in metres. §2: 5 cm of error in the trap
# base is 0.25 % of speed, which dwarfs the electronics' contribution.
[geometry]
sixty_foot     = 18.288
eighth_mile    = 201.168
finish         = 402.336
trap_base      = 20.115
stage_to_guard = 0.340

# Which node's pulse observation provides the launch margin (D20).
[margin]
source_address = 1

[[node]]
address     = 1
label       = "start-lane1"
mac         = "7c:df:a1:00:11:22"   # expected; mismatch is a warning
crystal_ppm = -12.4                  # measured once per board (D13)
terminated  = true                   # physical end of the bus (D09)

  [[node.input]]
  index = 0
  beam  = "prestage"
  lane  = 1

  [[node.input]]
  index = 1
  beam  = "stage"
  lane  = 1

  [[node.input]]
  index = 2
  beam  = "guard"
  lane  = 1

[[node]]
address = 4
label   = "trap"
mac     = "7c:df:a1:00:33:44"

  [[node.input]]
  index = 0
  beam  = "trap_entry"
  lane  = 1

  [[node.input]]
  index = 1
  beam  = "trap_entry"
  lane  = 2

# Optional, only if T4 finds a drift stable enough to calibrate (D19).
[[correction.temperature]]
mac        = "7c:df:a1:00:11:22"
input      = 1
ref_c      = 25.0
us_per_c   = 0.8
```

Beam meanings are a closed set: `prestage`, `stage`, `guard`, `interval_60`,
`interval_660`, `trap_entry`, `trap_exit`, `finish`. An unknown value is a load
error, not a warning — a typo must not silently drop a beam.

### Validation at load

The master refuses to start a round on a mapping file that fails any of these,
because every one of them produces a plausible-looking wrong number rather than
a visible failure:

- every mapped `(address, input)` exists in that node's `input_present` bitmap;
- no beam meaning is duplicated within a lane;
- `stage` and `finish` exist for every declared lane — the minimum system;
- `trap_base` is present if any `trap_*` beam is mapped;
- `trap_entry` and `trap_exit` are on the same node and the same lane, so the
  interval closes inside one timer;
- exactly one `margin.source_address`, and that node reports both pulses seen;
- exactly two nodes flagged `terminated` (**D09**);
- `guard` is mapped for every lane that has one, since §2 makes it mandatory
  for cars with aero and its absence changes how a break is interpreted;
- every `crystal_ppm` belongs to a MAC actually present on the bus.

Mismatch between a mapped MAC and the MAC read from that address is a
**warning**, not an error: swapping a dead node in the field means copying DIP
positions, and **D08** exists precisely so that works without editing this file.
The warning is there so the swap gets recorded afterwards, not to block it.
