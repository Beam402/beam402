#!/usr/bin/env python3
"""The reduction script against captures whose answer is already known.

`software.md` §7 puts this script first in the build order precisely because it
can be finished and trusted with no hardware: a synthetic capture carries a
*known* jitter and a *known* asymmetry, so the script's answer can be checked
against the truth rather than against something that looks reasonable.

By **D15** these numbers gate every purchase the project will make. The tests
that matter here are therefore not the ones proving it computes a mean — they
are the ones proving it **refuses to look clean** when the capture is not.

    python3 -m unittest discover bench
"""

import io
import os
import tempfile
import unittest

import capture
import reduce
import synth


def written(**kw):
    """A synthetic capture on disk, cleaned up by the caller."""
    fd, path = tempfile.mkstemp(suffix=".vcd")
    os.close(fd)
    events = synth.build(
        kw.get("passes", 300),
        kw.get("rpm", 2650.0),
        kw.get("jitter_us", 40.0),
        kw.get("asymmetry_us", 0.0),
        kw.get("chord", 0.05),
        kw.get("seed", 1),
    )
    synth.write_vcd(path, events, kw.get("timescale_ns", 1))
    return path


def reduced(path, **kw):
    """Run the reduction over a capture and give back its report and status.

    Through the script's own parser, so the tests exercise the defaults a person
    at the bench will actually get rather than a second set maintained here.
    """
    argv = [
        path,
        "--sensor",
        "SENSOR",
        "--reference",
        "REF",
        "--rpm",
        str(kw.get("rpm", 2650)),
        "--temp-c",
        kw.get("temp", "21.0"),
    ]
    if "window" in kw:
        argv += ["--window", str(kw["window"])]
    args = reduce.parser().parse_args(argv)

    edges = capture.read(path, ["SENSOR", "REF"])
    passes, unpaired = reduce.pair(edges, "SENSOR", "REF", True, True, args.window)
    out = io.StringIO()
    status = reduce.report(args, passes, unpaired, out)
    return out.getvalue(), status, passes, unpaired


class Reduction(unittest.TestCase):
    def setUp(self):
        self.paths = []

    def tearDown(self):
        for p in self.paths:
            os.unlink(p)

    def make(self, **kw):
        path = written(**kw)
        self.paths.append(path)
        return path

    # -- it recovers what was injected ------------------------------------

    def test_it_finds_the_jitter_that_was_put_in(self):
        # 40 us peak-to-peak, uniform, on both edges. The script has to come back
        # with 40 and not with a number that merely looks small.
        path = self.make(jitter_us=40.0, passes=300)
        text, status, passes, unpaired = reduced(path)
        self.assertEqual(unpaired, 0, text)
        self.assertEqual(len(passes), 600, "300 passes is 600 edges")
        deltas = [p.delta for p in passes]
        pp = max(deltas) - min(deltas)
        self.assertAlmostEqual(pp * 1e6, 40.0, delta=3.0)
        self.assertIn("T1  jitter", text)
        self.assertIn("PASS", text)
        self.assertEqual(status, 0)

    def test_it_finds_the_edge_asymmetry_that_was_put_in(self):
        # T2's whole subject: an offset that lands in every ET and cancels
        # nowhere. 120 us on the break edge only.
        path = self.make(jitter_us=10.0, asymmetry_us=120.0)
        text, status, passes, _ = reduced(path)
        breaks = [p.delta for p in passes if p.breaking]
        makes = [p.delta for p in passes if not p.breaking]
        gap = abs(sum(breaks) / len(breaks) - sum(makes) / len(makes))
        self.assertAlmostEqual(gap * 1e6, 120.0, delta=3.0)
        self.assertIn("T2  make/break asymmetry", text)
        self.assertEqual(status, 0, "120 us is inside the 500 us limit")

    def test_an_offset_is_not_charged_to_jitter(self):
        # §1: conflating jitter with offset is "the classic mistake", and pooling
        # the two edge directions commits it. 100 us of real jitter with a 350 us
        # asymmetry pools to ~450 — over T1's limit — while the jitter is well
        # inside it and the offset is well inside T2's. Both must pass, because
        # an offset is calibratable and jitter is not.
        path = self.make(jitter_us=100.0, asymmetry_us=350.0)
        text, status, passes, _ = reduced(path)
        deltas = [p.delta for p in passes]
        self.assertGreater(
            (max(deltas) - min(deltas)) * 1e6, 400.0, "the pooled spread is over"
        )
        self.assertNotIn("FAIL", text)
        self.assertEqual(status, 0)

    def test_a_sensor_that_fails_is_reported_as_failing(self):
        # The one outcome D15 actually turns on. 900 us peak-to-peak is more than
        # twice the threshold, and a script that rounded that to PASS would cost
        # the project a batch order.
        path = self.make(jitter_us=900.0)
        text, status, _, _ = reduced(path)
        self.assertIn("FAIL", text)
        self.assertNotEqual(status, 0)

    def test_an_asymmetry_over_the_limit_fails_even_with_low_jitter(self):
        # A rock-steady sensor with a 700 us edge offset still fails T2, because
        # the offset lands in every ET.
        path = self.make(jitter_us=5.0, asymmetry_us=700.0)
        text, status, _, _ = reduced(path)
        self.assertIn("T2", text)
        self.assertIn("FAIL", text)
        self.assertNotEqual(status, 0)

    # -- it refuses to look clean when it should not ----------------------

    def test_unpaired_edges_are_counted_and_shouted_about(self):
        # The failure this script exists to not have. With a window shorter than
        # the real sensor delay nothing pairs, and the answer must be an alarm
        # rather than a tidy distribution over whatever survived.
        path = self.make(jitter_us=20.0)
        text, status, passes, unpaired = reduced(path, window=1e-6)
        self.assertGreater(unpaired, 0)
        self.assertIn("UNPAIRED", text)
        self.assertIn("rig or wiring problem", text)
        self.assertNotEqual(status, 0, "an unpaired capture never exits clean")

    def test_a_missing_temperature_is_named_in_capitals(self):
        # T4 is about drift and cannot be reconstructed from a report that forgot
        # the temperature. The capture is still good; the report is not
        # comparable, and it says so.
        path = self.make()
        text, _, _, _ = reduced(path, temp="?")
        self.assertIn("NOT RECORDED", text)
        self.assertIn("T4 needs the body temperature", text)

    def test_the_speed_and_temperature_reach_the_report(self):
        # §5: every run is reported with its speed and body temperature, so runs
        # from different days compare. A number without them is not a result.
        path = self.make()
        text, _, _, _ = reduced(path, rpm=27, temp="18.5")
        self.assertIn("27 rpm", text)
        self.assertIn("18.5 C", text)

    def test_a_wrong_channel_name_stops_rather_than_guessing(self):
        path = self.make()
        with self.assertRaises(capture.CaptureError) as e:
            capture.read(path, ["D0", "REF"])
        self.assertIn("no channel named D0", str(e.exception))
        self.assertIn("SENSOR", str(e.exception), "and it says what is there")


