# Beam402 — Bench Validation Protocol

> Status: **protocol written, not yet run.** No results exist. This document
> says how to produce them.

This is the project's current stage and, by **D15**, its gate: no batch
purchases, no PCB fabrication and no public timelines until these measurements
exist. Everything downstream — the node design, the BOM, the promise of ±1 ms —
rests on numbers that no manufacturer publishes.

Results are reported as issues using the **validation result** template, one
issue per test, whether they pass or fail. A failed measurement recorded is
worth more to this project than a successful one kept in a notebook.

---

## 1. What is being measured, and what is not

**Not in question: whether the sensor sees the wheel.** A 30-inch tire presents
a ~573 mm chord at a 130 mm beam height, so the beam stays broken for 20.6 ms
at 100 km/h and 6.9 ms at 300 km/h, against a rated 1 ms response. That is a
sevenfold margin at the worst case and it is arithmetic, not an experiment.

**In question: whether the *instant* is reported the same way every time.** The
datasheet's "max 1 ms" is a worst-case *delay*, not a repeatability figure. A
constant delay subtracts out by calibration and costs nothing. A wandering one
lands directly in ET, and at 100 km/h one millisecond is 27.8 mm of car.

Two distinct quantities come out of this, and conflating them is the classic
mistake:

| Quantity | Symptom | Fixable by |
|---|---|---|
| **Jitter** — spread around the mean | random ET error, run to run | nothing; only a better sensor |
| **Offset** — shift of the mean | systematic ET error, invisible without a reference | calibration, *if* it is stable |

---

## 2. The rig

### Parts

From BOM stage 0. The rig itself is: a motor, a slotted disk, the sensor under
test, a reference detector, and a logic analyzer.

### Disk

- Rigid and flat — plywood, aluminium, or a blank PCB. ~100 mm radius, which
  gives 27.8 m/s edge speed (100 km/h) at 2650 rpm and staging creep at 27 rpm.
- **One slot, not many.** Multiple slots turn slot-to-slot machining variation
  into apparent sensor jitter. If you must use several, index them and analyse
  each separately.
- **Slot edges must be radial** — straight lines through the centre. A
  non-radial edge crosses different radii at different angles, which breaks the
  cancellation described below.
- Edges clean and square. A ragged edge is real variation, and you will spend a
  day blaming the sensor for it.

### Detector placement — the part that is easy to get wrong

The reference detector exists because motor speed wanders, and with a single
sensor that wander is indistinguishable from sensor jitter. Every measurement
below is a **difference** between two detectors watching the same slot edge, so
that common-mode speed drift cancels.

That cancellation is only as good as the angular separation between them:

| Separation | Effect of 1 % speed drift at 2650 rpm |
|---|---|
| 90° | 57 µs — a sixth of the abort threshold, injected by the rig |
| 5° | 3 µs — negligible |

**Mount the reference at the same clock position as the beam, on a smaller
radius** (e.g. beam at r = 100 mm, reference at r = 60 mm). Both then catch the
same edge at essentially the same instant.

Mount both rigidly, and decouple the motor mechanically — vibration is jitter
you added yourself.

### Wiring

```
sensor receiver ──[820 Ω]──▶│─ optocoupler ──┬── logic analyzer ch1
                                             └── ESP32 capture input
reference detector ── comparator ────────────┬── logic analyzer ch2
                                             └── (ESP32 capture input, T3)
ESP32 marker output ─────────────────────────── logic analyzer ch3
```

Common ground for the analyzer. The 820 Ω value should be checked against the
real optocoupler and the real sensor before trusting it — PNP residual voltage
varies from 0.5 to 2.5 V between parts, which swings LED current nearly 2:1.

---

## 3. Tests

Run T1–T5 for every sensor candidate: the brand part, each clone, and the
drag-specific unit. That comparison is the entire point of D15 — a clone that
passes changes the cost of the project by hundreds of dollars per venue.

### T1 — Sensor jitter

**Answers:** §11 #1. The gating measurement.

