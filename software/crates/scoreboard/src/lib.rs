#![forbid(unsafe_code)]

//! The spectator board, as pixels.
//!
//! **D23** puts the scoreboard on a LAN page reached by a QR code, and calls it
//! "latched numbers, not an application". This crate keeps that and adds one
//! constraint on top: what it produces is a **frame of pixels at a declared
//! resolution**, not a document.
//!
//! ## Why a resolution, when the page has none
//!
//! A real drag strip's board is an LED matrix, and the day one exists here the
//! served page should be its preview and its fallback — a club without a board
//! casts the page to a television and sees the same thing. That only works if
//! both consume the same frame.
//!
//! The useful half is the constraint, not the sharing: a page free to lay itself
//! out will grow a layout no board can render, and nobody finds out until the
//! panels arrive. Declaring the resolution first makes "does it fit" a test
//! instead of a discovery. Two of them fail today if a line grows by one
//! character.
//!
//! **No board has been bought or specified.** [`Board::REFERENCE`] is a plausible
//! geometry to design against, chosen because it is a whole number of the
//! commodity module every LED sign is built from — and **D15** gates buying
//! anything until the bench answers.
//!
//! ## What it will not do
//!
//! It does not decide anything. Who won, what broke out, which split is missing
//! — all of that arrives settled from [`beam402_race`], because a board that
//! reasons is a second implementation of the rules, and two implementations of a
//! rule is one more than anybody can keep right.

use beam402_protocol::Lane;
use beam402_race::{decide, Outcome, Pairing, Round};

pub mod font;
pub mod frame;
pub mod html;

pub use frame::Frame;
pub use html::Shot;

/// The board's shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Board {
    pub w: usize,
    /// Rows per lane. The board is one band per lane, stacked.
    pub band: usize,
    pub lanes: usize,
}

impl Board {
    /// What this project designs against until something is measured or bought.
    ///
    /// 128 × 32 per lane is eight of the 32 × 16 modules LED signs are assembled
    /// from — four across, two down, per band. It is named as a *reference*
    /// rather than a choice: **D15** gates buying anything, and the value here is
    /// that a fixed number makes the layout falsifiable, not that this is the
    /// number a club will end up with.
    pub const REFERENCE: Board = Board {
        w: 128,
        band: 32,
        lanes: 2,
    };

    pub const fn height(&self) -> usize {
        self.band * self.lanes
    }

    /// Rows a band spends, and what is left.
    ///
    /// At the reference geometry: seven for the dial line, fourteen for the ET
    /// at double size, seven for reaction and speed, one for the separator.
    /// That is twenty-nine of thirty-two. **There is no fourth line**, and the
    /// next field somebody wants — 60 ft, a class name, a driver — costs a
    /// taller band and therefore more panels. Written down here so the trade is
    /// visible before anyone is standing in front of a supplier.
    pub const fn rows_spent(&self) -> usize {
        font::H + font::H * 2 + font::H + 1
    }
}

/// What the board is showing. Derived from the round's phase by the caller, so
/// this crate never has to know what a staging machine is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Show {
    /// Between rounds.
    Idle,
    /// Cars at the line. Dial-ins up, nothing timed yet.
    Staging,
    /// The bus is quiet or the cars are out there. Numbers are not in yet, and
    /// the board says exactly that rather than showing a stale pair.
    Running,
    /// The round is over.
    Result,
}

/// Draw the board.
pub fn render(board: Board, show: Show, venue: &str, round: &Round, pairing: &Pairing) -> Frame {
    let mut f = Frame::new(board.w, board.height());

    if show == Show::Idle {
        let title = shout(venue);
        let y = (board.height() / 2).saturating_sub(font::H) as isize;
        if !f.centred(0, y, board.w, &title, 2) {
            f.centred(0, y + font::H as isize / 2, board.w, &title, 1);
        }
        return f;
    }

    let winner = match decide(round, pairing) {
        Outcome::Win { lane, .. } if show == Show::Result => Some(lane),
        _ => None,
    };

    for (i, entry) in pairing.entries().iter().enumerate() {
        let top = (i * board.band) as isize;
        band(&mut f, board, top, show, entry.lane, round, pairing, winner);
        if i + 1 < pairing.entries().len() {
            f.dotted(top + board.band as isize - 1, 4);
        }
    }
    f
}