class Capture(unittest.TestCase):
    def setUp(self):
        self.paths = []

    def tearDown(self):
        for p in self.paths:
            os.unlink(p)

    def test_a_coarse_analyzer_tick_is_honoured(self):
        # A 24 MHz analyzer quantises to ~42 ns, and §5 says that adds ~83 ns of
        # spread by itself. The reader must apply the file's own $timescale
        # rather than assume nanoseconds.
        fd, path = tempfile.mkstemp(suffix=".vcd")
        os.close(fd)
        self.paths.append(path)
        synth.write_vcd(path, synth.build(50, 2650.0, 20.0, 0.0, 0.05, 3), 42)
        edges = capture.read(path, ["SENSOR", "REF"])
        self.assertGreater(len(edges), 100)
        # Fifty passes at 2650 rpm is a bit over a second of capture.
        self.assertGreater(edges[-1].t, 1.0)
        self.assertLess(edges[-1].t, 2.0)

    def test_csv_reports_transitions_and_not_the_first_sample(self):
        # The state a capture opened in is not an event that happened.
        fd, path = tempfile.mkstemp(suffix=".csv")
        os.close(fd)
        self.paths.append(path)
        with open(path, "w", encoding="utf-8") as f:
            f.write("; a PulseView export\n")
            f.write("time,SENSOR,REF\n")
            f.write("0.000000,1,1\n")
            f.write("0.000001,1,1\n")
            f.write("0.000002,0,1\n")
            f.write("0.000003,0,0\n")
            f.write("0.000004,1,0\n")
        edges = capture.read(path, ["SENSOR", "REF"])
        self.assertEqual(len(edges), 3)
        self.assertEqual(edges[0].channel, "SENSOR")
        self.assertFalse(edges[0].high)
        self.assertAlmostEqual(edges[0].t, 0.000002)

    def test_an_unreadable_timescale_stops_the_run(self):
        fd, path = tempfile.mkstemp(suffix=".vcd")
        os.close(fd)
        self.paths.append(path)
        with open(path, "w", encoding="utf-8") as f:
            f.write("$timescale banana $end\n$enddefinitions $end\n")
        with self.assertRaises(capture.CaptureError):
            capture.read(path, ["SENSOR"])


if __name__ == "__main__":
    unittest.main()
