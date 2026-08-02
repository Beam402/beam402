#!/usr/bin/env python3
"""Synthetic captures, so the reduction script can be tested before the rig runs.

`software.md` §7 puts the reduction script first in the build order and says it
is tested "with synthetic captures". This is that: a VCD with a *known* jitter,
a *known* edge asymmetry and a *known* number of passes, so the script's answer
can be checked against the truth instead of against a plausible-looking number.

It also earns its place on the day. A capture from the rig is unrepeatable — that
disk, that temperature, that alignment — and finding out the channel names were
wrong after the run is expensive. Generate one of these first, run the reduction
over it, and the plumbing is proven before the motor is switched on.

    ./synth.py out.vcd --passes 300 --rpm 2650 --jitter-us 40 --asymmetry-us 120
"""

import argparse
import random
import sys

# The reference detector sits a few degrees round from the sensor, so a pass
# appears on both within a fraction of a revolution.
REFERENCE_LEAD_S = 200e-6


def build(passes, rpm, jitter_us, asymmetry_us, chord, seed, timescale_ns=1):
    """Edge times in seconds, as (t, channel, high).

    The truth being injected: every sensor edge is late by a mean delay plus a
    uniform jitter, and the *break* edge carries an extra offset that the make
    edge does not — which is the asymmetry T2 exists to find. The reference is
    ideal, because a reference that is not two or three orders faster than the
    sensor is not a reference (§3).
    """
    rng = random.Random(seed)
    period = 60.0 / rpm
    mean_delay = 300e-6
    events = []
    t = 0.05

    for _ in range(passes):
        # The reference sees the edge first, cleanly.
        blocked_at = t
        cleared_at = t + period * chord
        events.append((blocked_at, "REF", False))
        events.append((cleared_at, "REF", True))

        jitter = lambda: rng.uniform(-jitter_us / 2e6, jitter_us / 2e6)
        events.append(
            (
                blocked_at + REFERENCE_LEAD_S + mean_delay + asymmetry_us / 1e6 + jitter(),
                "SENSOR",
                False,
            )
        )
        events.append(
            (cleared_at + REFERENCE_LEAD_S + mean_delay + jitter(), "SENSOR", True)
        )
        t += period

    events.sort(key=lambda e: e[0])
    return events


def write_vcd(path, events, timescale_ns=1):
    """A VCD in the shape sigrok emits, so the reader is tested on the real thing."""
    ids = {"SENSOR": "!", "REF": '"'}
    scale = timescale_ns * 1e-9
    with open(path, "w", encoding="utf-8") as f:
        f.write("$date synthetic $end\n")
        f.write("$version beam402 bench/synth.py $end\n")
        f.write("$comment generated, not measured $end\n")
        f.write(f"$timescale {timescale_ns} ns $end\n")
        f.write("$scope module beam402 $end\n")
        for name, ident in ids.items():
            f.write(f"$var wire 1 {ident} {name} $end\n")
        f.write("$upscope $end\n")
        f.write("$enddefinitions $end\n")
        # Both channels start with the beam intact: high, under D17's light-ON.
        f.write("#0\n")
        for ident in ids.values():
            f.write(f"1{ident}\n")
        last = None
        for t, channel, high in events:
            tick = int(round(t / scale))
            if tick != last:
                f.write(f"#{tick}\n")
                last = tick
            f.write(f"{'1' if high else '0'}{ids[channel]}\n")


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    p.add_argument("out", help="where to write the VCD")
    p.add_argument("--passes", type=int, default=300, help="§3 asks for at least 300")
    p.add_argument("--rpm", type=float, default=2650.0, help="2650 is the fast setting")
    p.add_argument(
        "--jitter-us", type=float, default=40.0, help="peak-to-peak, uniform"
    )
    p.add_argument(
        "--asymmetry-us",
        type=float,
        default=0.0,
        help="extra delay on the break edge only — what T2 measures",
    )
    p.add_argument(
        "--chord",
        type=float,
        default=0.05,
        help="fraction of a revolution the slot keeps the beam broken",
    )
    p.add_argument("--seed", type=int, default=1, help="same seed, same capture")
    p.add_argument(
        "--timescale-ns",
        type=int,
        default=1,
        help="VCD tick, in nanoseconds. 42 is roughly a 24 MHz analyzer",
    )
    args = p.parse_args(argv)

    events = build(
        args.passes,
        args.rpm,
        args.jitter_us,
        args.asymmetry_us,
        args.chord,
        args.seed,
    )
    write_vcd(args.out, events, args.timescale_ns)
    print(
        f"wrote {args.out}: {args.passes} passes at {args.rpm:g} rpm, "
        f"{args.jitter_us:g} us jitter, {args.asymmetry_us:g} us asymmetry"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
