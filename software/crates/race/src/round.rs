//! Registers into a round, with the mapping supplying every word of meaning.
//!
//! This is the pure half of `software.md` §4's diagram. Nothing here reads a
//! clock, opens a file or touches a port: a [`RunBuilder`] is fed the poller's
//! events and hands back a [`Round`], so a recorded session replayed twice gives
//! the same numbers twice — which is the software half of **D01**'s
//! verifiability argument.
//!
//! ## Where a number is allowed to come from
//!
//! Every interval below is **one register from one node, measured on that node's
//! own timer, zeroed by the same start pulse** (**D04**). No time is ever
//! assembled from two clocks:
//!
//! - ET is the finish node's own capture register.
//! - Each split is the register of the node that owns that beam.
//! - Trap speed divides the mapping's laser-measured base by an interval whose
//!   two ends sit on one node and one timer.
//! - Reaction time is the tree's, against **that lane's** green.
//! - The launch margin is one node's view of both pulses on its common timer.
//!
//! ## Absence is data
//!
//! A missing split is never silently dropped and never invented. It arrives as a
//! [`Gap`] carrying *why*, and the rule the whole module serves is: **an ET that
//! is present and timing-valid is a run**, with unavailable intermediate splits
//! printed as "—" and their reason recorded. No ET is no time.

use std::collections::{BTreeMap, BTreeSet};

use beam402_mapping::{Beam, Mapping};
use beam402_poller::Event;
use beam402_protocol::{
    Identity, Lane, PulseObservation, RunRecord, Telemetry, TickDelta, Ticks, Tree,
};

/// Why a beam produced no time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Missing {
    /// The node that owns this beam is not answering.
    NodeSilent,
    /// It restarted and has reported no run since.
    NodeReset,
    /// It answers a `protocol_version` this build does not implement, so it is
    /// never read for timing.
    NodeRefused,
    /// Never identified. Nothing is assumed about a node that has not said what
    /// it is — including its tick rate, which every number here divides by.
    Unidentified,
    /// It answers, and has reported no run for this lane.
    NoRecord,
    /// It reported a run and disowned it: the pulse width proved wrong *after*
    /// the counter had started (**D16**).
    RunInvalidated,
    /// A self-test injection, which must never be read as a race.
    Synthetic,
    /// The run is real and this beam was simply not broken.
    NotSeen,
}

impl core::fmt::Display for Missing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            Missing::NodeSilent => "node not answering",
            Missing::NodeReset => "node restarted",
            Missing::NodeRefused => "unknown protocol version",
            Missing::Unidentified => "node never identified",
            Missing::NoRecord => "no run reported",
            Missing::RunInvalidated => "run disowned: bad pulse width",
            Missing::Synthetic => "self-test, not a race",
            Missing::NotSeen => "beam not broken",
        };
        f.write_str(s)
    }
}

/// A beam the mapping places for this lane that produced no time, and why.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gap {
    pub beam: Beam,
    pub address: u8,
    pub input: u8,
    pub why: Missing,
}

/// One lane's run, in seconds and metres per second.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct LaneRun {
    /// Seconds from **this lane's** green to its launch pulse, on the tree's
    /// clock. Negative is a red light and not a special case.
    pub reaction_s: Option<f64>,
    /// Seconds from the launch pulse to the finish beam.
    pub et_s: Option<f64>,
    /// Every intermediate beam that produced a time.
    pub splits_s: BTreeMap<Beam, f64>,
    pub trap_speed_ms: Option<f64>,
    /// Everything the mapping places for this lane that produced nothing.
    pub gaps: Vec<Gap>,
}

impl LaneRun {
    /// The agreed rule: an ET that exists and is timing-valid is a run, even if
    /// intermediate splits are missing. No ET is no time.
    pub fn has_time(&self) -> bool {
        self.et_s.is_some()
    }

    pub fn is_red(&self) -> bool {
        self.reaction_s.is_some_and(|r| r < 0.0)
    }

    pub fn trap_speed_kmh(&self) -> Option<f64> {
        self.trap_speed_ms.map(|v| v * 3.6)
    }