1. Spin the disk at the fast setting (~2650 rpm, 27.8 m/s edge speed).
2. Capture at least **300 passes** — sample rate and export format per §5.
3. For each pass compute Δt = t_sensor − t_reference on the same edge.
4. Repeat at the creep setting (~27 rpm). 300 passes takes ~11 minutes there.

**Record:** mean, standard deviation, peak-to-peak, and 99th percentile of Δt,
at both speeds, plus sensor body temperature and the number of passes.

**Expect:** a mean delay comfortably under the 1 ms datasheet maximum — likely
a few hundred µs. The spread is the unknown; a good sensor should land in tens
of µs.

**Pass:** peak-to-peak spread **< 400 µs**. Peak-to-peak rather than σ, because
one bad run loses a race.

### T2 — Make/break edge asymmetry

**Answers:** §11 #2. Produces a systematic offset, not noise, so it never
averages away.

Same capture as T1 — the data is already there, it just has to be split.

1. Compute Δt separately for the **break** edge (beam blocked) and the **make**
   edge (beam restored).
2. Take the difference of the two means.
3. Repeat at both speeds.

**Why it matters:** §2 starts ET when the tire *exits* the stage beam and stops
it when the tire *breaks* the finish beam. Two opposite transitions, through a
sensor whose hysteresis makes the thresholds deliberately unequal. The
difference lands in every ET and cancels nowhere.

**Record:** mean and spread for each edge separately, and their difference.

**Note:** the reference detector has its own asymmetry, so what you measure is
sensor-minus-reference. With a reference two or three orders faster, that error
is small — but say so in the report rather than assuming it away.

**Pass:** difference **< 500 µs** *and* stable across repeat runs. A large but
rock-steady offset is calibratable; a small drifting one is not.

### T3 — Capture jitter

**Answers:** §11 #3. Isolates the MCU from the sensor.

1. Bypass the sensor: feed the node input a clean electrical pulse — from the
   reference detector, or a generated one.
2. Have the node timestamp it with hardware capture (MCPWM), and compare the
   node's reported interval against the logic analyzer's measurement of the
   same interval.
3. At least 300 samples, captured as short bursts at the analyzer's full rate
   (§5) rather than as one long recording.

**Expect:** sub-microsecond. MCPWM capture is 32-bit at 80 MHz, i.e. 12.5 ns
resolution. Tens of µs means you are not actually using hardware capture —
check that the path is not falling back to a GPIO interrupt.

**Pass:** **< 50 µs**, expected far below.

**Also verify here:** the capture counter is 32-bit at 80 MHz and wraps every
~54 s. Confirm the firmware accumulates to 64 bits and that an interval
spanning a wrap is still reported correctly. Test it deliberately.

### T4 — Thermal drift

**Answers:** §11 #4. The failure mode is drift, not death: right in the
morning, quietly wrong by mid-afternoon.

1. Run T1 and T2 at room temperature. Record the sensor **body** temperature
   with the IR thermometer — not the air.
2. Enclose the sensor and warm it to ~60 °C with the heat gun. Let it settle.
3. Repeat T1 and T2 at temperature.

**Record:** shift in mean delay, change in spread, and both body temperatures.
The shift matters more than the spread here.

**Pass:** mean shift small enough to ignore, or stable and repeatable enough to
become a temperature correction in the mapping file. If it is large *and*
erratic, shading discipline (D19) becomes mandatory rather than advisory.

### T5 — Adjacent-beam crosstalk

**Answers:** §11 #6. Note the question is *how much rejection is needed*, not
*whether it happens* — the datasheet already settles that a neighbouring
emitter at 178 mm sits inside the receiver's ~7.5° acceptance.

1. Set up two beams at the real span, 178 mm apart, emitters both on the same
   side (as they will be in the field).
2. Block beam 1 only. Does receiver 1 report the break, or does it keep seeing
   emitter 2?
3. If it fails: reduce receiver sensitivity with the built-in VR, following the
   datasheet procedure, and repeat. Record the setting at which it starts
   reporting correctly.
4. Then add a hood of increasing depth and find the depth at which it works at
   full sensitivity.
5. **Repeat with two different manufacturers** — brand plus clone. Different
   modulation frequencies may remove the interference outright, which would be
   the cheapest possible fix.

