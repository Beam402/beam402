//! # Beam402 — the node simulator
//!
//! A scenario in, register images out, over the same [`Bus`] seam the serial
//! transport will implement (**D26**). The master never learns which side of that
//! seam it is talking to, and it never sees the scenario — if it could, the test
//! would prove nothing.
//!
//! ```text
//! scenario.toml   ground truth: reacted in 0.520 s, finished at 10.412 s
//!       │
//!       ▼   staging edges, launch pulses, beam edges, injected faults
//! NodeCore × N    latching, generations, D16's invalidation, the register image
//!       │
//!       ▼   Bus: one call is one transaction
//! master          must recover the numbers the scenario stated
//! ```
//!
//! ## One simulator, two device classes
//!
//! There is no separate tree simulator. The tree is a device of class 2 — the
//! same register layer plus one block and [`tree::TreeSim`]'s sequence machine.
//! **D07** keeps it off the universal board; nothing keeps it off the universal
//! protocol.
//!
//! ## Virtual time, and why nothing here reads a clock
//!
//! Time is a tick counter the caller advances. Two cars launching 3 ms apart is
//! not reproducible against a wall clock, and it is on **D26**'s list of runs the
//! simulator exists to replay. AutoStart's delay is drawn from the scenario's
//! `seed` for the same reason: without that, "here is the session, replay it, get
//! the same ET" dies in the first round.

pub mod reference;
mod scenario;
pub mod tree;

pub use scenario::{seconds, ticks, Car, Fault, Header, Mode, Rng, Scenario, TreeSetup, TICK_HZ};

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use beam402_bus::{Bus, BusError};
use beam402_mapping::{Beam, Mapping};
use beam402_node_core::{Config, EdgeKind, Event, NodeCore};
use beam402_protocol::blocks::{Block, Command, Opcode};
use beam402_protocol::words::{Millis, Ticks};
use beam402_protocol::Lane;

use tree::{Step, TreeSim};

/// Nominal start-pulse width. The real tolerance is a bench measurement
/// (`architecture.md` §11 #5); this is the value a healthy monostable produces.
const GOOD_PULSE_US: u16 = 5_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    /// Reaches **every** device: under **D24** they all observe both pulses.
    Pulse {
        lane: Lane,
    },
    PulseWidth {
        lane: Lane,
        width_us: u16,
    },
    Edge {
        address: u8,
        lane: Lane,
        input: u8,
        kind: EdgeKind,
    },
    /// Issued the way the master will issue it — through the command block.
    Write {
        address: u8,
        cmd: Command,
    },
    Lamp {
        step: Step,
    },
    /// Staging lamps. **The register map has no opcode for these**: `software.md`
    /// §4 says the master pushes a lamp change for two poll hops, but the command
    /// list has only `tree_lamp_test`. Until that gap is closed the simulator sets
    /// them directly, which is the one place here that is not the real path.
    StagingLamp {
        lane: Lane,
        prestage: bool,
        stage: bool,
    },
    GoSilent {
        address: u8,
    },
    Reboot {
        address: u8,
    },
    Overrun {
        address: u8,
        lane: Lane,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Scheduled {
    at: u64,
    seq: u32,
    action: Action,
}

impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: BinaryHeap is a max-heap and the earliest event must pop first.
        (other.at, other.seq).cmp(&(self.at, self.seq))
    }
}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Device {
    core: NodeCore,
    silent_from: Option<u64>,
    /// When *this* device's capture timer was last zeroed, per lane. Per-device
    /// rather than global on purpose: a node that rebooted never saw the pulse, so
    /// its timer is free-running from boot and the tick it captures is not a split.
    pulse_zero: [Option<u64>; 2],
    booted_at: u64,
}

impl Device {
    fn is_silent(&self, now: u64) -> bool {
        self.silent_from.is_some_and(|t| now >= t)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BuildError {
    /// A scenario names a lane the venue does not declare.
    UnknownLane(u8),
    /// A split names a beam the mapping does not place for that lane. Loud,
    /// because the alternative is a silently missing split.
    UnmappedBeam { lane: u8, beam: Beam },
    /// A fault names an address that is not on the bus.
    UnknownAddress(u8),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::UnknownLane(l) => write!(f, "lane {l} is not declared by the venue"),
            BuildError::UnmappedBeam { lane, beam } => {
                write!(f, "lane {lane} has no {beam} in the mapping file")
            }
            BuildError::UnknownAddress(a) => write!(f, "address {a} is not on the bus"),
        }
    }
}