    pub fn gap(&self, beam: Beam) -> Option<Missing> {
        self.gaps.iter().find(|g| g.beam == beam).map(|g| g.why)
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Round {
    lanes: [Option<LaneRun>; 2],
    /// `t_pulse_l2 − t_pulse_l1` in seconds, from the node the mapping names as
    /// the margin source. Negative means lane 2 left first — handicap included,
    /// because the handicap *is* part of the difference between the two pulses.
    pub launch_margin_s: Option<f64>,
    pub tree: Option<Tree>,
}

impl Round {
    pub fn lane(&self, lane: Lane) -> Option<&LaneRun> {
        self.lanes[lane.ord() as usize].as_ref()
    }

    /// Place a lane's run. Public because a round is also assembled when a
    /// session log is replayed, not only when a bus is live.
    pub fn set_lane(&mut self, lane: Lane, run: LaneRun) {
        self.lanes[lane.ord() as usize] = Some(run);
    }

    /// Which lane's tire left the stage beam first, on the tree's own clock.
    ///
    /// Needed by the "first or worst" rule and only interesting under a
    /// handicap, where the two greens are seconds apart: there, the driver with
    /// the *smaller* red light can easily be the one who left first, because
    /// their tree ran first. Comparing the two reds directly would hand the
    /// round to the wrong car.
    ///
    /// Both terms are the tree's registers (**D04**), and the subtraction wraps
    /// because the tick counter does.
    pub fn first_away(&self) -> Option<Lane> {
        let tree = self.tree?;
        let at = |lane: Lane| {
            tree.t_green(lane)
                .wrapping_add(tree.reaction_time(lane).0 as u32)
        };
        let delta = at(Lane::L2).wrapping_sub(at(Lane::L1)) as i32;
        match delta {
            0 => None,
            d if d > 0 => Some(Lane::L1),
            _ => Some(Lane::L2),
        }
    }

    /// Seconds by which lane 2 crossed the finish line after lane 1 — **D20**'s
    /// `(pulse₂ − pulse₁) + ET₂ − ET₁`. Negative means lane 2 won the stripe.
    ///
    /// Crossing order decides races and ET alone cannot recover it: in a bracket
    /// the quicker ET usually belongs to the car that left last.
    pub fn finish_margin_s(&self) -> Option<f64> {
        let launch = self.launch_margin_s?;
        let et1 = self.lane(Lane::L1)?.et_s?;
        let et2 = self.lane(Lane::L2)?.et_s?;
        Some(launch + et2 - et1)
    }
}

/// The seam between the bus and the numbers.
pub struct RunBuilder<'m> {
    mapping: &'m Mapping,
    identity: BTreeMap<u8, Identity>,
    records: BTreeMap<(u8, u8), RunRecord>,
    pulses: BTreeMap<u8, PulseObservation>,
    telemetry: BTreeMap<u8, Telemetry>,
    tree: Option<Tree>,
    silent: BTreeSet<u8>,
    reset: BTreeSet<u8>,
    refused: BTreeSet<u8>,
}

impl<'m> RunBuilder<'m> {
    pub fn new(mapping: &'m Mapping) -> Self {
        RunBuilder {
            mapping,
            identity: BTreeMap::new(),
            records: BTreeMap::new(),
            pulses: BTreeMap::new(),
            telemetry: BTreeMap::new(),
            tree: None,
            silent: BTreeSet::new(),
            reset: BTreeSet::new(),
            refused: BTreeSet::new(),
        }
    }

    /// Forget the last round's records, keeping what the bus itself established —
    /// identities, silence, telemetry. Called between rounds.
    pub fn clear_round(&mut self) {
        self.records.clear();
        self.pulses.clear();
        self.tree = None;
        self.reset.clear();
    }

    pub fn apply(&mut self, event: &Event) {
        match *event {
            Event::Identified { address, identity } => {
                self.identity.insert(address, identity);
                self.silent.remove(&address);
                self.refused.remove(&address);
            }
            Event::Unsupported { address, .. } => {
                self.refused.insert(address);
            }
            Event::Silent { address, .. } => {
                self.silent.insert(address);
            }
            Event::Returned { address } => {
                self.silent.remove(&address);
            }
            // A restart does not retroactively spoil a record already read: that
            // one was latched, carried a live generation, and was fetched before
            // the node went down. What it means is that anything *not* yet read
            // is gone, and this is the reason to print against it.
            Event::Reset { address, .. } => {
                self.reset.insert(address);
            }
            Event::Run {
                address,
                lane,
                record,
            } => {
                self.records.insert((address, lane.ord() as u8), record);
                self.reset.remove(&address);
            }
            Event::Pulse {
                address,
                observation,
            } => {
                self.pulses.insert(address, observation);
            }
            Event::Tree { tree, .. } => self.tree = Some(tree),
            Event::Telemetry { address, telemetry } => {
                self.telemetry.insert(address, telemetry);
            }
            // Digests drive staging, statuses drive the panel, and neither is a
            // timing source.
            _ => {}
        }
    }

    /// A node's effective tick rate, its measured crystal deviation applied.
    ///
    /// **D13**: the correction is a passport, not a job — it belongs to the board
    /// by MAC and is applied by the master, never by the node. A node reports
    /// ticks; interpreting them is always this side of the wire.
    fn hz(&self, address: u8) -> Option<f64> {
        let id = self.identity.get(&address)?;
        let nominal = id.tick_hz as f64;
        let ppm = self
            .mapping
            .node(address)
            .and_then(|n| n.crystal_ppm)
            .unwrap_or(0.0);
        Some(nominal * (1.0 + ppm / 1_000_000.0))
    }

