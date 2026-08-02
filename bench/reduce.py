#!/usr/bin/env python3
"""Turn a bench capture into the numbers T1, T2 and T4 are reported as.

`bench-validation.md` §5: the rig produces thousands of edge timestamps and the
deliverable is a distribution. This cannot be read off a PulseView window by eye,
and it must be the *same* script every time, so runs from different days and
different sensors compare.

By **D15** this is the gate. No batch purchase, no PCB fabrication and no public
timeline exists until these numbers do — so the thing this script must never do
is produce a clean-looking answer from a capture that did not deserve one.

Three habits follow from that, and they are the whole design:

* **Nothing is silently dropped.** An edge that could not be paired is counted
  and reported. A capture where a third of the passes did not pair is a rig
  problem, and a tidy sigma over the two thirds that worked would hide it.
* **Peak-to-peak leads.** §3 sets T1's threshold on peak-to-peak rather than
  sigma, "because one bad run loses a race", so that is the number printed first.
* **Speed and temperature are required.** T4 is about drift and cannot be
  reconstructed later from a report that forgot the temperature. Passing `?` is
  allowed and says NOT RECORDED in capitals.

Standard library only (`software.md` §6).

    ./reduce.py capture.vcd --sensor D0 --reference D1 --rpm 2650 --temp-c 21.5
"""

import argparse
import statistics
import sys

import capture

# The thresholds from bench-validation.md §3. Peak-to-peak, not sigma.
T1_JITTER_PP = 400e-6
T2_ASYMMETRY = 500e-6
T3_CAPTURE = 50e-6


class Pass:
    """One slot edge, seen by both detectors."""

    __slots__ = ("t", "delta", "breaking")

    def __init__(self, t, delta, breaking):
        self.t = t
        self.delta = delta
        self.breaking = breaking


def beam_blocked(edge, light_on):
    """Did this electrical edge mean the beam went dark?

    **D17** is PNP / light-ON: the output is high while the beam is intact, so a
    break is a *falling* edge. A reference detector wired the other way is the
    normal case rather than an error, which is why this is a flag and not an
    assumption.
    """
    return (not edge.high) if light_on else edge.high


def pair(edges, sensor, reference, sensor_light_on, ref_light_on, window):
    """Match each sensor edge to the reference edge of the same beam direction.

    Both detectors watch the same slot, so a pass produces one edge on each
    within a few tens of microseconds. Pairing is nearest-in-time *within a
    window*, and an edge with no partner inside it is returned as unpaired
    rather than matched to something far away — a wrong pairing is a plausible
    number, which is the failure this whole project refuses.
    """
    s = [(e.t, beam_blocked(e, sensor_light_on)) for e in edges if e.channel == sensor]
    r = [(e.t, beam_blocked(e, ref_light_on)) for e in edges if e.channel == reference]
    if not s:
        raise capture.CaptureError(f"no transitions on {sensor}")
    if not r:
        raise capture.CaptureError(f"no transitions on {reference}")

    passes, unpaired = [], 0
    j = 0
    for t, blocked in s:
        # The reference list is in time order, so the search only ever moves
        # forward: this stays linear over a capture with a million edges.
        while j + 1 < len(r) and abs(r[j + 1][0] - t) <= abs(r[j][0] - t):
            j += 1
        best = None
        for k in (j - 1, j, j + 1):
            if 0 <= k < len(r) and r[k][1] == blocked:
                if best is None or abs(r[k][0] - t) < abs(r[best][0] - t):
                    best = k
        if best is None or abs(r[best][0] - t) > window:
            unpaired += 1
            continue
        passes.append(Pass(t, t - r[best][0], blocked))
    return passes, unpaired


def stats(values):
    """What §3 asks a run to be reported as."""
    if not values:
        return None
    v = sorted(values)
    return {
        "n": len(v),
        "mean": statistics.fmean(v),
        "sd": statistics.stdev(v) if len(v) > 1 else 0.0,
        "pp": v[-1] - v[0],
        "p99": v[min(len(v) - 1, int(round(0.99 * (len(v) - 1))))],
    }


def us(x):
    return f"{x * 1e6:9.1f}"


