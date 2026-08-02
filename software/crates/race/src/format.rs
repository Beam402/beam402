//! What the two cars agreed to run, and what that makes the tree do.
//!
//! `software.md` §4 puts class and bracket rules on the *data* side of the line:
//! a club changing a rule should never see a compiler. So this module carries the
//! three shapes a round can have and nothing about any particular class — the
//! numbers arrive from configuration, and what lives here is only the arithmetic
//! that turns them into a handicap and a breakout limit.

use beam402_protocol::Lane;

/// The three ways a drag race decides a winner.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Format {
    /// Both cars get the green together and the first to the finish wins. No
    /// dial-in, no breakout: grudge racing, heads-up classes, qualifying.
    HeadsUp,
    /// Bracket racing. Each driver predicts an ET, the slower car leaves first by
    /// the difference, and running **quicker** than the prediction loses. Two
    /// drivers who both hit their dial exactly cross the finish line together,
    /// which is the whole point of the format: it makes a 12-second street car
    /// and a 7-second dragster a fair race.
    Bracket,
    /// A class index both drivers share — Super Comp's 8.90 and its relatives.
    /// The start is heads-up because the index is the same for both, and
    /// breakout still applies.
    Index { seconds: f64 },
}

impl Format {
    /// Does running quicker than the limit lose the round?
    pub const fn has_breakout(self) -> bool {
        !matches!(self, Format::HeadsUp)
    }

    pub const fn has_handicap(self) -> bool {
        matches!(self, Format::Bracket)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Entry {
    pub lane: Lane,
    /// The driver's predicted ET. Required by [`Format::Bracket`] and ignored by
    /// the others — an index is the class's number, not the driver's.
    pub dial_s: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PairingError {
    /// Both entries name the same lane, or a round has more than two.
    Lanes,
    /// Bracket racing without a dial-in. There is no sane default: guessing one
    /// would hand somebody a head start they did not earn.
    MissingDial(Lane),
    /// The handicap does not fit the `tree_handicap` argument — 65.5 s of
    /// spot. Not reachable with real dial-ins, and refused rather than wrapped.
    HandicapTooLarge { seconds: f64 },
}

impl core::fmt::Display for PairingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PairingError::Lanes => write!(f, "a round is one or two distinct lanes"),
            PairingError::MissingDial(l) => {
                write!(
                    f,
                    "lane {} has no dial-in and this is a bracket",
                    l.number()
                )
            }
            PairingError::HandicapTooLarge { seconds } => {
                write!(f, "a handicap of {seconds:.2} s does not fit the tree")
            }
        }
    }
}

/// One round: a format, and one or two entries.
#[derive(Clone, Debug)]
pub struct Pairing {
    format: Format,
    entries: Vec<Entry>,
}