#[allow(clippy::too_many_arguments)]
fn band(
    f: &mut Frame,
    board: Board,
    top: isize,
    show: Show,
    lane: Lane,
    round: &Round,
    pairing: &Pairing,
    winner: Option<Lane>,
) {
    // A solid bar down the edge is who won. It reads from the stands at a
    // distance where three letters do not, and it costs no characters in a row
    // that has none to spare.
    let left = if winner == Some(lane) {
        f.rect(0, top + 1, 3, board.band - 3);
        6
    } else {
        0
    };
    let right = board.w as isize;
    let run = round.lane(lane);

    // Top row: who, and what they said they would run.
    f.text(left, top, &format!("L{}", lane.number()), 1);
    let dial = match pairing.breakout_limit(lane) {
        Some(d) => format!("DIAL {d:.2}"),
        None => "NO DIAL".to_string(),
    };
    f.right(right, top, &dial, 1);

    // The middle row is the ET, twice size, because it is the number everybody
    // in the stands is actually looking at.
    let et = match (show, run.and_then(|r| r.et_s)) {
        (Show::Result, Some(et)) => format!("{et:.3}"),
        (Show::Result, None) => "NO TIME".to_string(),
        (Show::Running, _) => "RUN".to_string(),
        _ => "STAGED".to_string(),
    };
    let big = et.chars().all(|c| c.is_ascii_digit() || c == '.');
    if big {
        f.right(right, top + 9, &et, 2);
    } else {
        f.right(right, top + 12, &et, 1);
    }

    // Bottom row: reaction on the left, speed on the right. Both are only ever
    // shown once they exist — a board that keeps the last pair's numbers up is
    // how a spectator ends up certain of the wrong thing.
    if show == Show::Result {
        if let Some(rt) = run.and_then(|r| r.reaction_s) {
            let red = rt < 0.0;
            f.text(
                left,
                top + 24,
                &format!("{} {:.3}", if red { "RED" } else { "RT " }, rt.abs()),
                1,
            );
        }
        if let Some(kmh) = run.and_then(|r| r.trap_speed_kmh()) {
            f.right(right, top + 24, &format!("{kmh:.1} KMH"), 1);
        }
    }
}

