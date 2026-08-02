//! Who won, and why.
//!
//! Two rules do all the work, and both are older than any timing system:
//!
//! - **First to the finish wins.** Not the quicker ET — the car that crossed the
//!   stripe first. In a bracket those are usually different cars, which is why
//!   [`Round::finish_margin_s`] exists and why **D20** insists the launch margin
//!   is measured rather than inferred.
//! - **First or worst.** A driver who fouls loses. If both foul, the worse foul
//!   loses, and between two fouls of the same kind the earlier one does.
//!
//! Everything else is bookkeeping. The rules are gathered into one function on
//! purpose: `software.md` §4 promises a club can change a class rule without
//! seeing a compiler, and the first step toward keeping that promise is having
//! exactly one place where the rule is written down.

use beam402_protocol::Lane;

use crate::format::Pairing;
use crate::round::{LaneRun, Round};

/// Something that loses a round on its own.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Foul {
    /// Left before their own green. `by` is how early, in seconds.
    RedLight { by: f64 },
    /// Ran quicker than the dial-in or the index. `by` is how much quicker.
    Breakout { by: f64 },
}

impl Foul {
    /// **Red light beats breakout.** A driver who left early has taken something
    /// from their opponent; a driver who ran quick has only misjudged their own
    /// car. Rulebooks are consistent about this and so is the panel.
    const fn severity(self) -> u8 {
        match self {
            Foul::RedLight { .. } => 2,
            Foul::Breakout { .. } => 1,
        }
    }

    pub fn amount(self) -> f64 {
        match self {
            Foul::RedLight { by } | Foul::Breakout { by } => by,
        }
    }
}

