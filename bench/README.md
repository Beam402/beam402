# Bench tooling

Data reduction for [`docs/bench-validation.md`](../docs/bench-validation.md).
Python, standard library only, no install — it has to run on whatever laptop is
next to the motor.

By **D15** these numbers are the project's gate: no batch purchase, no PCB
fabrication and no public timeline exists until they do.

## Before the motor is switched on

**A capture is unrepeatable and a script is not.** That disk, that temperature,
that alignment, that sensor — none of it comes back. So prove the plumbing
against a synthetic capture first, and find out that the channel names were
wrong now rather than after the run:

```sh
./synth.py /tmp/dry-run.vcd --passes 300 --jitter-us 40 --asymmetry-us 120
./reduce.py /tmp/dry-run.vcd --sensor SENSOR --reference REF --rpm 2650 --temp-c 21
```

It should report ~40 µs of jitter and ~120 µs of asymmetry, because that is what
was put in. Then set the real channel names in PulseView to match what you will
pass to `--sensor` and `--reference`, and go.

## On the day

Export **transitions**, not samples — §5 does the arithmetic on why: 300 passes
at creep speed is 667 million samples and some thirteen gigabytes of text.

```sh
# T1, T2, T4 — two detectors, one capture, ~1 MHz
./reduce.py run-01.vcd \
    --sensor D0 --reference D1 \
    --rpm 2650 --temp-c 21.5 \
    --note "candidate A, clone, 130 mm beam height"

# T3 — the node's own reported intervals against the analyzer's, 24 MHz
./reduce.py t3.vcd --sensor D0 --against node-intervals.txt --rpm 0 --temp-c 21.5
```

`--rpm` and `--temp-c` are **required**. T4 is about drift and cannot be
reconstructed from a report that forgot the temperature. If it genuinely was not
measured, pass `--temp-c ?` and the report says NOT RECORDED in capitals.

The exit status is 0 only when every applicable test passes *and* every edge
paired, so this can drive a script without anyone reading the text.

## What it refuses to do

- **It does not drop edges quietly.** A sensor edge with no reference edge inside
  `--window` is counted and shouted about. A tidy σ over the two thirds of passes
  that happened to pair is exactly the shape of a wrong answer.
- **It does not charge an offset to jitter.** §1 calls conflating the two the
  classic mistake — jitter is unfixable, an offset is calibratable — so T1 is
  judged on the worse *single* edge direction, never on the pooled spread, and
  the asymmetry is judged separately by T2.
- **It does not decide T2 for you.** §3 requires the asymmetry to be *stable*
  across repeat runs, and one run cannot show that. The report says so.
- **It does not guess at polarity.** **D17** is PNP light-ON, so a beam break is
  a falling edge; a reference detector wired the other way is normal, and
  `--reference-dark-on` says so explicitly rather than being inferred.

## Save everything

§5: commit the script *and* the raw captures. Numbers derived later from a
preserved capture are as good as numbers derived on the spot; numbers eyeballed
from a capture nobody kept are gone.

Report results as issues with the **validation result** template, one per test,
pass or fail. A failed measurement recorded is worth more to this project than a
successful one kept in a notebook.

## Tests

```sh
cd bench && python3 -m unittest discover
```

They run against synthetic captures whose jitter and asymmetry are *known*, so
the script's answer is checked against the truth rather than against something
that looks reasonable. The ones that matter are not the ones proving it computes
a mean — they are the ones proving it refuses to look clean when the capture is
not.