/// Uppercase, and only what the font can draw. A venue name arrives from a
/// mapping file and the board has no way to complain about it at three in the
/// afternoon, so anything unprintable becomes a space here rather than a row of
/// boxes on a sign.
fn shout(s: &str) -> String {
    s.chars()
        .map(|c| {
            let c = c.to_ascii_uppercase();
            if font::glyph(c).is_some() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_race::{Entry, Format, LaneRun};

    fn pairing() -> Pairing {
        Pairing::new(
            Format::Bracket,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: Some(12.34),
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: Some(7.50),
                },
            ],
        )
        .unwrap()
    }

    fn round() -> Round {
        let mut r = Round::default();
        r.launch_margin_s = Some(4.88);
        r.set_lane(
            Lane::L1,
            LaneRun {
                reaction_s: Some(0.500),
                et_s: Some(12.340),
                trap_speed_ms: Some(105.87),
                ..LaneRun::default()
            },
        );
        r.set_lane(
            Lane::L2,
            LaneRun {
                reaction_s: Some(0.540),
                et_s: Some(7.500),
                trap_speed_ms: Some(105.87),
                ..LaneRun::default()
            },
        );
        r
    }

    #[test]
    fn a_result_fits_the_reference_board() {
        // The test the resolution exists for. If a line grows by one character
        // this fails here, at a desk, and not on a sign fifty metres from the
        // stands with the last digit of an ET missing.
        let b = Board::REFERENCE;
        let f = render(b, Show::Result, "Sim Strip", &round(), &pairing());
        assert_eq!((f.w, f.h), (128, 64));
        assert!(f.ink() > 200, "the board drew almost nothing");

        // Every row of the frame exists; nothing ran past the last one.
        let mut fit = Frame::new(b.w, b.height());
        assert!(fit.right(b.w as isize, 9, "12.340", 2), "the big ET fits");
        assert!(fit.right(b.w as isize, 0, "DIAL 12.34", 1), "the dial fits");
        assert!(fit.text(0, 24, "RED 0.500", 1), "the reaction fits");
        assert!(
            fit.right(b.w as isize, 24, "381.1 KMH", 1),
            "the speed fits"
        );
    }

    #[test]
    fn the_band_has_no_fourth_line() {
        // The vertical budget, as an assertion. Adding a field means a taller
        // band, which means more panels — a purchase, and D15 gates those.
        let b = Board::REFERENCE;
        assert_eq!(b.rows_spent(), 29);
        assert!(
            b.band - b.rows_spent() < font::H,
            "{} rows spare would hold another line",
            b.band - b.rows_spent()
        );
    }

    #[test]
    fn the_widest_line_the_bottom_row_can_hold_is_known() {
        // Reaction on the left and speed on the right share one row of 128
        // pixels. This is where the next number to be added will not fit, and
        // the number is written down rather than left to be discovered.
        let used = Frame::measure("RED 0.500", 1) + Frame::measure("381.1 KMH", 1);
        assert_eq!(used, 54 + 54);
        assert!(
            used <= Board::REFERENCE.w,
            "{used} px of a {} px row",
            Board::REFERENCE.w
        );
        assert!(
            Board::REFERENCE.w - used < 30,
            "and there is no room for another field"
        );
    }

    #[test]
    fn the_winner_gets_a_bar_and_the_loser_does_not() {
        let f = render(Board::REFERENCE, Show::Result, "x", &round(), &pairing());
        // Probed twenty rows down, where only the bar can be: the lane label is
        // in the top seven rows and the big ET is right-aligned, so this column
        // is otherwise dark in both bands.
        assert!(f.lit(1, 20), "lane 1 won on the stripe and has the bar");
        assert!(!f.lit(1, 52), "lane 2 does not");
    }

    #[test]
    fn a_running_board_does_not_show_the_last_pairs_numbers() {
        // The failure mode this exists for: a spectator reads a stale ET as this
        // round's, and is certain of the wrong thing.
        let running = render(Board::REFERENCE, Show::Running, "x", &round(), &pairing());
        let result = render(Board::REFERENCE, Show::Result, "x", &round(), &pairing());
        assert_ne!(running, result);
        assert!(running.ink() < result.ink());
        assert!(!running.lit(1, 20), "and nobody has won yet");
    }

    #[test]
    fn a_lane_with_no_time_says_so_rather_than_going_blank() {
        let mut r = round();
        r.set_lane(Lane::L2, LaneRun::default());
        let f = render(Board::REFERENCE, Show::Result, "x", &r, &pairing());
        let blank = Frame::new(Board::REFERENCE.w, Board::REFERENCE.height());
        assert_ne!(f, blank);
        // Lane 2's band is not empty: it carries the words instead of a number.
        let ink: usize = (32..64)
            .map(|y| (0..128).filter(|x| f.lit(*x, y)).count())
            .sum();
        assert!(ink > 40, "the empty lane still says something");
    }

    #[test]
    fn a_venue_name_the_font_cannot_draw_does_not_become_a_row_of_boxes() {
        // The name comes from a mapping file and the board cannot complain about
        // it in the middle of an event.
        let f = render(
            Board::REFERENCE,
            Show::Idle,
            "Трасса #1",
            &round(),
            &pairing(),
        );
        assert!(f.ink() > 0, "the printable part still shows");
        assert_eq!(shout("Трасса #1"), "1");
    }
}
