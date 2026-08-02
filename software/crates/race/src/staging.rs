//! Where the cars are, and what the master should do about it.
//!
//! Everything from "a car rolls toward the line" to "the round is over" lives
//! here, and it is a pure function like the rest of the crate: beam states in,
//! [`Action`]s out. The runtime executes the actions against the poller. That
//! split is not tidiness — it is what makes a round replayable, and it is why
//! this module can be finished and trusted before a beam exists.
//!
//! ## Reading the line
//!
//! Three beams per lane, and under **D17** a *set* bit means the beam is
//! **intact**, so a car is where the zeroes are:
//!
//! | pre-stage | stage | guard | meaning |
//! |---|---|---|---|
//! | intact | intact | — | nothing at the line |
//! | broken | intact | — | pre-staged: the tire is ~7 in (178 mm) short |
//! | broken | broken | intact | staged: the tire is on the line |
//! | intact | broken | intact | deep-staged: rolled past the pre-stage beam |
//! | — | broken | broken | **bodywork**, not a tire |
//!
//! The last row is the guard beam earning its place. It sits 13 3/8 in (340 mm)
//! downtrack of the stage beam, further than a tire's footprint, so the two
//! cannot be broken by the same tire — a splitter or a front lip can break both,
//! and a system without the guard would call that "staged" and start a race
//! against a car that is not on the line (`architecture.md` §2).
//!
//! ## What it will not do
//!
//! It does not decide the winner — that is [`outcome`](crate::outcome) — and it
//! does not know how long anything takes in real time. Durations arrive through
//! [`Staging::tick`], so the same round replays identically against a wall clock
//! and against a simulator.

use std::collections::BTreeSet;

use beam402_mapping::{Beam, Mapping};
use beam402_poller::Event;
use beam402_protocol::{Digest, Lamp, LampFlags, Lane};

/// Where one car is, relative to the starting line.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Position {
    #[default]
    Away,
    /// Pre-stage broken, stage intact.
    PreStaged,
    /// Both broken: the tire is on the line and the run may begin.
    Staged,
    /// Rolled past the pre-stage beam. Legal in some classes and banned in
    /// others, so whether it blocks the round is configuration.
    Deep,
    /// Stage and guard broken together. Not a tire, and not a car that is ready.
    Bodywork,
    /// The start node for this lane is not answering, so nothing is known. Not
    /// the same as `Away`, and the difference matters: one is a car that has not
    /// arrived, the other is a system that cannot see.
    Unknown,
}

impl Position {
    /// Ready for the tree, whatever the class thinks of how it got there.
    pub const fn is_on_the_line(self) -> bool {
        matches!(self, Position::Staged | Position::Deep)
    }
}

/// How the round is going.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Nobody at the line.
    Idle,
    /// At least one car is at the line, and the round is not startable yet.
    Staging,
    /// Both lanes on the line and held there for the settle time. The tree may
    /// be armed.
    Ready,
    /// Armed. The tree owns the sequence from here, including AutoStart's delay.
    Armed,
    /// The bus is quiet across the launch (`architecture.md` §3).
    Quiet,
    /// Polling again, watching the run arrive.
    Running,
    /// Every lane that started has finished, or the round ran out of patience.
    Complete,
    /// The operator has to look at something before this round can start.
    Blocked(Blocked),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Blocked {
    /// Stage and guard broken together: bodywork on the beam.
    Bodywork(Lane),
    /// Deep staged in a class that does not allow it.
    DeepStaging(Lane),
    /// The start node cannot be seen, so "staged" cannot be established.
    StartNodeUnseen(Lane),
}

impl core::fmt::Display for Blocked {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Blocked::Bodywork(l) => {
                write!(f, "lane {}: bodywork on the stage beam", l.number())
            }
            Blocked::DeepStaging(l) => write!(f, "lane {}: deep staged", l.number()),
            Blocked::StartNodeUnseen(l) => {
                write!(f, "lane {}: start node not answering", l.number())
            }
        }
    }
}