def report(args, passes, unpaired, out=sys.stdout):
    breaks = [p.delta for p in passes if p.breaking]
    makes = [p.delta for p in passes if not p.breaking]
    both = stats([p.delta for p in passes])
    b, m = stats(breaks), stats(makes)

    w = out.write
    w(f"beam402 bench reduction — {args.capture}\n")
    w(f"sensor {args.sensor} vs reference {args.reference}\n")
    w(f"speed  {args.rpm} rpm\n")
    temp = "NOT RECORDED" if args.temp_c is None else f"{args.temp_c:.1f} C"
    w(f"body   {temp}\n")
    if args.note:
        w(f"note   {args.note}\n")
    w("\n")

    w("                    n        mean          sd     pk-pk         p99\n")
    for label, s in (("pooled", both), ("break", b), ("make", m)):
        if s is None:
            w(f"{label:<12}       0            no edges of this direction\n")
            continue
        w(
            f"{label:<12} {s['n']:7d}  {us(s['mean'])}us {us(s['sd'])}us "
            f"{us(s['pp'])}us {us(s['p99'])}us\n"
        )
    w("\n")

    total = len(passes) + unpaired
    if unpaired:
        share = 100.0 * unpaired / total if total else 0.0
        # Loud on purpose. A tidy distribution over the edges that happened to
        # pair is exactly the shape of a wrong answer.
        w(
            f"UNPAIRED  {unpaired} of {total} sensor edges ({share:.1f}%) had no "
            f"reference edge within {args.window * 1e6:.0f} us.\n"
            f"          That is a rig or wiring problem, not a distribution. "
            f"Fix it before believing anything above.\n\n"
        )

    # T1 is judged **per edge direction**, not over the pooled set.
    #
    # §1 says conflating jitter with offset is the classic mistake, and pooling
    # commits it: the make/break asymmetry is a *shift of two means*, so it lands
    # in the pooled peak-to-peak and inflates it. A sensor with 100 us of real
    # jitter and a 350 us offset — jitter well inside T1, offset well inside T2 —
    # pools to 450 and would be failed for a fault it does not have. The offset
    # is calibratable and T2 is where it is judged.
    worst = max(
        (s for s in (b, m) if s), key=lambda s: s["pp"], default=both
    )
    verdicts = []
    if worst:
        verdicts.append(("T1  jitter, peak-to-peak", worst["pp"], T1_JITTER_PP, "<"))
    if b and m:
        verdicts.append(
            ("T2  make/break asymmetry", abs(b["mean"] - m["mean"]), T2_ASYMMETRY, "<")
        )
    for name, value, limit, _ in verdicts:
        ok = value < limit
        w(
            f"{name:<26} {us(value)}us  against {limit * 1e6:.0f}us  "
            f"{'PASS' if ok else 'FAIL'}\n"
        )
    if not verdicts:
        w("no verdict: not enough edges to judge\n")
    elif b and m:
        w(
            "\nT1 is the worse of the two edge directions, not the pooled spread: "
            "the\nasymmetry is an offset and pooling would charge it to jitter "
            "(§1).\n"
        )
    elif b and m:
        w(
            "\nT2 also requires the asymmetry to be *stable* across repeat runs "
            "(§3).\nOne run cannot show that. Run it again and compare.\n"
        )
    if args.temp_c is None:
        w(
            "\nT4 needs the body temperature of every run and this one does not "
            "have it.\nThe capture is still good; the report is not comparable "
            "until it is recorded.\n"
        )
    return 0 if all(v[1] < v[2] for v in verdicts) and not unpaired else 1


