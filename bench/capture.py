"""Reading a logic-analyzer capture as a stream of transitions.

`bench-validation.md` §5 is blunt about why this is not "export CSV and load it":
300 passes at creep speed is ~11 minutes, which even at 1 MHz is 667 million
samples and some thirteen gigabytes of text. Sample-level export is not merely
wasteful there, it is impossible.

So VCD is the format, because it records *value changes* rather than samples and
turns the same run into a couple of thousand lines. CSV stays as a fallback for
short bursts, and it is read the same way — as a stream that never holds more
than one row, so a capture that turns out to be larger than expected produces a
slow answer rather than a dead laptop.

Standard library only (§6). This runs on whatever machine is at the bench.
"""

import csv
import re
from dataclasses import dataclass

UNITS = {"s": 1.0, "ms": 1e-3, "us": 1e-6, "ns": 1e-9, "ps": 1e-12, "fs": 1e-15}


class CaptureError(Exception):
    """The capture cannot be read, and guessing would be worse than stopping."""


@dataclass(frozen=True)
class Edge:
    """One transition: when, on which channel, and to what."""

    t: float  # seconds from the start of the capture
    channel: str
    high: bool

    @property
    def rising(self):
        return self.high


def read(path, channels=None):
    """Transitions from a capture, in time order.

    `channels` restricts the result to those names; everything else is dropped
    at the parser rather than downstream, because a 16-channel capture of a
    2-channel test is otherwise most of the work.
    """
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        head = f.read(400)
    if "$timescale" in head or "$enddefinitions" in head:
        return read_vcd(path, channels)
    return read_csv(path, channels)


def read_vcd(path, channels=None):
    """Stream a VCD.

    Handles what sigrok and PulseView actually emit: a `$timescale`, one
    `$var wire 1 <id> <name> $end` per channel, then `#<tick>` stamps followed by
    `0<id>` / `1<id>` lines. Vector values and `x`/`z` are skipped — a
    three-state edge is not a timestamp, and inventing one would be a
    measurement.

    The header is scanned as **tokens**, not by splitting on the text `$end`.
    `$enddefinitions` begins with those four characters, so a naive split tears
    it in half and the header never ends — which is a parser that reads an entire
    capture as its own preamble and then reports, calmly, that there were no
    channels.
    """
    scale = None
    ids = {}
    edges = []
    state = {}
    now = 0.0
    in_header = True
    pending = []

    def declare():
        nonlocal scale, in_header
        if not pending:
            return
        head = pending[0]
        if head == "$timescale":
            scale = _timescale(" ".join(pending[1:]))
        elif head == "$var" and len(pending) >= 5:
            ids[pending[3]] = pending[4]
        elif head == "$enddefinitions":
            in_header = False
        pending.clear()

    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for line in f:
            if in_header:
                for token in line.split():
                    if token == "$end":
                        declare()
                        if not in_header:
                            break
                    else:
                        pending.append(token)
                if in_header:
                    continue
                if scale is None:
                    raise CaptureError(f"{path}: no $timescale in the header")
                continue

            line = line.strip()
            if not line:
                continue
            if line.startswith("#"):
                now = int(line[1:]) * scale
                continue
            value, ident = line[0], line[1:].strip()
            if value not in "01" or ident not in ids:
                # Vectors, x and z. Not timestamps.
                continue
            name = ids[ident]
            high = value == "1"
            # The dump at the first timestamp is the state the capture *opened*
            # in, not an event that happened, and a VCD may restate a value that
            # has not changed. Reporting either as a transition puts a phantom
            # edge on every channel at t=0 — which pairs, and lands in the
            # distribution D15 turns on.
            was = state.get(ident)
            state[ident] = high
            if was is None or was == high:
                continue
            if channels and name not in channels:
                continue
            edges.append(Edge(now, name, high))

    if in_header:
        raise CaptureError(f"{path}: the header never ended — not a VCD?")
    if channels:
        # Against what the file *declares*, not against what happened to toggle.
        # A channel that stayed flat all run is present and is a finding.
        _require(path, channels, set(ids.values()))
    return edges


def read_csv(path, channels=None):
    """Stream a PulseView CSV export, emitting transitions rather than samples.

    Only for short captures — §5 keeps CSV as a fallback and says why. The first
    value of each channel is not a transition and is not reported as one: it is
    the state the capture began in, which is not an event that happened.
    """
    edges = []
    state = {}
    seen = set()
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        rows = csv.reader(r for r in f if not r.lstrip().startswith(";"))
        try:
            header = [h.strip() for h in next(rows)]
        except StopIteration:
            raise CaptureError(f"{path}: empty capture") from None
        if len(header) < 2:
            raise CaptureError(f"{path}: expected a time column and at least one channel")
        seen.update(header[1:])
        for row in rows:
            if len(row) != len(header):
                continue
            try:
                t = float(row[0])
            except ValueError:
                continue
            for name, cell in zip(header[1:], row[1:]):
                if channels and name not in channels:
                    continue
                cell = cell.strip()
                if cell not in ("0", "1"):
                    continue
                high = cell == "1"
                if name in state and state[name] != high:
                    edges.append(Edge(t, name, high))
                state[name] = high

    if channels:
        _require(path, channels, seen)
    return edges


def _require(path, wanted, seen):
    missing = [c for c in wanted if c not in seen]
    if missing:
        raise CaptureError(
            f"{path}: no channel named {', '.join(missing)}. "
            f"The capture has: {', '.join(sorted(seen)) or 'nothing'}"
        )


def _timescale(text):
    m = re.match(r"\s*(\d+)\s*([a-z]+)\s*$", text.strip())
    if not m:
        raise CaptureError(f"unreadable $timescale {text!r}")
    number, unit = int(m.group(1)), m.group(2)
    if unit not in UNITS:
        raise CaptureError(f"unknown time unit {unit!r}")
    return number * UNITS[unit]