/// What the runtime should do next. Nothing here touches a bus; executing these
/// is the impure layer's job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Push the four staging bits to the tree with `tree_staging`.
    ShowStaging(LampFlags),
    /// Arm the tree: `tree_handicap` per lane that owes time, then `tree_arm`.
    Arm,
    /// Stop transmitting.
    Quiet,
    /// Resume polling and collect what the nodes latched.
    Collect,
    /// Give up on this round; the operator decides what happens to it.
    Abandon,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// How long both cars must hold on the line before the tree may be armed.
    /// Prevents arming on a car that is rolling through.
    pub settle_ms: u64,
    /// How long the bus stays silent after the arm. It has to cover AutoStart's
    /// bound plus the cascade plus a driver's reaction, because during it the
    /// master is blind on purpose and cannot notice that the green has gone.
    pub quiet_ms: u64,
    /// How long to wait for both cars to finish before abandoning the round.
    /// A car that stops on the track never produces an ET, and something has to
    /// end the round.
    pub round_timeout_ms: u64,
    /// Whether the class permits deep staging.
    pub deep_staging: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            settle_ms: 500,
            // 700 ms of AutoStart bound, a 1.5 s standard cascade, and a second
            // of slack for a driver who is not quick.
            quiet_ms: 3_500,
            round_timeout_ms: 60_000,
            deep_staging: false,
        }
    }
}

/// The staging machine.
pub struct Staging<'m> {
    mapping: &'m Mapping,
    cfg: Config,
    phase: Phase,
    position: [Position; 2],
    lamps: LampFlags,
    /// Milliseconds spent in a phase that ends on time rather than on evidence.
    elapsed_ms: u64,
    /// The armed handicap, which the quiet window has to outlast.
    spot_ms: u64,
    /// Lanes whose finish beam has reported a run since the arm.
    finished: BTreeSet<u8>,
    /// Addresses whose digest is currently unreadable.
    silent: BTreeSet<u8>,
    digests: [Option<Digest>; 64],
}

impl<'m> Staging<'m> {
    pub fn new(mapping: &'m Mapping) -> Self {
        Staging::with_config(mapping, Config::default())
    }