pub struct Simulator {
    devices: BTreeMap<u8, Device>,
    tree_address: u8,
    tree: TreeSim,
    queue: BinaryHeap<Scheduled>,
    seq: u32,
    now: u64,
    /// Timeline instant of each lane's start pulse — the zero its capture timer
    /// was reset to.
    pulse_at: [Option<u64>; 2],
    green_at: Option<u64>,
    launch_at: [Option<u64>; 2],
    stuck: BTreeSet<(u8, u8)>,
    bad_width: BTreeMap<u8, u16>,
    overruns: Vec<(u8, Lane)>,
    scenario: Scenario,
    chord: u64,
    /// Held, not borrowed, so that [`Bus::write`] can run the tree's sequence
    /// machine. Without it a master could set the tree's command register over
    /// the seam and nothing would happen — the one path **D26** exists to
    /// exercise would be the one path the simulator could not.
    mapping: Mapping,
}

impl Simulator {
    /// Devices come from the **mapping file**, not from the scenario: the same
    /// file the master reads, so the two cannot disagree about what is on the bus.
    /// The scenario adds only what the mapping has no place for — the tree's
    /// address, the runs, and the faults.
    pub fn new(mapping: &Mapping, scenario: Scenario) -> Result<Self, BuildError> {
        let mut devices = BTreeMap::new();
        for node in &mapping.nodes {
            let present = node.inputs.iter().fold(0u16, |m, i| m | (1u16 << i.index));
            let mac = node
                .mac
                .map(|m| m.0)
                .unwrap_or(0x7CDF_A100_0000 | node.address as u64);
            devices.insert(
                node.address,
                Device {
                    core: NodeCore::new(Config::timing_node(node.address, mac, present)),
                    silent_from: None,
                    pulse_zero: [None; 2],
                    booted_at: 0,
                },
            );
        }
        let tree_mac = 0x7CDF_A1FF_0000 | scenario.tree.address as u64;
        devices.insert(
            scenario.tree.address,
            Device {
                core: NodeCore::new(Config::tree(scenario.tree.address, tree_mac)),
                silent_from: None,
                pulse_zero: [None; 2],
                booted_at: 0,
            },
        );

        let chord = ticks(scenario.scenario.chord_ms / 1000.0);
        let mut sim = Simulator {
            devices,
            tree_address: scenario.tree.address,
            tree: TreeSim::new(scenario.tree.mode),
            queue: BinaryHeap::new(),
            seq: 0,
            now: 0,
            pulse_at: [None; 2],
            green_at: None,
            launch_at: [None; 2],
            stuck: BTreeSet::new(),
            bad_width: BTreeMap::new(),
            overruns: Vec::new(),
            scenario,
            chord,
            mapping: mapping.clone(),
        };
        sim.plan()?;
        Ok(sim)
    }