**Record:** which combinations fail, the sensitivity setting or hood depth that
fixes each, and whether reduced sensitivity costs anything in T1.

**Expect:** failure at full sensitivity with two identical sensors. The useful
output is the cost of the fix.

---

## 4. Deferred to the next stage

These need hardware that stage 0 does not buy:

- **§11 #5 — start-pulse noise immunity.** Requires the full cable drum and an
  oscilloscope. A logic analyzer cannot show you the shape of coupled ignition
  noise, only that a bit went wrong.
- **§11 #7 — sunlight in the receiver axis.** Requires a real low sun. Take it
  opportunistically: point a receiver at a rising or setting sun, with and
  without a hood, and record the threshold at which it fails.

---

## 5. Capture settings and data reduction

The rig produces thousands of edge timestamps; the deliverable is a
distribution. This cannot be read off a PulseView window by eye.

### Export transitions, not samples

The obvious instruction — "export CSV" — does not survive the arithmetic. At
2650 rpm, 300 passes is 6.8 s of recording; at 24 MHz that is 163 million
samples, several GB of text. The creep run is far worse: 300 passes at 27 rpm
takes ~11 minutes, so even at 1 MHz it is 667 million samples, ~13 GB.
**Sample-level export is not merely wasteful at creep speed, it is impossible at
any useful rate.**

Export **transitions**. VCD is the natural format — it records value changes
rather than samples, so the same 300 passes become ~1800 lines. Keep CSV as a
fallback for short captures only, and read either one as a stream rather than
loading it whole.

### Sample rate is not the same for every test

| Tests | Rate | Why |
|---|---|---|
| T1, T2, T4 | ~1 MHz | 1 µs against a 400 µs pass threshold is ample, and the runs are long |
| T3 | full 24 MHz | expects sub-microsecond, so it needs the 41.7 ns floor — but its captures are short bursts of ~300 pulse pairs, not minutes of recording |

That split is what makes both feasible at once: long captures at reduced rate,
short bursts at full rate. Note the analyzer's own quantization is ±1 sample per
edge, so at 24 MHz it contributes ~83 ns peak-to-peak of *added* spread. Ample
for proving T3's < 50 µs; not fine enough to measure a true capture jitter of a
few ticks, which is a limit to state in the report rather than to forget.

### The reduction script

Given a capture, it reports: number of passes, mean, σ, peak-to-peak and 99th
percentile of Δt, split by edge direction, plus the run's speed and temperature.
The same script for every run, so results from different days and different
sensors are comparable.

**If the script is written on the day rather than before it, the captures are
what must be protected.** A capture is unrepeatable — that disk, that
temperature, that sensor, that alignment — while a script can be written twice.
So from the very first run, save the raw transitions with the speed and body
temperature recorded alongside, and do not draw a conclusion from a window read
by eye. Numbers derived later from a preserved capture are as good as numbers
derived on the spot; numbers eyeballed from a capture nobody kept are gone.

Commit the script and the raw captures. For a project whose argument is
verifiability, the measurements are as much a deliverable as the design.

---

## 6. What a failure means

The abort criteria from D15, and what each one implies:

| Result | Meaning |
|---|---|
| Jitter > 400 µs | Wrong sensor, not a firmware problem. Escalate to laser at finish/trap (D03) or to the sports-timing category (D02) |
| Edge offset > 500 µs **and** drifting with temperature | Same conclusion — a drifting offset cannot be calibrated |
| Capture jitter tens of µs | Firmware problem — the path is not using hardware capture |
| Crosstalk unfixable by sensitivity or a shallow hood | Revisit D18; ask distributors again about paired A/B modulation channels |
| A clone matches the brand part | The open BOM gets several times cheaper — the best outcome available here |

**Ground truth.** None of the numbers above can be validated against another
copy of the same sensor. No vendor in any category publishes repeatability, so
borrow or rent **one** timing-grade photocell (Alge, Microgate) and run T1
against it. One unit, not eight — buying a set would defeat the project's
premise, but having none means the bench can only compare candidates to each
other.