    /// Microseconds to subtract from an interval measured on this input, if the
    /// mapping carries a temperature row for it and a bracket temperature is
    /// known.
    ///
    /// The coefficient comes from the file and the file is empty until **T4**
    /// finds a drift stable enough to calibrate (**D19**), so this is normally
    /// zero. The path exists because the alternative — deciding after T4 how to
    /// apply a number the mapping already has a place for — is how corrections
    /// end up applied twice or not at all.
    fn temperature_us(&self, address: u8, input: u8) -> f64 {
        let Some(id) = self.identity.get(&address) else {
            return 0.0;
        };
        let Some(row) = self
            .mapping
            .temperature_corrections()
            .iter()
            .find(|c| c.mac.0 == id.mac && c.input == input)
        else {
            return 0.0;
        };
        let Some(t) = self.telemetry.get(&address) else {
            return 0.0;
        };
        let Some(raw) = t.temp_bracket.get(input as usize) else {
            return 0.0;
        };
        let celsius = *raw as f64 / 10.0;
        (celsius - row.ref_c) * row.us_per_c
    }

    fn seconds(&self, address: u8, input: u8, t: Ticks) -> Option<f64> {
        let hz = self.hz(address)?;
        Some(t.0 as f64 / hz - self.temperature_us(address, input) / 1_000_000.0)
    }

    /// Why this address can produce nothing, or `None` when it can.
    fn unusable(&self, address: u8) -> Option<Missing> {
        if self.refused.contains(&address) {
            Some(Missing::NodeRefused)
        } else if self.silent.contains(&address) {
            Some(Missing::NodeSilent)
        } else if !self.identity.contains_key(&address) {
            Some(Missing::Unidentified)
        } else {
            None
        }
    }

    /// This beam's time, or the reason there is none.
    fn beam_time(&self, address: u8, input: u8, lane: Lane) -> Result<f64, Missing> {
        if let Some(why) = self.unusable(address) {
            return Err(why);
        }
        let Some(record) = self.records.get(&(address, lane.ord() as u8)) else {
            return Err(if self.reset.contains(&address) {
                Missing::NodeReset
            } else {
                Missing::NoRecord
            });
        };
        if !record.is_race() {
            return Err(Missing::Synthetic);
        }
        if !record.is_timing_valid() {
            return Err(Missing::RunInvalidated);
        }
        let capture = record.inputs.get(input as usize).ok_or(Missing::NotSeen)?;
        let t = capture.break_at().ok_or(Missing::NotSeen)?;
        self.seconds(address, input, t).ok_or(Missing::Unidentified)
    }

    pub fn round(&self) -> Round {
        let mut round = Round {
            tree: self.tree,
            ..Round::default()
        };

        let source = self.mapping.margin.source_address;
        if let Some(p) = self.pulses.get(&source) {
            if let (Some(TickDelta(m)), Some(hz)) = (p.launch_margin(), self.hz(source)) {
                round.launch_margin_s = Some(m as f64 / hz);
            }
        }

        for lane in self.mapping.declared_lanes() {
            round.lanes[lane.ord() as usize] = Some(self.lane_run(lane));
        }
        round
    }

    fn lane_run(&self, lane: Lane) -> LaneRun {
        let mut run = LaneRun::default();

        for site in self.mapping.sites().filter(|s| s.lane == lane) {
            // Pre-stage and the guard beam are staging instruments, not splits:
            // `architecture.md` §6 gives them no capture channel, so asking them
            // for a time and reporting its absence would be noise.
            if !site.beam.is_timed() {
                continue;
            }
            match self.beam_time(site.address, site.input, lane) {
                Ok(seconds) => {
                    if site.beam == Beam::Finish {
                        run.et_s = Some(seconds);
                    } else {
                        run.splits_s.insert(site.beam, seconds);
                    }
                }
                Err(why) => run.gaps.push(Gap {
                    beam: site.beam,
                    address: site.address,
                    input: site.input,
                    why,
                }),
            }
        }

        // Both ends on one node and one timer, so the interval closes without a
        // cross-clock subtraction. The base is laser-measured (§2: 5 cm of error
        // is 0.25 % of speed, which dwarfs the electronics).
        if let (Some(base), Some(entry), Some(exit)) = (
            self.mapping.geometry.trap_base,
            run.splits_s.get(&Beam::TrapEntry).copied(),
            run.splits_s.get(&Beam::TrapExit).copied(),
        ) {
            if exit > entry {
                run.trap_speed_ms = Some(base / (exit - entry));
            }
        }

        // The tree's clock, not a node's: both terms of the subtraction are the
        // tree's own registers (§5), which is what makes this the one number in
        // the system allowed to be a difference at all.
        if let (Some(tree), Some(hz)) = (self.tree, self.tree_hz()) {
            run.reaction_s = Some(tree.reaction_time(lane).0 as f64 / hz);
        }
        run
    }

    /// The tree's own tick rate. It is a device on the bus like any other, so its
    /// rate is read from its identity rather than assumed.
    fn tree_hz(&self) -> Option<f64> {
        self.identity
            .iter()
            .find(|(_, id)| id.device_class == beam402_protocol::DeviceClass::TreeModule)
            .map(|(addr, _)| *addr)
            .and_then(|addr| self.hz(addr))
    }
}