    fn plan(&mut self) -> Result<(), BuildError> {
        let mapping = self.mapping.clone();
        // Faults first, so a stuck beam is known before edges are scheduled.
        for fault in self.scenario.faults.clone() {
            match fault {
                Fault::Silent { address, from_s } => {
                    self.require(address)?;
                    self.push(ticks(from_s), Action::GoSilent { address });
                }
                Fault::Reboot { address, at_s } => {
                    self.require(address)?;
                    self.push(ticks(at_s), Action::Reboot { address });
                }
                Fault::BeamStuck { address, input } => {
                    self.require(address)?;
                    self.stuck.insert((address, input));
                }
                Fault::BadPulseWidth { lane, width_us } => {
                    self.bad_width.insert(lane, width_us);
                }
                Fault::CaptureOverrun { address, lane } => {
                    self.require(address)?;
                    let lane = Lane::from_number(lane).ok_or(BuildError::UnknownLane(lane))?;
                    // Deferred to plan_after_arm: at tick 0 the pulse that starts
                    // the run would clear the flag again, making it a no-op.
                    self.overruns.push((address, lane));
                }
            }
        }

        // Staging: the front tire breaks pre-stage, then stage, and sits in both.
        for car in self.scenario.cars.clone() {
            let lane = Lane::from_number(car.lane).ok_or(BuildError::UnknownLane(car.lane))?;
            let stage_t = ticks(car.stage_at_s);
            if let Some(site) = mapping.site(lane, Beam::Prestage) {
                self.push(
                    stage_t.saturating_sub(ticks(0.4)),
                    Action::Edge {
                        address: site.address,
                        lane,
                        input: site.input,
                        kind: EdgeKind::Break,
                    },
                );
                self.push(
                    stage_t.saturating_sub(ticks(0.4)),
                    Action::StagingLamp {
                        lane,
                        prestage: true,
                        stage: false,
                    },
                );
            }
            let stage = mapping
                .site(lane, Beam::Stage)
                .ok_or(BuildError::UnmappedBeam {
                    lane: car.lane,
                    beam: Beam::Stage,
                })?;
            self.push(
                stage_t,
                Action::Edge {
                    address: stage.address,
                    lane,
                    input: stage.input,
                    kind: EdgeKind::Break,
                },
            );
            self.push(
                stage_t,
                Action::StagingLamp {
                    lane,
                    prestage: true,
                    stage: true,
                },
            );
        }

        // Resolve every split against the mapping now, not at arm time. Deferring
        // it would let a scenario build and then quietly run with one split fewer,
        // which is the failure this whole project refuses.
        for car in self.scenario.cars.clone() {
            let lane = Lane::from_number(car.lane).ok_or(BuildError::UnknownLane(car.lane))?;
            for beam in car.splits.keys() {
                mapping.site(lane, *beam).ok_or(BuildError::UnmappedBeam {
                    lane: car.lane,
                    beam: *beam,
                })?;
            }
        }

        // The master arms; the tree runs it. Issued through the command block so
        // the path is the real one.
        self.push(
            ticks(self.scenario.tree.arm_at_s),
            Action::Write {
                address: self.tree_address,
                cmd: Command {
                    opcode: Opcode::TreeArm,
                    arg0: self.scenario.tree.mode as u16,
                    arg1: self.scenario.tree.random_delay_ms,
                    seq: 1,
                },
            },
        );
        Ok(())
    }