impl Pairing {
    pub fn new(format: Format, entries: Vec<Entry>) -> Result<Self, PairingError> {
        if entries.is_empty() || entries.len() > 2 {
            return Err(PairingError::Lanes);
        }
        if entries.len() == 2 && entries[0].lane == entries[1].lane {
            return Err(PairingError::Lanes);
        }
        if format.has_handicap() {
            for e in &entries {
                if e.dial_s.is_none() {
                    return Err(PairingError::MissingDial(e.lane));
                }
            }
        }
        let p = Pairing { format, entries };
        // Computed here so an impossible handicap is a construction error rather
        // than a surprise between the arm and the green.
        p.handicap_ms()?;
        Ok(p)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// A single car on the track. It still has to make a valid run.
    pub fn is_bye(&self) -> bool {
        self.entries.len() == 1
    }

    pub fn entry(&self, lane: Lane) -> Option<&Entry> {
        self.entries.iter().find(|e| e.lane == lane)
    }

    pub fn opponent(&self, lane: Lane) -> Option<Lane> {
        self.entries.iter().map(|e| e.lane).find(|l| *l != lane)
    }

    /// Milliseconds each lane's cascade is held back, indexed by
    /// [`Lane::ord`]. Exactly what goes out as `tree_handicap` before the arm.
    ///
    /// The slower car — the larger dial — leaves first with zero. Rounding is to
    /// the millisecond, which is ten times finer than the hundredth a dial-in is
    /// quoted to, so the residue is well under the resolution of the thing being
    /// equalised.
    pub fn handicap_ms(&self) -> Result<[u16; 2], PairingError> {
        if !self.format.has_handicap() || self.is_bye() {
            return Ok([0; 2]);
        }
        let a = &self.entries[0];
        let b = &self.entries[1];
        let (da, db) = match (a.dial_s, b.dial_s) {
            (Some(da), Some(db)) => (da, db),
            _ => return Err(PairingError::MissingDial(a.lane)),
        };
        let spot = (da - db).abs();
        let ms = (spot * 1000.0).round();
        if ms > u16::MAX as f64 {
            return Err(PairingError::HandicapTooLarge { seconds: spot });
        }
        let mut out = [0u16; 2];
        // The quicker car waits. Equal dials wait not at all, which is a
        // heads-up start arrived at honestly rather than by a special case.
        let waits = if da < db { a.lane } else { b.lane };
        if ms > 0.0 {
            out[waits.ord() as usize] = ms as u16;
        }
        Ok(out)
    }

    /// The ET this lane may not run quicker than, or `None` when the format has
    /// no breakout.
    pub fn breakout_limit(&self, lane: Lane) -> Option<f64> {
        match self.format {
            Format::HeadsUp => None,
            Format::Index { seconds } => Some(seconds),
            Format::Bracket => self.entry(lane)?.dial_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bracket(d1: f64, d2: f64) -> Pairing {
        Pairing::new(
            Format::Bracket,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: Some(d1),
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: Some(d2),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn the_slower_car_leaves_first_by_the_difference() {
        // A 12.34 street car against a 7.50 dragster: the dragster waits 4.84 s.
        let p = bracket(12.34, 7.50);
        assert_eq!(p.handicap_ms().unwrap(), [0, 4840]);
        // And the other way round, so the arithmetic is not accidentally
        // lane-flavoured.
        let p = bracket(7.50, 12.34);
        assert_eq!(p.handicap_ms().unwrap(), [4840, 0]);
    }

    #[test]
    fn equal_dials_are_a_heads_up_start_without_a_special_case() {
        assert_eq!(bracket(9.90, 9.90).handicap_ms().unwrap(), [0, 0]);
    }

    #[test]
    fn an_index_class_starts_heads_up_and_still_breaks_out() {
        let p = Pairing::new(
            Format::Index { seconds: 8.90 },
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: None,
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(p.handicap_ms().unwrap(), [0, 0]);
        assert_eq!(p.breakout_limit(Lane::L1), Some(8.90));
    }

    #[test]
    fn heads_up_has_no_limit_to_break_out_of() {
        let p = Pairing::new(
            Format::HeadsUp,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: None,
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(p.breakout_limit(Lane::L1), None);
    }

    #[test]
    fn a_bracket_without_a_dial_in_refuses_to_build() {
        // Defaulting it would hand somebody a spot they did not earn, and the
        // error would show up as a lost race rather than as a broken entry.
        assert_eq!(
            Pairing::new(
                Format::Bracket,
                vec![
                    Entry {
                        lane: Lane::L1,
                        dial_s: Some(9.9)
                    },
                    Entry {
                        lane: Lane::L2,
                        dial_s: None
                    },
                ],
            )
            .err(),
            Some(PairingError::MissingDial(Lane::L2))
        );
    }

    #[test]
    fn a_bye_run_has_no_handicap_to_apply() {
        let p = Pairing::new(
            Format::Bracket,
            vec![Entry {
                lane: Lane::L1,
                dial_s: Some(11.5),
            }],
        )
        .unwrap();
        assert!(p.is_bye());
        assert_eq!(p.handicap_ms().unwrap(), [0, 0]);
        assert_eq!(p.breakout_limit(Lane::L1), Some(11.5));
    }
}