    pub fn with_config(mapping: &'m Mapping, cfg: Config) -> Self {
        Staging {
            mapping,
            cfg,
            phase: Phase::Idle,
            position: [Position::Unknown; 2],
            lamps: LampFlags::from_bits(0),
            elapsed_ms: 0,
            spot_ms: 0,
            finished: BTreeSet::new(),
            silent: BTreeSet::new(),
            digests: [None; 64],
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn position(&self, lane: Lane) -> Position {
        self.position[lane.ord() as usize]
    }

    pub fn lamps(&self) -> LampFlags {
        self.lamps
    }

    /// The operator has seen whatever was wrong and cleared the round.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.elapsed_ms = 0;
        self.finished.clear();
    }

    /// A bus event. Returns whatever the runtime should do about it.
    pub fn apply(&mut self, event: &Event) -> Vec<Action> {
        match *event {
            Event::Digest { address, digest } => {
                if (address as usize) < self.digests.len() {
                    self.digests[address as usize] = Some(digest);
                }
                self.silent.remove(&address);
            }
            Event::Silent { address, .. } => {
                self.silent.insert(address);
                if (address as usize) < self.digests.len() {
                    self.digests[address as usize] = None;
                }
            }
            Event::Returned { address } => {
                self.silent.remove(&address);
            }
            // A car is done when its **finish beam** is in the record — not when
            // the finish node's record merely changed. Every device observes both
            // start pulses (**D24**), so every device's generation moves at the
            // launch; treating that as "finished" ends the round on the starting
            // line, which is exactly what the first slip this printed did.
            Event::Run {
                address,
                lane,
                record,
            } => {
                if let Some(site) = self.mapping.site(lane, Beam::Finish) {
                    let crossed = record
                        .inputs
                        .get(site.input as usize)
                        .is_some_and(|c| c.break_at().is_some());
                    if site.address == address && crossed {
                        self.finished.insert(lane.number());
                    }
                }
            }
            _ => {}
        }
        self.reread();
        self.advance()
    }

    /// Time passing. The one input that is not a bus event, kept separate so it
    /// can be virtual in a test and real in the field without either knowing.
    pub fn tick(&mut self, elapsed_ms: u64) -> Vec<Action> {
        self.elapsed_ms += elapsed_ms;
        self.advance()
    }

    /// Whether the tree may be armed right now.
    pub fn is_ready(&self) -> bool {
        self.phase == Phase::Ready
    }

    /// The runtime armed the tree, with the handicap it armed it with.
    ///
    /// The handicap is needed here and nowhere else in this module: the quiet
    /// window has to cover the **last** launch, and in a bracket that is seconds
    /// after the first. A window sized for a heads-up start leaves the master
    /// transmitting through the second car's launch, which is the one moment
    /// `architecture.md` §3 exists to keep the bus out of.
    pub fn armed(&mut self, handicap_ms: [u16; 2]) -> Vec<Action> {
        if self.phase != Phase::Ready {
            return Vec::new();
        }
        self.phase = Phase::Armed;
        self.elapsed_ms = 0;
        self.spot_ms = handicap_ms[0].max(handicap_ms[1]) as u64;
        self.finished.clear();
        // The quiet window opens with the arm rather than with the green,
        // because the master cannot see the green: the launch is the noisiest
        // instant the system has, and it is also the one it must not be
        // transmitting through.
        self.phase = Phase::Quiet;
        vec![Action::Quiet]
    }

    /// Read every lane's position out of the digests currently held.
    fn reread(&mut self) {
        for lane in Lane::ALL {
            self.position[lane.ord() as usize] = self.read_lane(lane);
        }
    }

    fn read_lane(&self, lane: Lane) -> Position {
        let Some(stage) = self.mapping.site(lane, Beam::Stage) else {
            return Position::Unknown;
        };
        let Some(digest) = self.digests[stage.address as usize] else {
            return Position::Unknown;
        };

        // Every beam that decides a position must live on the node the stage
        // beam lives on. Split across two nodes they would be read a poll cycle
        // apart, and "staged" would flicker.
        let broken = |beam: Beam| {
            self.mapping
                .site(lane, beam)
                .filter(|s| s.address == stage.address)
                .map(|s| digest.beam_broken(s.input))
        };

        let on_stage = digest.beam_broken(stage.input);
        // The guard beam is a veto, not a position: 340 mm downtrack of the
        // stage beam is further than a tire reaches, so both broken is a
        // splitter and the stage reading means nothing.
        if on_stage && broken(Beam::Guard) == Some(true) {
            return Position::Bodywork;
        }
        match (broken(Beam::Prestage).unwrap_or(false), on_stage) {
            (true, true) => Position::Staged,
            (false, true) => Position::Deep,
            (true, false) => Position::PreStaged,
            (false, false) => Position::Away,
        }
    }

    /// Lamps follow the beams directly. Deep staging shows stage without
    /// pre-stage, which is exactly what a driver deep-staging expects to see.
    fn lamps_for(&self) -> LampFlags {
        let mut lamps = LampFlags::from_bits(0);
        for lane in Lane::ALL {
            let ord = lane.ord();
            let (pre, stage) = match self.position(lane) {
                Position::PreStaged => (true, false),
                Position::Staged => (true, true),
                Position::Deep => (false, true),
                // Bodywork lights nothing: the driver must see that the system
                // does not consider them staged.
                Position::Away | Position::Bodywork | Position::Unknown => (false, false),
            };
            lamps = lamps
                .set(Lamp::Prestage, ord, pre)
                .set(Lamp::Stage, ord, stage);
        }
        lamps
    }

    fn blocking(&self) -> Option<Blocked> {
        for lane in self.mapping.declared_lanes() {
            match self.position(lane) {
                Position::Bodywork => return Some(Blocked::Bodywork(lane)),
                Position::Deep if !self.cfg.deep_staging => {
                    return Some(Blocked::DeepStaging(lane))
                }
                Position::Unknown => return Some(Blocked::StartNodeUnseen(lane)),
                _ => {}
            }
        }
        None
    }

    fn all_on_the_line(&self) -> bool {
        self.mapping
            .declared_lanes()
            .all(|l| self.position(l).is_on_the_line())
    }

    fn advance(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();

        // Lamps track the beams in every phase before the arm. Once the tree is
        // running its own sequence they are its business.
        if matches!(
            self.phase,
            Phase::Idle | Phase::Staging | Phase::Ready | Phase::Blocked(_)
        ) {
            let want = self.lamps_for();
            if want != self.lamps {
                self.lamps = want;
                actions.push(Action::ShowStaging(want));
            }
        }

        match self.phase {
            Phase::Idle | Phase::Staging | Phase::Ready | Phase::Blocked(_) => {
                // The settle timer measures time *held* on the line, so anything
                // that takes a car off it starts the measurement again. Resetting
                // only on a phase change would let a car crossing the beams on
                // its way to the water box bank the time it spent passing, and
                // arm the next pair early.
                if !self.all_on_the_line() || self.blocking().is_some() {
                    self.elapsed_ms = 0;
                }
                let was = self.phase;
                self.phase = self.pre_start_phase();
                if self.phase == Phase::Ready && was != Phase::Ready {
                    actions.push(Action::Arm);
                }
            }
            Phase::Armed => {}
            Phase::Quiet => {
                if self.elapsed_ms >= self.cfg.quiet_ms + self.spot_ms {
                    self.phase = Phase::Running;
                    self.elapsed_ms = 0;
                    actions.push(Action::Collect);
                }
            }
            Phase::Running => {
                let expected = self.mapping.declared_lanes().count();
                if self.finished.len() >= expected {
                    self.phase = Phase::Complete;
                } else if self.elapsed_ms >= self.cfg.round_timeout_ms {
                    self.phase = Phase::Complete;
                    actions.push(Action::Abandon);
                }
            }
            Phase::Complete => {}
        }
        actions
    }

    /// The phase this position implies, before any timer is considered.
    fn pre_start_phase(&self) -> Phase {
        if let Some(why) = self.blocking() {
            return Phase::Blocked(why);
        }
        if !self.all_on_the_line() {
            let anybody = self
                .mapping
                .declared_lanes()
                .any(|l| self.position(l) != Position::Away);
            return if anybody { Phase::Staging } else { Phase::Idle };
        }
        // Both on the line. Ready only once they have held it.
        if self.phase == Phase::Ready || self.elapsed_ms >= self.cfg.settle_ms {
            Phase::Ready
        } else {
            Phase::Staging
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_protocol::words::Generation;
    use beam402_protocol::{EdgeFlags, InputCapture, RunFlags, RunRecord, StatusFlags};
    use beam402_sim::reference::{venue, START_L1, START_L2};

    /// Lane 1's start node carries inputs 0 = pre-stage, 1 = stage, 2 = guard;
    /// lane 2's node the same. Under **D17** a set bit is an *intact* beam, so a
    /// car is described by what it breaks.
    fn beams(address: u8, broken: &[u8]) -> Event {
        let mut state = 0b111u16;
        for b in broken {
            state &= !(1 << b);
        }
        Event::Digest {
            address,
            digest: Digest {
                run_gen_l1: Generation::NEVER,
                run_gen_l2: Generation::NEVER,
                status: StatusFlags::from_bits(0),
                input_state: state,
            },
        }
    }

    /// A record from the finish node with that lane's finish beam actually in
    /// it. The empty record a launch produces is not this, and the difference is
    /// what keeps a round from ending on the starting line.
    fn crossed_the_finish(m: &Mapping, lane: Lane) -> Event {
        let site = m.site(lane, Beam::Finish).unwrap();
        let mut record = RunRecord {
            flags: RunFlags::from_bits(1), // valid
            ..RunRecord::default()
        };
        record.inputs[site.input as usize] =
            InputCapture::new(2, EdgeFlags::from_bits(0b11), 833_000_000, 834_600_000);
        Event::Run {
            address: site.address,
            lane,
            record,
        }
    }

    const PRESTAGE: u8 = 0;
    const STAGE: u8 = 1;
    const GUARD: u8 = 2;

    fn both_away(s: &mut Staging) {
        s.apply(&beams(START_L1, &[]));
        s.apply(&beams(START_L2, &[]));
    }

    /// Roll both cars onto the line and hold them there long enough to arm.
    fn stage_both(s: &mut Staging) -> Vec<Action> {
        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));
        s.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        s.tick(600)
    }

    #[test]
    fn a_car_rolling_in_lights_prestage_then_stage() {
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        assert_eq!(s.phase(), Phase::Idle);

        let acts = s.apply(&beams(START_L1, &[PRESTAGE]));
        assert_eq!(s.position(Lane::L1), Position::PreStaged);
        assert_eq!(s.phase(), Phase::Staging);
        assert_eq!(
            acts,
            vec![Action::ShowStaging(LampFlags::from_bits(0).set(
                Lamp::Prestage,
                0,
                true
            ))]
        );

        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));
        assert_eq!(s.position(Lane::L1), Position::Staged);
        // One lane is not a race.
        assert_eq!(s.phase(), Phase::Staging);
    }

    #[test]
    fn bodywork_on_the_stage_beam_is_not_a_staged_car() {
        // The guard beam earning its place. 340 mm downtrack of the stage beam
        // is further than a tire reaches, so both broken is a splitter — and a
        // system without the guard would call this staged and run a race against
        // a car that is not on the line.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);

        s.apply(&beams(START_L1, &[PRESTAGE, STAGE, GUARD]));
        assert_eq!(s.position(Lane::L1), Position::Bodywork);
        assert!(!s.position(Lane::L1).is_on_the_line());

        s.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        let held = s.tick(5_000);
        assert_eq!(s.phase(), Phase::Blocked(Blocked::Bodywork(Lane::L1)));
        assert!(
            !held.contains(&Action::Arm),
            "no amount of waiting turns bodywork into a staged car"
        );

        // And the driver is shown nothing, so the disagreement is visible from
        // the seat rather than only on the operator's screen.
        assert!(!s.lamps().lit(Lamp::Stage, 0));
    }

    #[test]
    fn deep_staging_is_a_class_rule_and_not_a_fault() {
        let m = venue();

        let mut strict = Staging::new(&m);
        both_away(&mut strict);
        strict.apply(&beams(START_L1, &[STAGE]));
        strict.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        assert_eq!(strict.position(Lane::L1), Position::Deep);
        strict.tick(5_000);
        assert_eq!(
            strict.phase(),
            Phase::Blocked(Blocked::DeepStaging(Lane::L1))
        );

        let mut permissive = Staging::with_config(
            &m,
            Config {
                deep_staging: true,
                ..Config::default()
            },
        );
        both_away(&mut permissive);
        permissive.apply(&beams(START_L1, &[STAGE]));
        permissive.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        let acts = permissive.tick(600);
        assert_eq!(permissive.phase(), Phase::Ready);
        assert!(acts.contains(&Action::Arm));

        // And the lamps say what the driver did: stage without pre-stage.
        assert!(permissive.lamps().lit(Lamp::Stage, 0));
        assert!(!permissive.lamps().lit(Lamp::Prestage, 0));
    }

    #[test]
    fn the_tree_is_not_armed_until_both_cars_have_held_the_line() {
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);

        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));
        let acts = s.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        assert_eq!(s.phase(), Phase::Staging, "both on the line, none settled");
        assert!(!acts.contains(&Action::Arm));

        let acts = s.tick(499);
        assert!(!acts.contains(&Action::Arm));
        let acts = s.tick(1);
        assert_eq!(s.phase(), Phase::Ready);
        assert!(acts.contains(&Action::Arm));
    }

    #[test]
    fn a_car_rolling_through_does_not_bank_the_time_it_spent_passing() {
        // Without this a car that crosses the beams on its way to the water box
        // contributes to the settle timer, and the next pair is armed early.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);

        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));
        s.apply(&beams(START_L2, &[PRESTAGE, STAGE]));
        s.tick(400);
        // Lane 1 rolls out again.
        s.apply(&beams(START_L1, &[]));
        assert_eq!(s.phase(), Phase::Staging);
        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));

        let acts = s.tick(400);
        assert!(
            !acts.contains(&Action::Arm),
            "the 400 ms before the roll-out must not count"
        );
        assert!(s.tick(200).contains(&Action::Arm));
    }

    #[test]
    fn a_silent_start_node_is_not_an_empty_lane() {
        // The difference between "no car has arrived" and "the system cannot
        // see" is the difference between waiting and starting a race blind.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        s.apply(&beams(START_L1, &[PRESTAGE, STAGE]));
        s.apply(&beams(START_L2, &[PRESTAGE, STAGE]));

        s.apply(&Event::Silent {
            address: START_L2,
            error: beam402_bus::BusError::Timeout,
        });
        assert_eq!(s.position(Lane::L2), Position::Unknown);
        s.tick(5_000);
        assert_eq!(
            s.phase(),
            Phase::Blocked(Blocked::StartNodeUnseen(Lane::L2))
        );
    }

    #[test]
    fn the_quiet_window_opens_with_the_arm_because_the_green_cannot_be_seen() {
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        stage_both(&mut s);
        assert_eq!(s.phase(), Phase::Ready);

        assert_eq!(s.armed([0; 2]), vec![Action::Quiet]);
        assert_eq!(s.phase(), Phase::Quiet);

        assert!(s.tick(3_000).is_empty(), "still silent");
        assert_eq!(s.tick(600), vec![Action::Collect]);
        assert_eq!(s.phase(), Phase::Running);
    }

    #[test]
    fn a_launch_is_not_a_finish() {
        // Every device observes both start pulses (**D24**), so every device's
        // generation moves at the launch and the finish node emits a record with
        // nothing in it. Reading that as "the car is done" ends the round on the
        // starting line — which is what the first slip this printed did.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        stage_both(&mut s);
        s.armed([0; 2]);
        s.tick(4_000);

        for lane in Lane::ALL {
            s.apply(&Event::Run {
                address: m.site(lane, Beam::Finish).unwrap().address,
                lane,
                record: RunRecord::default(),
            });
        }
        assert_eq!(s.phase(), Phase::Running, "nobody has crossed anything");
    }

    #[test]
    fn the_quiet_window_outlasts_the_handicap() {
        // A window sized for a heads-up start leaves the master transmitting
        // through the second car's launch — the one moment §3 exists to keep the
        // bus out of.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        stage_both(&mut s);
        s.armed([0, 4_840]);

        // 3.5 s of window plus 4.84 s of spot: the second car has not left yet.
        assert!(s.tick(8_300).is_empty(), "still inside the spot");
        assert_eq!(s.tick(100), vec![Action::Collect]);
    }

    #[test]
    fn a_round_ends_when_both_cars_have_finished() {
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        stage_both(&mut s);
        s.armed([0; 2]);
        s.tick(4_000);

        for lane in Lane::ALL {
            s.apply(&crossed_the_finish(&m, lane));
        }
        assert_eq!(s.phase(), Phase::Complete);
    }

    #[test]
    fn a_car_that_stops_on_the_track_still_ends_the_round() {
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        stage_both(&mut s);
        s.armed([0; 2]);
        s.tick(4_000);

        s.apply(&crossed_the_finish(&m, Lane::L1));
        assert_eq!(s.phase(), Phase::Running, "one car is still out there");

        let acts = s.tick(60_000);
        assert_eq!(acts, vec![Action::Abandon]);
        assert_eq!(s.phase(), Phase::Complete);
    }

    #[test]
    fn lamps_are_written_only_when_they_change() {
        // Two poll hops per lamp change is the budget (`software.md` §4). Writing
        // the same bitmap every cycle would spend it on nothing.
        let m = venue();
        let mut s = Staging::new(&m);
        both_away(&mut s);
        assert!(s.apply(&beams(START_L1, &[PRESTAGE])).len() == 1);
        assert!(s.apply(&beams(START_L1, &[PRESTAGE])).is_empty());
        assert!(s.apply(&beams(START_L1, &[PRESTAGE, STAGE])).len() == 1);
    }
}
