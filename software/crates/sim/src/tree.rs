//! The tree's sequence, and where reaction time comes from.
//!
//! The tree is not a second simulator. It is a device of class 2: the same
//! register layer, the same latching, plus one block and this state machine
//! (**D07** keeps it off the universal board, **D24** keeps it off a special
//! protocol).
//!
//! What it owns that no other device does is **the instant the green lit**. Under
//! **D24** it also observes both start pulses, so both terms of
//! `RT = t_pulse − t_green` sit on its own clock and **D04** is not violated to
//! produce a number handed to a driver. A red light is not a special case — it is
//! a negative reaction time.

use beam402_protocol::words::Generation;
use beam402_protocol::{Lane, Tree, TreeMode};

use crate::scenario::{ticks, Mode};

/// `tree_state`. The register map does not fix these values; they are the
/// simulator's convention until the tree firmware settles them, and are written
/// down here rather than left implicit.
pub mod state {
    pub const IDLE: u16 = 0;
    pub const ARMED: u16 = 1;
    pub const SEQUENCING: u16 = 2;
    pub const GREEN: u16 = 3;
}

/// `lamp_state`, likewise a convention rather than contract — the map calls it
/// only "bitmap, for the operator display".
pub mod lamp {
    pub const PRESTAGE_L1: u16 = 1 << 0;
    pub const PRESTAGE_L2: u16 = 1 << 1;
    pub const STAGE_L1: u16 = 1 << 2;
    pub const STAGE_L2: u16 = 1 << 3;
    pub const AMBER_1: u16 = 1 << 4;
    pub const AMBER_2: u16 = 1 << 5;
    pub const AMBER_3: u16 = 1 << 6;
    pub const GREEN: u16 = 1 << 7;
    pub const RED_L1: u16 = 1 << 8;
    pub const RED_L2: u16 = 1 << 9;
}

/// Which step of the cascade a scheduled lamp change is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Amber1,
    Amber2,
    Amber3,
    Green,
}

#[derive(Clone, Debug)]
pub struct TreeSim {
    mode: Mode,
    block: Tree,
    green_at: Option<u64>,
    pulse_at: [Option<u64>; 2],
    reaction: [i64; 2],
}

impl TreeSim {
    pub fn new(mode: Mode) -> Self {
        let mut block = Tree::default();
        block.mode = match mode {
            Mode::Standard => TreeMode::Standard,
            Mode::Pro => TreeMode::Pro,
        };
        TreeSim {
            mode,
            block,
            green_at: None,
            pulse_at: [None; 2],
            reaction: [0; 2],
        }
    }

    pub fn block(&self) -> Tree {
        self.block
    }

    pub fn green_at(&self) -> Option<u64> {
        self.green_at
    }

    /// Sequence delays must include LED turn-on time; a tree that ignores it
    /// systematically red-lights experienced drivers. That calibration is a
    /// bench measurement (`architecture.md` §8), so the figure lives in one place
    /// rather than being folded invisibly into the cascade.
    pub fn cascade_ticks(&self) -> u64 {
        match self.mode {
            // Three ambers 500 ms apart, then green.
            Mode::Standard => ticks(1.5),
            // All three together, green 400 ms later.
            Mode::Pro => ticks(0.4),
        }
    }

    pub fn step_offset(&self, step: Step) -> u64 {
        match (self.mode, step) {
            (_, Step::Amber1) => 0,
            (Mode::Standard, Step::Amber2) => ticks(0.5),
            (Mode::Standard, Step::Amber3) => ticks(1.0),
            (Mode::Pro, Step::Amber2) | (Mode::Pro, Step::Amber3) => 0,
            (_, Step::Green) => self.cascade_ticks(),
        }
    }

    pub fn staged(&mut self, lane: Lane, prestage: bool, stage: bool) {
        let (p, s) = match lane {
            Lane::L1 => (lamp::PRESTAGE_L1, lamp::STAGE_L1),
            Lane::L2 => (lamp::PRESTAGE_L2, lamp::STAGE_L2),
        };
        let mut bits = self.block.lamp_state;
        if prestage {
            bits |= p;
        } else {
            bits &= !p;
        }
        if stage {
            bits |= s;
        } else {
            bits &= !s;
        }
        self.block.lamp_state = bits;
    }

    /// The master arms; the tree runs it (`software.md` §5). AutoStart's bounds
    /// arrive with the command and are volatile per round, so **D08**'s "the DIP
    /// switch is the only configuration" survives.
    pub fn arm(&mut self) {
        self.block.sequence_gen = self.block.sequence_gen.next();
        self.block.state = state::ARMED;
        self.block.foul_flags = 0;
        self.block.lamp_state &=
            lamp::PRESTAGE_L1 | lamp::PRESTAGE_L2 | lamp::STAGE_L1 | lamp::STAGE_L2;
        self.green_at = None;
        self.pulse_at = [None; 2];
        self.reaction = [0; 2];
        self.block = self.block.with_reaction_times(0, 0);
    }

    pub fn abort(&mut self) {
        self.block.state = state::IDLE;
        self.block.lamp_state = 0;
        self.green_at = None;
    }

    pub fn light(&mut self, step: Step, at: u64) {
        match step {
            Step::Amber1 => {
                self.block.state = state::SEQUENCING;
                self.block.lamp_state |= lamp::AMBER_1;
            }
            Step::Amber2 => self.block.lamp_state |= lamp::AMBER_2,
            Step::Amber3 => self.block.lamp_state |= lamp::AMBER_3,
            Step::Green => {
                self.block.state = state::GREEN;
                self.block.lamp_state |= lamp::GREEN;
                self.green_at = Some(at);
                // Captured from the lamp driver output, not taken when firmware
                // writes the LED — otherwise firmware latency lands in a number
                // handed to the driver, which is D16's mistake in another device.
                self.block.t_green_l1 = at as u32;
                self.block.t_green_l2 = at as u32;
                // A driver who left before green already has a pulse on record.
                // The reaction time is computable now, and it is negative.
                for lane in Lane::ALL {
                    if let Some(p) = self.pulse_at[lane.ord() as usize] {
                        self.settle(lane, p, at);
                    }
                }
            }
        }
    }

    /// A start pulse reached the tree. Both terms of the subtraction are the
    /// tree's own registers, so this is not a cross-node comparison.
    pub fn observed_pulse(&mut self, lane: Lane, at: u64) {
        self.pulse_at[lane.ord() as usize] = Some(at);
        match self.green_at {
            Some(green) => self.settle(lane, at, green),
            // Left before green. Nothing is lost: the tree holds the pulse in its
            // own register and settles the number when the green lights, which is
            // how a red light comes out as a negative reaction time rather than as
            // a special case.
            None => self.note_foul(lane),
        }
    }

    fn settle(&mut self, lane: Lane, pulse: u64, green: u64) {
        let rt = pulse as i64 - green as i64;
        self.reaction[lane.ord() as usize] = rt;
        self.block = self
            .block
            .with_reaction_times(self.reaction[0] as i32, self.reaction[1] as i32);
        if rt < 0 {
            self.note_foul(lane);
        }
    }

    fn note_foul(&mut self, lane: Lane) {
        let (foul, red) = match lane {
            Lane::L1 => (1u16 << 0, lamp::RED_L1),
            Lane::L2 => (1u16 << 1, lamp::RED_L2),
        };
        self.block.foul_flags |= foul;
        self.block.lamp_state |= red;
    }

    pub fn sequence_gen(&self) -> Generation {
        self.block.sequence_gen
    }
}