impl core::fmt::Display for Foul {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Foul::RedLight { by } => write!(f, "red light by {by:.3} s"),
            Foul::Breakout { by } => write!(f, "broke out by {by:.3} s"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Reason {
    /// Crossed the stripe first, by this many seconds.
    FirstToFinish { margin_s: f64 },
    /// The opponent fouled.
    OpponentFouled(Foul),
    /// Both fouled and this one's was the lesser.
    LesserFoul { own: Foul, opponent: Foul },
    /// The opponent has no ET — no time, whatever else happened.
    OpponentNoTime,
    /// No opponent. A bye still has to be a valid run.
    Bye,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome {
    Win {
        lane: Lane,
        reason: Reason,
    },
    /// Nobody made a run there is anything to award. Not a draw — a draw is not a
    /// thing a drag race produces once the margin is measured in microseconds.
    NoContest,
}

/// Whether this lane fouled, and how.
///
/// A breakout is only possible in a format that has a limit, and only counts
/// against a run that produced an ET: you cannot break out of a run you did not
/// finish.
pub fn foul(run: &LaneRun, limit: Option<f64>) -> Option<Foul> {
    if let Some(rt) = run.reaction_s {
        if rt < 0.0 {
            return Some(Foul::RedLight { by: -rt });
        }
    }
    let (Some(et), Some(limit)) = (run.et_s, limit) else {
        return None;
    };
    if et < limit {
        return Some(Foul::Breakout { by: limit - et });
    }
    None
}

/// Decide the round.
pub fn decide(round: &Round, pairing: &Pairing) -> Outcome {
    if pairing.is_bye() {
        return bye(round, pairing);
    }
    let Some(entries) = two(pairing) else {
        return Outcome::NoContest;
    };
    let (a, b) = entries;
    let (Some(run_a), Some(run_b)) = (round.lane(a), round.lane(b)) else {
        return Outcome::NoContest;
    };

    let foul_a = foul(run_a, pairing.breakout_limit(a));
    let foul_b = foul(run_b, pairing.breakout_limit(b));

    match (foul_a, foul_b) {
        (None, Some(f)) => {
            return Outcome::Win {
                lane: a,
                reason: Reason::OpponentFouled(f),
            }
        }
        (Some(f), None) => {
            return Outcome::Win {
                lane: b,
                reason: Reason::OpponentFouled(f),
            }
        }
        (Some(fa), Some(fb)) => return double_foul(round, a, b, fa, fb),
        (None, None) => {}
    }

    // No fouls: it comes down to the stripe, and only a run that produced an ET
    // reached it.
    match (run_a.has_time(), run_b.has_time()) {
        (true, false) => Outcome::Win {
            lane: a,
            reason: Reason::OpponentNoTime,
        },
        (false, true) => Outcome::Win {
            lane: b,
            reason: Reason::OpponentNoTime,
        },
        (false, false) => Outcome::NoContest,
        (true, true) => match round.finish_margin_s() {
            // Positive means lane 2 crossed later, so lane 1 took the stripe.
            Some(m) if m > 0.0 => Outcome::Win {
                lane: Lane::L1,
                reason: Reason::FirstToFinish { margin_s: m },
            },
            Some(m) if m < 0.0 => Outcome::Win {
                lane: Lane::L2,
                reason: Reason::FirstToFinish { margin_s: -m },
            },
            // Either the margin source did not report both pulses, or the two
            // cars crossed on the same tick. Both are "we cannot say", and
            // saying so is the only honest answer a timing system has.
            _ => Outcome::NoContest,
        },
    }
}

/// **First or worst.** A red light outranks a breakout; two of a kind are
/// separated by which came first, and only when that cannot be established by
/// which was worse.
fn double_foul(round: &Round, a: Lane, b: Lane, fa: Foul, fb: Foul) -> Outcome {
    let (winner, own, opponent) = if fa.severity() != fb.severity() {
        if fa.severity() > fb.severity() {
            (b, fb, fa)
        } else {
            (a, fa, fb)
        }
    } else if matches!(fa, Foul::RedLight { .. }) {
        // Two red lights: **first** loses, and under a handicap that is a
        // question about the clock rather than about the two numbers. The tree
        // knows, because both greens and both pulses are its own registers.
        match round.first_away() {
            Some(first) if first == a => (b, fb, fa),
            Some(_) => (a, fa, fb),
            // No tree block to ask. Fall back to the worse red, which is the
            // right answer whenever the greens were together and the only one
            // available when they were not.
            None if fa.amount() >= fb.amount() => (b, fb, fa),
            None => (a, fa, fb),
        }
    } else {
        // Two breakouts: **worst** loses — the run further from the dial.
        if fa.amount() >= fb.amount() {
            (b, fb, fa)
        } else {
            (a, fa, fb)
        }
    };
    Outcome::Win {
        lane: winner,
        reason: Reason::LesserFoul { own, opponent },
    }
}

fn bye(round: &Round, pairing: &Pairing) -> Outcome {
    let lane = pairing.entries()[0].lane;
    let Some(run) = round.lane(lane) else {
        return Outcome::NoContest;
    };
    // A bye is not free: the driver still has to make a clean, timed run. Clubs
    // differ on whether a red light on a bye ends the day, so this is the one
    // line to change and the reason it is a line.
    if !run.has_time() || foul(run, pairing.breakout_limit(lane)).is_some() {
        return Outcome::NoContest;
    }
    Outcome::Win {
        lane,
        reason: Reason::Bye,
    }
}

fn two(pairing: &Pairing) -> Option<(Lane, Lane)> {
    let e = pairing.entries();
    (e.len() == 2).then(|| (e[0].lane, e[1].lane))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{Entry, Format};
    use crate::round::Round;

    fn run(rt: f64, et: f64) -> LaneRun {
        LaneRun {
            reaction_s: Some(rt),
            et_s: Some(et),
            ..LaneRun::default()
        }
    }

    /// Build a round from two lanes' numbers, deriving the launch margin the way
    /// the hardware would: the difference between the two launch pulses.
    fn round_of(l1: LaneRun, l2: LaneRun, handicap_l2_s: f64) -> Round {
        let launch1 = l1.reaction_s.unwrap();
        let launch2 = handicap_l2_s + l2.reaction_s.unwrap();
        let mut r = Round::default();
        r.launch_margin_s = Some(launch2 - launch1);
        r.set_lane(Lane::L1, l1);
        r.set_lane(Lane::L2, l2);
        r
    }

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

    fn heads_up() -> Pairing {
        Pairing::new(
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
        .unwrap()
    }

    #[test]
    fn heads_up_is_won_at_the_stripe_not_on_the_clock() {
        // Lane 2 runs the quicker ET and still loses, because lane 1 left first
        // by more than the ET difference. This is the case ET alone gets wrong.
        let r = round_of(run(0.510, 10.400), run(0.640, 10.350), 0.0);
        match decide(&r, &heads_up()) {
            Outcome::Win {
                lane: Lane::L1,
                reason: Reason::FirstToFinish { margin_s },
            } => assert!((margin_s - 0.080).abs() < 1e-9, "{margin_s}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_perfect_bracket_is_a_dead_heat_by_construction() {
        // The whole point of the format: a 12.34 car and a 7.50 car who both hit
        // their dial exactly and react identically arrive together. The handicap
        // rides in the launch margin, so nothing here knows it exists.
        let p = bracket(12.34, 7.50);
        let spot = p.handicap_ms().unwrap()[Lane::L2.ord() as usize] as f64 / 1000.0;
        let r = round_of(run(0.500, 12.340), run(0.500, 7.500), spot);
        let margin = r.finish_margin_s().unwrap();
        assert!(margin.abs() < 1e-9, "dead heat, got {margin}");
        assert_eq!(decide(&r, &p), Outcome::NoContest);
    }

    #[test]
    fn the_slower_car_wins_a_bracket_by_driving_better() {
        // The dragster reacts worse and gives up 0.04 s it never gets back, even
        // though its ET is 4.8 seconds quicker.
        let p = bracket(12.34, 7.50);
        let spot = p.handicap_ms().unwrap()[Lane::L2.ord() as usize] as f64 / 1000.0;
        let r = round_of(run(0.500, 12.340), run(0.540, 7.500), spot);
        match decide(&r, &p) {
            Outcome::Win {
                lane: Lane::L1,
                reason: Reason::FirstToFinish { margin_s },
            } => assert!((margin_s - 0.040).abs() < 1e-9, "{margin_s}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn running_quicker_than_the_dial_loses_the_round() {
        let p = bracket(12.34, 7.50);
        let spot = p.handicap_ms().unwrap()[Lane::L2.ord() as usize] as f64 / 1000.0;
        // Lane 2 runs 7.44 — a 0.06 breakout — and crosses first. It still loses.
        let r = round_of(run(0.500, 12.340), run(0.500, 7.440), spot);
        assert!(r.finish_margin_s().unwrap() < 0.0, "lane 2 took the stripe");
        match decide(&r, &p) {
            Outcome::Win {
                lane: Lane::L1,
                reason: Reason::OpponentFouled(Foul::Breakout { by }),
            } => assert!((by - 0.060).abs() < 1e-9, "{by}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn heads_up_has_nothing_to_break_out_of() {
        // The same 7.44 against the same field, with no dial: it just wins.
        let r = round_of(run(0.500, 10.400), run(0.500, 7.440), 0.0);
        match decide(&r, &heads_up()) {
            Outcome::Win {
                lane: Lane::L2,
                reason: Reason::FirstToFinish { .. },
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_red_light_is_worse_than_a_breakout() {
        // Both foul. Lane 1 left early, lane 2 ran quick; lane 2 wins, and the
        // panel says why in both directions.
        let p = bracket(12.34, 7.50);
        let spot = p.handicap_ms().unwrap()[Lane::L2.ord() as usize] as f64 / 1000.0;
        let r = round_of(run(-0.012, 12.340), run(0.500, 7.400), spot);
        match decide(&r, &p) {
            Outcome::Win {
                lane: Lane::L2,
                reason: Reason::LesserFoul { own, opponent },
            } => {
                assert!(matches!(own, Foul::Breakout { .. }));
                assert!(matches!(opponent, Foul::RedLight { .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_red_lights_are_separated_by_who_left_first() {
        let r = round_of(run(-0.030, 10.400), run(-0.008, 10.350), 0.0);
        match decide(&r, &heads_up()) {
            Outcome::Win {
                lane: Lane::L2,
                reason: Reason::LesserFoul { .. },
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_breakouts_are_separated_by_who_broke_out_more() {
        let p = bracket(9.90, 9.90);
        let r = round_of(run(0.500, 9.700), run(0.500, 9.880), 0.0);
        match decide(&r, &p) {
            Outcome::Win {
                lane: Lane::L2,
                reason: Reason::LesserFoul { own, opponent },
            } => {
                assert!(matches!(own, Foul::Breakout { by } if (by - 0.020).abs() < 1e-9));
                assert!(matches!(opponent, Foul::Breakout { by } if (by - 0.200).abs() < 1e-9));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_lane_with_no_et_has_no_time_whatever_else_it_did() {
        let mut broken = run(0.480, 0.0);
        broken.et_s = None;
        let r = round_of(run(0.520, 10.400), broken, 0.0);
        assert_eq!(
            decide(&r, &heads_up()),
            Outcome::Win {
                lane: Lane::L1,
                reason: Reason::OpponentNoTime
            }
        );
    }

    #[test]
    fn a_car_that_never_ran_cannot_break_out() {
        // Breakout is a property of a finished run. Without an ET there is
        // nothing to compare to the dial, and calling it a foul would turn a
        // broken beam into a disqualification.
        let mut broken = run(0.500, 0.0);
        broken.et_s = None;
        assert_eq!(foul(&broken, Some(9.90)), None);
    }

    #[test]
    fn a_bye_still_has_to_be_a_run() {
        let p = Pairing::new(
            Format::Bracket,
            vec![Entry {
                lane: Lane::L1,
                dial_s: Some(11.50),
            }],
        )
        .unwrap();

        let mut r = Round::default();
        r.set_lane(Lane::L1, run(0.520, 11.600));
        assert_eq!(
            decide(&r, &p),
            Outcome::Win {
                lane: Lane::L1,
                reason: Reason::Bye
            }
        );

        // Red light on a bye: no win to award.
        let mut r = Round::default();
        r.set_lane(Lane::L1, run(-0.010, 11.600));
        assert_eq!(decide(&r, &p), Outcome::NoContest);
    }
}