def against(args, edges, out=sys.stdout):
    """T3: the node's own reported intervals against the analyzer's.

    A different shape of question from T1 — two measurements of the *same*
    interval by two instruments, rather than two detectors on one event — so it
    is a separate mode rather than a flag. §3 expects sub-microsecond, and warns
    that tens of microseconds means the path fell back to a GPIO interrupt
    instead of hardware capture.
    """
    ticks = []
    with open(args.against, "r", encoding="utf-8") as f:
        for line in f:
            line = line.split("#")[0].strip()
            if line:
                ticks.append(float(line))

    marks = [e.t for e in edges if e.channel == args.sensor and e.rising]
    measured = [b - a for a, b in zip(marks, marks[1:])]
    n = min(len(ticks), len(measured))
    if n == 0:
        raise capture.CaptureError(
            "nothing to compare: the capture or the node's intervals are empty"
        )
    deltas = [ticks[i] - measured[i] for i in range(n)]
    s = stats(deltas)

    w = out.write
    w(f"beam402 bench reduction — T3 — {args.capture}\n")
    w(f"node intervals {args.against}\n")
    if len(ticks) != len(measured):
        # Never quietly truncate to the shorter list: a missing interval means
        # the node and the analyzer are not looking at the same pulses, and the
        # difference distribution is then meaningless rather than merely short.
        w(
            f"\nMISMATCH  the node reported {len(ticks)} intervals, the analyzer "
            f"saw {len(measured)}.\n          Compared the first {n}. They may "
            f"not be the same {n} — check the trigger.\n"
        )
    w("\n")
    w(f"n        {s['n']}\n")
    w(f"mean     {us(s['mean'])}us\n")
    w(f"sd       {us(s['sd'])}us\n")
    w(f"pk-pk    {us(s['pp'])}us\n")
    w(f"p99      {us(s['p99'])}us\n\n")
    ok = s["pp"] < T3_CAPTURE
    w(
        f"T3  capture jitter, peak-to-peak {us(s['pp'])}us  against "
        f"{T3_CAPTURE * 1e6:.0f}us  {'PASS' if ok else 'FAIL'}\n"
    )
    w(
        "\nThe analyzer quantises to +/-1 sample per edge, so at 24 MHz it adds "
        "~83 ns\nof spread by itself (§5). Ample against 50 us; not fine enough "
        "to measure a\ntrue capture jitter of a few ticks. Say so in the report.\n"
    )
    return 0 if ok else 1


def temperature(text):
    if text == "?":
        return None
    return float(text)


def parser():
    """The command line, exposed so the tests drive the real defaults."""
    p = argparse.ArgumentParser(
        description="Reduce a bench capture to the numbers bench-validation.md asks for.",
        epilog="Thresholds are from bench-validation.md §3. Exit status is 0 only "
        "if every applicable test passes and every edge paired.",
    )
    p.add_argument("capture", help="VCD from sigrok/PulseView, or CSV for a short burst")
    p.add_argument("--sensor", required=True, help="channel name of the sensor under test")
    p.add_argument("--reference", help="channel name of the reference detector (T1/T2/T4)")
    p.add_argument(
        "--against",
        help="file of the node's own reported intervals in seconds, one per line (T3)",
    )
    p.add_argument("--rpm", required=True, help="disk speed for this run")
    p.add_argument(
        "--temp-c",
        required=True,
        type=temperature,
        help="sensor *body* temperature, or ? if it was not measured",
    )
    p.add_argument("--note", default="", help="sensor part number, rig notes, anything")
    p.add_argument(
        "--window",
        type=float,
        default=2e-3,
        help="furthest a reference edge may be and still be the same pass, in seconds "
        "(default 2 ms)",
    )
    p.add_argument(
        "--sensor-dark-on",
        action="store_true",
        help="the sensor drives high when the beam is BLOCKED. D17 is light-ON, so "
        "this is not the default",
    )
    p.add_argument(
        "--reference-dark-on",
        action="store_true",
        help="same, for the reference detector",
    )
    return p


def main(argv=None):
    p = parser()
    args = p.parse_args(argv)

    if not args.reference and not args.against:
        p.error("give --reference for T1/T2/T4, or --against for T3")

    wanted = [args.sensor] + ([args.reference] if args.reference else [])
    edges = capture.read(args.capture, wanted)

    if args.against:
        return against(args, edges)

    passes, unpaired = pair(
        edges,
        args.sensor,
        args.reference,
        not args.sensor_dark_on,
        not args.reference_dark_on,
        args.window,
    )
    if not passes:
        raise capture.CaptureError(
            "no sensor edge found a reference edge. Check the channel names, the "
            "polarity flags, and --window."
        )
    return report(args, passes, unpaired)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except capture.CaptureError as e:
        print(f"reduce: {e}", file=sys.stderr)
        sys.exit(2)
