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
//!
//! ## Two cascades, not one
//!
//! A handicap start runs the two lanes on separate clocks-of-their-own: the
//! slower car's cascade begins, and the quicker car's begins `handicap`
//! milliseconds later, so that two drivers who both run exactly their dial-in
//! cross the finish line together. Everything downstream is unchanged — each
//! lane's reaction time is measured against **its own** green, and **D20**'s
//! launch margin already carries the handicap, because the handicap *is* part of
//! the difference between the two launch pulses.
//!
//! That is why the register map has `t_green_l1` and `t_green_l2` rather than one
//! green, and it is why the lamps are per lane: during a handicap start the two
//! columns are genuinely showing different things.

use beam402_protocol::flags::{Lamp, LampFlags};
use beam402_protocol::words::Generation;
use beam402_protocol::{Lane, Tree, TreeMode, TreeState};

use crate::scenario::{ticks, Mode};

/// Which step of a lane's cascade a scheduled lamp change is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Amber1,
    Amber2,
    Amber3,
    Green,
}

impl Step {
    pub const ALL: [Step; 4] = [Step::Amber1, Step::Amber2, Step::Amber3, Step::Green];

    const fn lamp(self) -> Lamp {
        match self {
            Step::Amber1 => Lamp::Amber1,
            Step::Amber2 => Lamp::Amber2,
            Step::Amber3 => Lamp::Amber3,
            Step::Green => Lamp::Green,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TreeSim {
    mode: Mode,
    block: Tree,
    /// Written by `tree_handicap`, consumed by `tree_arm`. Consumed rather than
    /// left standing so a handicap forgotten from the previous pair cannot apply
    /// to the next one: a race that goes heads-up when it should not is visible
    /// to everybody, a stale head start is not.
    pending_handicap: [u16; 2],
    green_at: [Option<u64>; 2],
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
            pending_handicap: [0; 2],
            green_at: [None; 2],
            pulse_at: [None; 2],
            reaction: [0; 2],
        }
    }

    pub fn block(&self) -> Tree {
        self.block
    }

    pub fn green_at(&self, lane: Lane) -> Option<u64> {
        self.green_at[lane.ord() as usize]
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

    /// Offset of a step within one lane's cascade, handicap included.
    pub fn step_offset(&self, lane: Lane, step: Step) -> u64 {
        let within = match (self.mode, step) {
            (_, Step::Amber1) => 0,
            (Mode::Standard, Step::Amber2) => ticks(0.5),
            (Mode::Standard, Step::Amber3) => ticks(1.0),
            (Mode::Pro, Step::Amber2) | (Mode::Pro, Step::Amber3) => 0,
            (_, Step::Green) => self.cascade_ticks(),
        };
        within + self.handicap_ticks(lane)
    }

    pub fn handicap_ticks(&self, lane: Lane) -> u64 {
        ticks(self.block.handicap_ms(lane) as f64 / 1000.0)
    }

    /// `tree_handicap`, arg0 = lane, arg1 = milliseconds. Volatile per round, so
    /// **D08**'s "the DIP switch is the only configuration" survives.
    pub fn set_handicap(&mut self, lane: Lane, ms: u16) {
        self.pending_handicap[lane.ord() as usize] = ms;
    }

    pub fn staged(&mut self, lane: Lane, prestage: bool, stage: bool) {
        let ord = lane.ord();
        self.block.lamps =
            self.block
                .lamps
                .set(Lamp::Prestage, ord, prestage)
                .set(Lamp::Stage, ord, stage);
    }

    /// The master arms; the tree runs it (`software.md` §5). AutoStart's bounds
    /// arrive with the command and are volatile per round, and so is the
    /// handicap — which is latched here, where the master can still read it back
    /// and refuse to let the cars stage if it is wrong.
    pub fn arm(&mut self) {
        self.block.sequence_gen = self.block.sequence_gen.next();
        self.block.state = TreeState::Armed;
        self.block.foul_flags = 0;
        // Staging lamps survive an arm; everything the sequence lights does not.
        let mut lamps = LampFlags::from_bits(0);
        for lane in Lane::ALL {
            for lamp in [Lamp::Prestage, Lamp::Stage] {
                lamps = lamps.set(lamp, lane.ord(), self.block.lamps.lit(lamp, lane.ord()));
            }
        }
        self.block.lamps = lamps;
        self.block = self
            .block
            .with_handicap(self.pending_handicap[0], self.pending_handicap[1]);
        self.pending_handicap = [0; 2];
        self.green_at = [None; 2];
        self.pulse_at = [None; 2];
        self.reaction = [0; 2];
        self.block = self.block.with_reaction_times(0, 0);
    }

    pub fn abort(&mut self) {
        self.block.state = TreeState::Idle;
        self.block.lamps = LampFlags::from_bits(0);
        self.block = self.block.with_handicap(0, 0);
        self.pending_handicap = [0; 2];
        self.green_at = [None; 2];
    }

    pub fn light(&mut self, lane: Lane, step: Step, at: u64) {
        let ord = lane.ord();
        self.block.lamps = self.block.lamps.set(step.lamp(), ord, true);
        if step != Step::Green {
            if self.block.state == TreeState::Armed {
                self.block.state = TreeState::Sequencing;
            }
            return;
        }

        self.green_at[ord as usize] = Some(at);
        // Captured from the lamp driver output, not taken when firmware writes the
        // LED — otherwise firmware latency lands in a number handed to the driver,
        // which is D16's mistake in another device.
        match lane {
            Lane::L1 => self.block.t_green_l1 = at as u32,
            Lane::L2 => self.block.t_green_l2 = at as u32,
        }
        if self.green_at.iter().all(|g| g.is_some()) {
            self.block.state = TreeState::Green;
        } else {
            self.block.state = TreeState::Sequencing;
        }
        // A driver who left before their own green already has a pulse on record.
        // The reaction time is computable now, and it is negative.
        if let Some(p) = self.pulse_at[ord as usize] {
            self.settle(lane, p, at);
        }
    }

    /// A start pulse reached the tree. Both terms of the subtraction are the
    /// tree's own registers, so this is not a cross-node comparison.
    pub fn observed_pulse(&mut self, lane: Lane, at: u64) {
        self.pulse_at[lane.ord() as usize] = Some(at);
        match self.green_at[lane.ord() as usize] {
            Some(green) => self.settle(lane, at, green),
            // Left before green. Nothing is lost: the tree holds the pulse in its
            // own register and settles the number when that lane's green lights,
            // which is how a red light comes out as a negative reaction time
            // rather than as a special case.
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
        self.block.foul_flags |= 1u16 << lane.ord();
        self.block.lamps = self.block.lamps.set(Lamp::Red, lane.ord(), true);
    }

    pub fn sequence_gen(&self) -> Generation {
        self.block.sequence_gen
    }
}
