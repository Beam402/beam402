//! The time slip.
//!
//! This is the only thing most people at a track will ever see of the project,
//! so two rules govern it. **A number that was not measured is never printed** —
//! a missing split is an em dash with its reason underneath, never a zero and
//! never a blank. And **the winner is stated with the reason it won**, because
//! "lane 1" on its own is what an argument in the staging lanes is made of.

use std::fmt::Write;

use beam402_mapping::Beam;
use beam402_protocol::Lane;
use beam402_race::staging::Blocked;
use beam402_race::{decide, foul, Outcome, Pairing, Reason, Round};

/// Intervals a slip prints, in the order a car meets them.
const SPLITS: [(Beam, &str); 4] = [
    (Beam::Interval60, "60 ft"),
    (Beam::Interval660, "1/8 mi"),
    (Beam::TrapEntry, "trap in"),
    (Beam::TrapExit, "trap out"),
];

pub fn render(
    round: &Round,
    pairing: &Pairing,
    blocked: Option<Blocked>,
    abandoned: bool,
) -> String {
    let mut out = String::new();
    let lanes: Vec<Lane> = pairing.entries().iter().map(|e| e.lane).collect();

    let _ = writeln!(out, "{:<10}{}", "", header(&lanes));
    let _ = writeln!(out, "{}", "-".repeat(10 + 14 * lanes.len()));

    row(&mut out, "dial", &lanes, |l| {
        pairing.entry(l).and_then(|e| e.dial_s).map(secs)
    });
    row(&mut out, "reaction", &lanes, |l| {
        round.lane(l)?.reaction_s.map(secs)
    });
    for (beam, label) in SPLITS {
        // A split nobody mapped is not a gap in this round — it is a beam the
        // track does not have, and printing a row of dashes for it would suggest
        // otherwise.
        let mapped = lanes.iter().any(|l| {
            round
                .lane(*l)
                .is_some_and(|r| r.splits_s.contains_key(&beam) || r.gap(beam).is_some())
        });
        if !mapped {
            continue;
        }
        row(&mut out, label, &lanes, |l| {
            round.lane(l)?.splits_s.get(&beam).copied().map(secs)
        });
    }
    row(&mut out, "ET", &lanes, |l| round.lane(l)?.et_s.map(secs));
    row(&mut out, "speed", &lanes, |l| {
        round
            .lane(l)?
            .trap_speed_kmh()
            .map(|v| format!("{v:.2} km/h"))
    });

    let _ = writeln!(out);
    for lane in &lanes {
        for gap in round.lane(*lane).map(|r| r.gaps.as_slice()).unwrap_or(&[]) {
            let _ = writeln!(
                out,
                "  lane {} {}: — ({}, node {} input {})",
                lane.number(),
                gap.beam,
                gap.why,
                gap.address,
                gap.input
            );
        }
        if let Some(f) = round
            .lane(*lane)
            .and_then(|r| foul(r, pairing.breakout_limit(*lane)))
        {
            let _ = writeln!(out, "  lane {}: {f}", lane.number());
        }
    }

    if let Some(why) = blocked {
        let _ = writeln!(out, "  blocked: {why}");
    }
    if abandoned {
        let _ = writeln!(
            out,
            "  round abandoned: a car never reached the finish beam"
        );
    }

    let _ = writeln!(out, "\n{}", verdict(round, pairing));
    out
}

fn verdict(round: &Round, pairing: &Pairing) -> String {
    match decide(round, pairing) {
        Outcome::Win { lane, reason } => {
            let why = match reason {
                Reason::FirstToFinish { margin_s } => {
                    format!("first to the finish by {margin_s:.4} s")
                }
                Reason::OpponentFouled(f) => format!("opponent {f}"),
                Reason::LesserFoul { own, opponent } => {
                    format!("both fouled — {own}, opponent {opponent}")
                }
                Reason::OpponentNoTime => "opponent has no time".to_string(),
                Reason::Bye => "bye run".to_string(),
            };
            format!("WIN  lane {} — {why}", lane.number())
        }
        // Not a draw. Once the margin is measured in microseconds a drag race
        // does not produce one, so this says what actually happened: there was
        // nothing to award.
        Outcome::NoContest => "NO CONTEST — nothing to award".to_string(),
    }
}

fn header(lanes: &[Lane]) -> String {
    lanes
        .iter()
        .map(|l| format!("{:>13} ", format!("lane {}", l.number())))
        .collect()
}

fn row(out: &mut String, label: &str, lanes: &[Lane], f: impl Fn(Lane) -> Option<String>) {
    let _ = write!(out, "{label:<10}");
    for lane in lanes {
        // The em dash is load-bearing: it says "not measured", which a zero
        // would not.
        let cell = f(*lane).unwrap_or_else(|| "—".to_string());
        let _ = write!(out, "{cell:>13} ");
    }
    let _ = writeln!(out);
}

fn secs(v: f64) -> String {
    format!("{v:.4}")
}
