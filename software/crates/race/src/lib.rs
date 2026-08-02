#![forbid(unsafe_code)]

//! Race logic: a pure function of the event stream, the mapping and the pairing.
//!
//! No serial handles, no clock reads, no file I/O below this line — `software.md`
//! §4's rule, and the reason all of this can be finished before any hardware
//! exists. A recorded session replays deterministically, which is what makes
//! "here is the session, replay it and get the same ET" a real answer to a
//! disputed round rather than a slogan.
//!
//! Three pieces, in the order a round happens:
//!
//! 1. [`format`] — what the two cars agreed to run, and the handicap that falls
//!    out of it. This is the module the word *bracket* lives in.
//! 2. [`round`] — registers into seconds, with the mapping supplying every word
//!    of meaning and a named reason attached to anything missing.
//! 3. [`outcome`] — first to the finish, first or worst.
//!
//! ## What a handicap does and does not change
//!
//! A bracket start runs the two lanes on separate cascades: the slower car
//! leaves, and the quicker car's tree begins `dial_slow − dial_quick` later. It
//! is worth being precise about how little of this system that touches, because
//! the answer is what says the timing model was right.
//!
//! - **ET is unchanged.** Its zero is that car's own launch pulse, so a car that
//!   waited four seconds on the line measures exactly what a car that did not
//!   would (**D04**).
//! - **Reaction time is unchanged** — measured against *that lane's* green,
//!   which is why the register map has two of them.
//! - **The margin is unchanged.** `(pulse₂ − pulse₁) + ET₂ − ET₁` is the finish
//!   order whether or not the pulses were four seconds apart: the handicap *is*
//!   part of the difference between the pulses (**D20**).
//!
//! What it does change is the tree, and that is where the cost landed: two
//! cascades, per-lane lamps, and two registers of handicap the master writes
//! before it arms.

pub mod format;
pub mod outcome;
pub mod round;

pub use format::{Entry, Format, Pairing, PairingError};
pub use outcome::{decide, foul, Foul, Outcome, Reason};
pub use round::{Gap, LaneRun, Missing, Round, RunBuilder};