    /// Everything downstream of green, scheduled the moment the sequence is armed
    /// — the drawn delay makes the green instant known, and a red light needs a
    /// launch scheduled *before* green.
    fn plan_after_arm(&mut self, armed_at: u64) -> Result<(), BuildError> {
        let mapping = self.mapping.clone();
        let mut rng = Rng::new(self.scenario.scenario.seed);
        let delay = rng.below(self.scenario.tree.random_delay_ms as u64 + 1);
        let sequence_start = armed_at + ticks(delay as f64 / 1000.0);
        let green = sequence_start + self.tree.cascade_ticks();
        self.green_at = Some(green);

        for step in [Step::Amber1, Step::Amber2, Step::Amber3, Step::Green] {
            self.push(
                sequence_start + self.tree.step_offset(step),
                Action::Lamp { step },
            );
        }

        for car in self.scenario.cars.clone() {
            let lane = Lane::from_number(car.lane).ok_or(BuildError::UnknownLane(car.lane))?;
            let launch = (green as i64 + (car.reaction_s * TICK_HZ as f64).round() as i64)
                .max(armed_at as i64) as u64;
            self.launch_at[lane.ord() as usize] = Some(launch);

            // The tire leaving the stage beam *is* the launch (D16): pre-stage
            // clears first because it sits uptrack, then stage, and the monostable
            // fires from that edge.
            if let Some(site) = mapping.site(lane, Beam::Prestage) {
                self.push(
                    launch.saturating_sub(ticks(0.005)),
                    Action::Edge {
                        address: site.address,
                        lane,
                        input: site.input,
                        kind: EdgeKind::Make,
                    },
                );
            }
            let stage = mapping
                .site(lane, Beam::Stage)
                .ok_or(BuildError::UnmappedBeam {
                    lane: car.lane,
                    beam: Beam::Stage,
                })?;
            self.push(
                launch,
                Action::Edge {
                    address: stage.address,
                    lane,
                    input: stage.input,
                    kind: EdgeKind::Make,
                },
            );
            self.push(launch, Action::Pulse { lane });
            self.push(
                launch + ticks(0.005),
                Action::PulseWidth {
                    lane,
                    width_us: self
                        .bad_width
                        .get(&car.lane)
                        .copied()
                        .unwrap_or(GOOD_PULSE_US),
                },
            );

            for (address, l) in self.overruns.clone() {
                if l == lane {
                    self.push(launch + ticks(0.001), Action::Overrun { address, lane });
                }
            }

            for (beam, at_s) in &car.splits {
                let site = mapping.site(lane, *beam).ok_or(BuildError::UnmappedBeam {
                    lane: car.lane,
                    beam: *beam,
                })?;
                let t = launch + ticks(*at_s);
                self.push(
                    t,
                    Action::Edge {
                        address: site.address,
                        lane,
                        input: site.input,
                        kind: EdgeKind::Break,
                    },
                );
                if !self.stuck.contains(&(site.address, site.input)) {
                    self.push(
                        t + self.chord,
                        Action::Edge {
                            address: site.address,
                            lane,
                            input: site.input,
                            kind: EdgeKind::Make,
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn require(&self, address: u8) -> Result<(), BuildError> {
        if self.devices.contains_key(&address) {
            Ok(())
        } else {
            Err(BuildError::UnknownAddress(address))
        }
    }

    fn push(&mut self, at: u64, action: Action) {
        self.seq += 1;
        self.queue.push(Scheduled {
            at,
            seq: self.seq,
            action,
        });
    }

    pub fn now(&self) -> u64 {
        self.now
    }

    pub fn now_s(&self) -> f64 {
        seconds(self.now)
    }

    pub fn green_at(&self) -> Option<u64> {
        self.green_at
    }

    pub fn launch_at(&self, lane: Lane) -> Option<u64> {
        self.launch_at[lane.ord() as usize]
    }

    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    /// Advance virtual time, applying everything scheduled up to `t`.
    pub fn advance_to(&mut self, t: u64) {
        while let Some(next) = self.queue.peek().copied() {
            if next.at > t {
                break;
            }
            self.queue.pop();
            self.now = next.at;
            self.dispatch(next.action);
        }
        self.now = self.now.max(t);
    }

    pub fn advance_by_s(&mut self, dt: f64) {
        self.advance_to(self.now + ticks(dt));
    }

    /// Run until the timeline is empty.
    pub fn run(&mut self) {
        while let Some(next) = self.queue.peek().copied() {
            self.advance_to(next.at);
        }
    }

    fn dispatch(&mut self, action: Action) {
        let now = self.now;
        match action {
            Action::Pulse { lane } => {
                self.pulse_at[lane.ord() as usize] = Some(now);
                // Every device observes it — that is D24, and it is what puts both
                // pulses on one timer wherever the mapping says the margin lives.
                for dev in self.devices.values_mut() {
                    dev.pulse_zero[lane.ord() as usize] = Some(now);
                    dev.core.apply(Event::Pulse {
                        lane,
                        t: Ticks(now as u32),
                    });
                }
                self.tree.observed_pulse(lane, now);
                self.sync_tree();
            }
            Action::PulseWidth { lane, width_us } => {
                for dev in self.devices.values_mut() {
                    dev.core.apply(Event::PulseWidth { lane, width_us });
                }
            }
            Action::Edge {
                address,
                lane,
                input,
                kind,
            } => {
                if let Some(dev) = self.devices.get_mut(&address) {
                    // Ticks are measured from *this device's* zero. With no pulse
                    // since boot the timer is free-running, so what it captures is
                    // a number, not a split — and the record it lands in carries
                    // generation 0, which is the master's cue to discard it.
                    // Staging is read from `input_state`, never from a record, so
                    // an edge before the first pulse is harmless rather than wrong.
                    let base = dev.pulse_zero[lane.ord() as usize].unwrap_or(dev.booted_at);
                    let t = Ticks(now.saturating_sub(base) as u32);
                    dev.core.apply(Event::Edge {
                        lane,
                        input,
                        t,
                        kind,
                        log_t: Millis((now / (TICK_HZ / 1000)) as u32),
                    });
                }
            }
            Action::Write { address, cmd } => {
                let mut buf = [0u16; Command::LEN as usize];
                cmd.encode(&mut buf).ok();
                // Straight through [`Bus::write`], which is now the only command
                // path there is: the scheduled arm and a master's arm are the
                // same code.
                let _ = self.write(address, Command::ADDR, &buf);
            }
            Action::Lamp { step } => {
                self.tree.light(step, now);
                self.sync_tree();
            }
            Action::StagingLamp {
                lane,
                prestage,
                stage,
            } => {
                self.tree.staged(lane, prestage, stage);
                self.sync_tree();
            }
            Action::GoSilent { address } => {
                if let Some(dev) = self.devices.get_mut(&address) {
                    dev.silent_from = Some(now);
                }
            }
            Action::Reboot { address } => {
                if let Some(dev) = self.devices.get_mut(&address) {
                    dev.core.reboot();
                    dev.pulse_zero = [None; 2];
                    dev.booted_at = now;
                }
            }
            Action::Overrun { address, lane } => {
                if let Some(dev) = self.devices.get_mut(&address) {
                    dev.core.apply(Event::CaptureOverrun { lane });
                }
            }
        }
    }

    fn sync_tree(&mut self) {
        let block = self.tree.block();
        if let Some(dev) = self.devices.get_mut(&self.tree_address) {
            *dev.core.tree_mut() = block;
        }
    }

    fn on_command(&mut self, address: u8, cmd: Command) {
        if address != self.tree_address {
            return;
        }
        match cmd.opcode {
            Opcode::TreeArm => {
                self.tree.arm();
                self.sync_tree();
                let at = self.now;
                // Ignored deliberately: a scenario that names an unmapped beam
                // fails at construction, so this cannot fail here.
                let _ = self.plan_after_arm(at);
            }
            Opcode::TreeAbort => {
                self.tree.abort();
                self.sync_tree();
            }
            _ => {}
        }
    }
}

impl Bus for Simulator {
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError> {
        let now = self.now;
        // An address nobody answers is indistinguishable from a dead node, and
        // that is correct: both surface as silence on the operator panel.
        let dev = self.devices.get(&address).ok_or(BusError::Timeout)?;
        if dev.is_silent(now) {
            return Err(BusError::Timeout);
        }
        let count = out.len() as u16;
        dev.core
            .read(reg, count, out)
            .map_err(|e| BusError::Exception(e as u8))
    }

    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError> {
        let now = self.now;
        let dev = self.devices.get_mut(&address).ok_or(BusError::Timeout)?;
        if dev.is_silent(now) {
            return Err(BusError::Timeout);
        }
        let executed = dev
            .core
            .write(reg, values)
            .map_err(|e| BusError::Exception(e as u8))?;
        // A retry with an unchanged sequence number acknowledges without running
        // the command again (`protocol.md` §2), so the tree must not re-arm on it
        // either.
        if executed && reg == Command::ADDR {
            if let Ok(cmd) = Command::decode(values) {
                self.on_command(address, cmd);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_bus::BusExt;
    use beam402_protocol::blocks::{Digest, PulseObservation, Status, Tree};
    use beam402_protocol::TickDelta;

    use crate::reference::*;

    fn sim(text: &str) -> Simulator {
        Simulator::new(&venue(), scenario(text)).expect("scenario must build against the venue")
    }

    fn finished(text: &str) -> Simulator {
        let mut sim = sim(text);
        sim.run();
        sim
    }

    // -- a clean pair, read the way the master will read it -----------------

    #[test]
    fn the_master_recovers_every_number_the_scenario_stated() {
        let mut sim = finished(&clean_pair());

        // ET is the finish node's own capture register, not an assembly of two
        // clocks. D04 in one read.
        let r1 = sim.run_record(FINISH, Lane::L1).unwrap();
        assert_eq!(r1.inputs[0].break_at(), Some(Ticks(ticks(ET1) as u32)));
        let r2 = sim.run_record(FINISH, Lane::L2).unwrap();
        assert_eq!(r2.inputs[1].break_at(), Some(Ticks(ticks(ET2) as u32)));

        // The 60 ft split, likewise a single register from the node that owns it.
        let s1 = sim.run_record(SIXTY, Lane::L1).unwrap();
        assert_eq!(s1.inputs[0].break_at(), Some(Ticks(ticks(SIXTY1) as u32)));

        // Trap speed = measured base / (exit - entry), both on one node and one
        // timer, so the interval closes without a cross-clock subtraction.
        let t1 = sim.run_record(TRAP, Lane::L1).unwrap();
        let entry = t1.inputs[0].break_at().unwrap().0 as f64;
        let exit = t1.inputs[1].break_at().unwrap().0 as f64;
        let speed_ms = TRAP_BASE_M / ((exit - entry) / TICK_HZ as f64);
        assert!(
            (speed_ms * 3.6 - TRAP_BASE_M / (EXIT1 - ENTRY1) * 3.6).abs() < 0.01,
            "trap speed should follow from the stated splits, got {speed_ms}"
        );

        // Reaction time comes from the tree, on the tree's own clock.
        let tree: Tree = sim.block(TREE).unwrap();
        assert_eq!(tree.reaction_time(Lane::L1), TickDelta(ticks(R1) as i32));
        assert_eq!(tree.reaction_time(Lane::L2), TickDelta(ticks(R2) as i32));
        assert!(!tree.is_red(Lane::L1) && !tree.is_red(Lane::L2));
        assert_eq!(tree.foul_flags, 0);

        // The launch margin is the first term of D20's formula, read from the node
        // the mapping names — and lane 2 left first, so it is negative.
        let p: PulseObservation = sim.block(START_L1).unwrap();
        let expected = ticks(R2) as i32 - ticks(R1) as i32;
        assert_eq!(p.launch_margin(), Some(TickDelta(expected)));
        assert!(
            expected < 0,
            "lane 2 reacted quicker, so the margin is negative"
        );
    }

    #[test]
    fn a_run_is_valid_and_the_widths_are_healthy() {
        let mut sim = finished(&clean_pair());
        for (addr, lane) in [(FINISH, Lane::L1), (SIXTY, Lane::L1), (TRAP, Lane::L1)] {
            let r = sim.run_record(addr, lane).unwrap();
            assert!(r.is_timing_valid(), "address {addr} disowned its run");
            assert!(r.is_race(), "address {addr} reported a self-test");
        }
        let p: PulseObservation = sim.block(START_L1).unwrap();
        assert!(p.width_valid(Lane::L1) && p.width_valid(Lane::L2));
        assert!(!p.width_marginal(Lane::L1));
    }

    #[test]
    fn every_device_observes_both_pulses() {
        // D24: the margin is meaningful at whichever address the mapping names,
        // because nobody has a role.
        let mut sim = finished(&clean_pair());
        for addr in [START_L1, START_L2, SIXTY, TRAP, FINISH, TREE] {
            let p: PulseObservation = sim.block(addr).unwrap();
            assert!(p.seen(Lane::L1) && p.seen(Lane::L2), "address {addr}");
            assert!(p.launch_margin().is_some(), "address {addr}");
        }
    }

    // -- staging, before any pulse exists ----------------------------------

    #[test]
    fn staging_is_visible_in_input_state_before_the_first_pulse() {
        let mut sim = sim(&clean_pair());
        sim.advance_to(ticks(2.0));

        let d: Digest = sim.block(START_L1).unwrap();
        assert!(d.beam_broken(0), "pre-stage broken");
        assert!(d.beam_broken(1), "stage broken");
        assert!(d.beam_intact(2), "the guard beam is not broken by a tire");
        assert!(
            d.run_gen_l1.is_never(),
            "no run has started, so the record must not look like one"
        );

        let tree: Tree = sim.block(TREE).unwrap();
        assert_eq!(
            tree.lamp_state & 0b1111,
            0b1111,
            "both lanes pre-staged and staged"
        );
    }

    #[test]
    fn the_launch_is_the_tire_leaving_the_stage_beam() {
        // D16: the monostable fires from that edge, so the launch instant and the
        // stage make are the same moment.
        let mut sim = finished(&clean_pair());
        let launch = sim.launch_at(Lane::L1).unwrap();
        let green = sim.green_at().unwrap();
        assert_eq!(launch - green, ticks(R1));

        // And the edge itself is *not* in the run it starts, which is worth
        // pinning because it looks like a bug and is not: the pulse derived from
        // that edge resets the capture timer, so the make was captured against the
        // previous zero. The master never needs it — the launch instant is the
        // zero, not a register.
        let stage = sim.run_record(START_L1, Lane::L1).unwrap();
        assert_eq!(stage.inputs[1].make_at(), None);
        assert_eq!(stage.gen.raw(), 1, "the run did start");
    }

    // -- the ugly runs of D26 ----------------------------------------------

    #[test]
    fn a_red_light_is_a_negative_reaction_time() {
        let text = clean_pair().replace(&format!("reaction_s = {R1}"), "reaction_s = -0.045");
        let mut sim = finished(&text);
        let tree: Tree = sim.block(TREE).unwrap();
        assert!(tree.is_red(Lane::L1));
        assert!(!tree.is_red(Lane::L2));
        assert_eq!(
            tree.reaction_time(Lane::L1),
            TickDelta(-(ticks(0.045) as i32))
        );
        assert_eq!(tree.foul_flags & 1, 1);
        // ...and the run itself is perfectly ordinary. A foul is not a broken run.
        assert!(sim.run_record(FINISH, Lane::L1).unwrap().is_timing_valid());
    }

    #[test]
    fn two_cars_leaving_three_milliseconds_apart() {
        let text = clean_pair()
            .replace(&format!("reaction_s = {R1}"), "reaction_s = 0.500")
            .replace(&format!("reaction_s = {R2}"), "reaction_s = 0.503");
        let mut sim = finished(&text);
        let p: PulseObservation = sim.block(START_L1).unwrap();
        assert_eq!(
            p.launch_margin(),
            Some(TickDelta(ticks(0.003) as i32)),
            "3 ms is 240,000 ticks and it decides the race"
        );
    }

    #[test]
    fn a_bad_pulse_width_disowns_a_run_that_had_already_started() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"bad_pulse_width\"\nlane = 1\nwidth_us = 900\n\n[[car]]\nlane = 1",
        );
        let mut sim = finished(&text);

        let r = sim.run_record(FINISH, Lane::L1).unwrap();
        assert!(r.flags.valid(), "the counter did start");
        assert!(r.flags.invalidated(), "and the pulse was then disowned");
        assert!(!r.is_timing_valid());
        // The number is still there. Whether to use it is the master's call.
        assert_eq!(r.inputs[0].break_at(), Some(Ticks(ticks(ET1) as u32)));

        // Lane 2 is untouched — one bad pulse does not spoil the other lane.
        assert!(sim.run_record(FINISH, Lane::L2).unwrap().is_timing_valid());
        let d: Digest = sim.block(FINISH).unwrap();
        assert!(d.pulse_invalid(Lane::L1) && !d.pulse_invalid(Lane::L2));
    }

    #[test]
    fn a_silent_node_times_out_rather_than_answering_zeros() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"silent\"\naddress = 3\nfrom_s = 0.0\n\n[[car]]\nlane = 1",
        );
        let mut sim = finished(&text);
        assert_eq!(sim.run_record(SIXTY, Lane::L1), Err(BusError::Timeout));
        // Everything else still answers; one dead node is not a dead bus (D09).
        assert!(sim.run_record(FINISH, Lane::L1).is_ok());
    }

    #[test]
    fn a_node_rebooting_mid_run_must_not_look_like_it_holds_a_split() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"reboot\"\naddress = 3\nat_s = 6.0\n\n[[car]]\nlane = 1",
        );
        let mut sim = finished(&text);

        let s: Status = sim.block(SIXTY).unwrap();
        assert_eq!(
            s.boot_count, 2,
            "boot_count moves, invalidating what was held"
        );
        assert!(s.faults.unexpected_reset());

        // The dangerous part, stated plainly: the node came back before the car
        // reached its beam, captured the edge with a free-running timer, and now
        // holds a number that *looks* like a split.
        let leftover = sim.run_record(SIXTY, Lane::L1).unwrap();
        assert!(leftover.inputs[0].break_at().is_some());
        assert_ne!(
            leftover.inputs[0].break_at(),
            Some(Ticks(ticks(SIXTY1) as u32)),
            "and it is not the right number"
        );

        // What makes it harmless is generation 0 plus the moved boot_count. This is
        // exactly why protocol.md §4 makes the master discard anything it holds for
        // a node whose run_gen reads 0 — the record's contents are not the defence.
        let d: Digest = sim.block(SIXTY).unwrap();
        assert!(
            d.run_gen_l1.is_never(),
            "a rebooted node must never appear to hold a valid split"
        );
        assert!(!leftover.gen.changed_from(d.run_gen_l1));
    }

    #[test]
    fn a_beam_that_breaks_and_never_makes_again() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"beam_stuck\"\naddress = 6\ninput = 0\n\n[[car]]\nlane = 1",
        );
        let mut sim = finished(&text);

        let cap = sim.run_record(FINISH, Lane::L1).unwrap().inputs[0];
        assert_eq!(
            cap.break_at(),
            Some(Ticks(ticks(ET1) as u32)),
            "ET is intact"
        );
        assert_eq!(cap.make_at(), None, "the beam never came back");
        // And the line still reads broken, which is loud (D17).
        let d: Digest = sim.block(FINISH).unwrap();
        assert!(d.beam_broken(0));
    }

    #[test]
    fn a_capture_overrun_is_reported_rather_than_hidden() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"capture_overrun\"\naddress = 3\nlane = 1\n\n[[car]]\nlane = 1",
        );
        let mut sim = finished(&text);
        assert!(sim.run_record(SIXTY, Lane::L1).unwrap().flags.overflow());
        assert!(!sim.run_record(SIXTY, Lane::L2).unwrap().flags.overflow());
    }

    // -- determinism -------------------------------------------------------

    #[test]
    fn the_same_seed_gives_the_same_green_and_a_different_one_does_not() {
        // Without this, D26's "here is the session, replay it, get the same ET"
        // fails on the first round.
        let a = finished(&clean_pair());
        let b = finished(&clean_pair());
        assert_eq!(a.green_at(), b.green_at());

        let c = finished(&clean_pair().replace("seed = 42", "seed = 43"));
        assert_ne!(a.green_at(), c.green_at());
    }

    #[test]
    fn autostart_delay_stays_inside_its_bound() {
        let sim = finished(&clean_pair());
        let green = sim.green_at().unwrap();
        let earliest = ticks(3.0) + ticks(1.5);
        assert!(green >= earliest);
        assert!(green <= earliest + ticks(0.700));
    }

    // -- the scenario is held to the mapping -------------------------------

    #[test]
    fn a_split_naming_an_unmapped_beam_fails_to_build() {
        // Loud, because the alternative is a run that quietly has one split fewer.
        let text = clean_pair().replace("interval_60 = 1.601", "interval_660 = 6.8");
        let m = venue();
        let s = Scenario::parse(&text).unwrap();
        assert_eq!(
            Simulator::new(&m, s).err(),
            Some(BuildError::UnmappedBeam {
                lane: 2,
                beam: Beam::Interval660
            })
        );
    }

    #[test]
    fn a_fault_naming_an_absent_address_fails_to_build() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"silent\"\naddress = 9\nfrom_s = 0.0\n\n[[car]]\nlane = 1",
        );
        let m = venue();
        let s = Scenario::parse(&text).unwrap();
        assert_eq!(
            Simulator::new(&m, s).err(),
            Some(BuildError::UnknownAddress(9))
        );
    }

    #[test]
    fn an_unknown_fault_kind_fails_to_parse() {
        let text = clean_pair().replace(
            "[[car]]\nlane = 1",
            "[[fault]]\nkind = \"gremlins\"\naddress = 3\n\n[[car]]\nlane = 1",
        );
        assert!(Scenario::parse(&text).is_err());
    }

    // -- the bus seam ------------------------------------------------------

    #[test]
    fn a_read_that_leaves_a_block_is_an_exception_not_a_guess() {
        let mut sim = finished(&clean_pair());
        let mut buf = [0u16; 2];
        assert_eq!(
            sim.read(FINISH, 0x0004, &mut buf),
            Err(BusError::Exception(2))
        );
        // The tree block does not exist on a timing node.
        let mut buf = [0u16; Tree::LEN as usize];
        assert_eq!(
            sim.read(FINISH, Tree::ADDR, &mut buf),
            Err(BusError::Exception(2))
        );
        assert!(sim.read(TREE, Tree::ADDR, &mut buf).is_ok());
    }

    #[test]
    fn a_run_record_is_one_transaction() {
        // protocol.md §2: splitting the read can pair a split from one run with a
        // generation from the next. BusExt makes that structurally impossible.
        let mut sim = finished(&clean_pair());
        let r = sim.run_record(FINISH, Lane::L1).unwrap();
        let d: Digest = sim.block(FINISH).unwrap();
        assert_eq!(r.gen, d.run_gen_l1, "the record and the digest agree");
    }
}
