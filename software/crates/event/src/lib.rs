#![forbid(unsafe_code)]

//! The event around the racing: entries, classes, qualifying, ladders, byes.
//!
//! `architecture.md` §9 and `software.md` §4 both list this and neither says how
//! it works, because how it works is a club's business. **D23** is the rule that
//! shapes everything here: *a club changing a class rule must never see a
//! compiler.* So a class is data — its format, how it qualifies, which ladder it
//! runs, who gets lane choice — and this crate is the arithmetic those choices
//! imply and nothing else.
//!
//! Pure, like the rest of race control below the bus: no clock, no file, no
//! port. An event replays from its results exactly.
//!
//! ## What it will not decide
//!
//! Who won a round. That is [`beam402_race`], settled by measurements, and it
//! arrives here as a fact. A ladder that re-derived an outcome would be a second
//! implementation of the rules, which is one more than anybody can keep right.

use std::collections::BTreeMap;

pub mod desk;
pub mod ladder;
pub mod progress;
pub mod sheet;
pub mod sync;

pub use ladder::{Pair, Style};
pub use progress::{OnDeck, Progress, Refused};
pub use sheet::{Record, Sheet};
pub use sync::{Appended, Cursor, Held, SyncError};

use beam402_race::Format;

/// A qualifying position, 1-based. Seed 1 is the top qualifier.
pub type Seed = usize;

/// What a round is called, from how many pairs are left in it.
///
/// The last three rounds have names everybody at a track uses and the earlier
/// ones do not, so this is the naming and not a numbering with exceptions.
pub fn round_name(pairs: usize, number: usize) -> String {
    match pairs {
        1 => "final".to_string(),
        2 => "semi-final".to_string(),
        4 => "quarter-final".to_string(),
        _ => format!("round {number}"),
    }
}

/// An entry's identity within an event. Stable across rounds, unlike the seed,
/// which does not exist until qualifying is over.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct EntryId(pub u32);

#[derive(Clone, PartialEq, Debug)]
pub struct Entry {
    pub id: EntryId,
    pub driver: String,
    pub car: String,
    /// The driver's predicted ET. Required by a bracket class and ignored by the
    /// others.
    pub dial_s: Option<f64>,
}

/// How a field is ordered before the ladder is drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Seeding {
    /// Quickest ET qualifies best. Heads-up classes and index classes.
    QuickestEt,
    /// Closest to the dial-in, which is what a bracket class is actually
    /// measuring — a bracket racer's ET is their prediction, not their speed, so
    /// ordering a bracket field by ET would rank the fast cars first for no
    /// reason connected to the racing.
    ClosestToDial,
    /// As entered. Small club events that do not qualify at all.
    EntryOrder,
    /// A draw. Deterministic from the seed, so the same event replays to the
    /// same ladder and a disputed pairing can be re-derived rather than argued.
    Draw { seed: u64 },
}

/// Who picks a lane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LaneChoice {
    /// The better qualifier, every round.
    BetterQualifier,
    /// The better qualifier in round one; after that, whoever was closer to
    /// their dial in the previous round. Common in bracket racing and it needs
    /// the previous round's result, so it is asked for by name.
    PreviousRound,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Class {
    pub name: String,
    pub format: Format,
    pub seeding: Seeding,
    pub ladder: Style,
    pub lane_choice: LaneChoice,
    /// A class rule, not a fault (`software.md` §4).
    pub deep_staging: bool,
    /// The slow end of a class defined as a time window. `Format::Index`'s number
    /// is the quick end.
    pub slowest_s: Option<f64>,
    /// The field sizes this class runs, ascending. The largest one the entry list
    /// fills is the field; empty is everybody.
    pub field: Vec<usize>,
    /// Below this the class does not run.
    pub min_entries: usize,
    /// How many passes score. `None` is all of them.
    pub attempts: Option<usize>,
}

/// One qualifying attempt.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Attempt {
    pub entry: EntryId,
    /// `None` when the car did not finish. It still qualifies — at the back.
    pub et_s: Option<f64>,
    /// What the driver dialled for this attempt.
    pub dial_s: Option<f64>,
    /// A red light does not void a qualifying time under most rulebooks, but it
    /// is carried so a club whose rules differ has the fact to hand.
    pub red: bool,
    /// A `V` line said this pass does not count (**D37**). It loses its **time**
    /// and not the **attempt**: the run down the track was spent.
    pub void: bool,
}

/// The ordered field: index 0 is seed 1.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    order: Vec<EntryId>,
}

impl Field {
    /// Order a field from every attempt made.
    ///
    /// Each entry keeps its **best** attempt by the class's own measure, which
    /// is why the rule cannot be applied to a pre-reduced list of times: "best"
    /// means quickest in one class and closest to the dial in another.
    pub fn qualify(class: &Class, entries: &[Entry], attempts: &[Attempt]) -> Field {
        let mut best: BTreeMap<EntryId, f64> = BTreeMap::new();
        // Passes taken, whether or not they produced a number: a run down the
        // track is a run down the track, and a rulebook that gives three attempts
        // does not give a fourth to whoever broke on the first.
        let mut taken: BTreeMap<EntryId, usize> = BTreeMap::new();
        for a in attempts {
            let n = taken.entry(a.entry).or_insert(0);
            *n += 1;
            // Rulebooks say *scoring* attempts and mean it. A pass beyond the
            // limit is not forbidden — it is in the log like any other — it just
            // does not count towards the field.
            if class.attempts.is_some_and(|max| *n > max) {
                continue;
            }
            // A voided pass is counted above and scored nowhere: the attempt was
            // spent, the time does not exist (**D37**).
            if a.void {
                continue;
            }
            let Some(score) = score(class.seeding, a) else {
                continue;
            };
            best.entry(a.entry)
                .and_modify(|b| {
                    if score < *b {
                        *b = score
                    }
                })
                .or_insert(score);
        }

        let mut order: Vec<EntryId> = entries.iter().map(|e| e.id).collect();
        match class.seeding {
            Seeding::EntryOrder => {}
            Seeding::Draw { seed } => shuffle(&mut order, seed),
            _ => {
                // A car with no usable attempt qualifies last rather than not at
                // all: it is entered, it paid, and a rulebook that drops it says
                // so itself. Ties keep entry order, so the result is stable.
                order.sort_by(|a, b| {
                    let (x, y) = (best.get(a), best.get(b));
                    match (x, y) {
                        (Some(x), Some(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                });
            }
        }

        // **The cut.** Everything above is an ordering; this is a rulebook, and it
        // is the difference between "seeded last" and "did not qualify". Cars with
        // no usable pass sort last, so they are what a cut removes first — which
        // is the same sentence read from the other end.
        //
        // It needs no new record: `draw` writes the order down, so the field a
        // ladder was drawn on is in the log whether it was cut or not.
        // The largest listed size the entry list fills. Filling none of them is
        // not an error and not an empty field: it is a small turnout, and the
        // rulebook's smallest bracket runs with byes in it.
        if let Some(&size) = class.field.iter().rev().find(|&&size| size <= order.len()) {
            order.truncate(size);
        }
        Field { order }
    }

    /// A field in a fixed order, for a club that does not qualify.
    pub fn from_order(order: Vec<EntryId>) -> Field {
        Field { order }
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The entry holding this seed. Seeds are 1-based.
    pub fn entry(&self, seed: Seed) -> Option<EntryId> {
        self.order.get(seed.checked_sub(1)?).copied()
    }

    pub fn seed_of(&self, entry: EntryId) -> Option<Seed> {
        self.order.iter().position(|e| *e == entry).map(|i| i + 1)
    }

    pub fn seeds(&self) -> impl Iterator<Item = (Seed, EntryId)> + '_ {
        self.order
            .iter()
            .copied()
            .enumerate()
            .map(|(i, e)| (i + 1, e))
    }
}

fn score(seeding: Seeding, a: &Attempt) -> Option<f64> {
    match seeding {
        Seeding::QuickestEt => a.et_s,
        // The absolute distance from the dial. Some clubs disallow a breakout in
        // qualifying and would want only the positive side; that is a rule, so it
        // belongs in configuration rather than here, and this is the common case.
        Seeding::ClosestToDial => Some((a.et_s? - a.dial_s?).abs()),
        Seeding::EntryOrder | Seeding::Draw { .. } => None,
    }
}

/// SplitMix64 and a Fisher–Yates shuffle, written out.
///
/// The requirement is not that the draw be unpredictable — it is that the *same*
/// seed give the same ladder forever, so a disputed pairing can be re-derived a
/// season later. A dependency that improved its generator would break that.
fn shuffle(items: &mut [EntryId], seed: u64) {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..items.len()).rev() {
        items.swap(i, (next() % (i as u64 + 1)) as usize);
    }
}

/// A round in progress: who is racing whom, and who has won so far.
#[derive(Clone, PartialEq, Debug)]
pub struct Round {
    pub number: usize,
    pub pairs: Vec<Pair>,
    won: BTreeMap<usize, Seed>,
}

impl Round {
    /// The first round of a class.
    pub fn open(class: &Class, field: &Field) -> Round {
        Round {
            number: 1,
            pairs: ladder::first_round(&class.ladder, field.len()),
            won: BTreeMap::new(),
        }
    }

    /// Record a winner. The outcome comes from [`beam402_race`]; nothing here
    /// re-derives it.
    pub fn win(&mut self, position: usize, seed: Seed) -> Result<(), Error> {
        let pair = self
            .pairs
            .iter()
            .find(|p| p.position == position)
            .ok_or(Error::NoSuchPair(position))?;
        if !pair.has(seed) {
            return Err(Error::NotInThisPair { position, seed });
        }
        self.won.insert(position, seed);
        Ok(())
    }

    /// A bye is a win, but only if the driver made a run. Most rulebooks require
    /// the car to make a full pass to advance, which is why this is a call and
    /// not an assumption.
    pub fn bye(&mut self, position: usize, completed: bool) -> Result<(), Error> {
        let pair = self
            .pairs
            .iter()
            .find(|p| p.position == position)
            .ok_or(Error::NoSuchPair(position))?;
        if !pair.is_bye() {
            return Err(Error::NotABye(position));
        }
        if completed {
            let seed = pair.left;
            self.won.insert(position, seed);
        }
        Ok(())
    }

    pub fn winner(&self, position: usize) -> Option<Seed> {
        self.won.get(&position).copied()
    }

    pub fn outstanding(&self) -> Vec<&Pair> {
        self.pairs
            .iter()
            .filter(|p| !self.won.contains_key(&p.position))
            .collect()
    }

    pub fn is_complete(&self) -> bool {
        self.outstanding().is_empty()
    }

    /// Who is still in, in the order the pairs were run — which is what the
    /// fixed-bracket styles need to pair neighbours correctly.
    pub fn survivors(&self) -> Vec<Seed> {
        let mut pairs: Vec<&Pair> = self.pairs.iter().collect();
        pairs.sort_by_key(|p| p.position);
        pairs
            .iter()
            .filter_map(|p| self.won.get(&p.position).copied())
            .collect()
    }

    /// The next round, or `None` when this one settled the class.
    pub fn advance(&self, class: &Class) -> Result<Option<Round>, Error> {
        if !self.is_complete() {
            return Err(Error::RoundUnfinished(self.outstanding().len()));
        }
        let pairs = ladder::next_round(&class.ladder, self.number, &self.survivors());
        if pairs.is_empty() {
            return Ok(None);
        }
        Ok(Some(Round {
            number: self.number + 1,
            pairs,
            won: BTreeMap::new(),
        }))
    }

    /// Which car picks a lane, by the class's rule.
    ///
    /// `closer_to_dial` answers, for [`LaneChoice::PreviousRound`], which of the
    /// two ran nearer their dial last time out. It is passed in rather than
    /// worked out here because it is a fact about a previous *round*, and this
    /// type only knows about its own.
    pub fn lane_choice(
        &self,
        class: &Class,
        pair: &Pair,
        closer_to_dial: Option<Seed>,
    ) -> Option<Seed> {
        let right = pair.right?;
        match class.lane_choice {
            LaneChoice::BetterQualifier => Some(pair.left.min(right)),
            LaneChoice::PreviousRound => {
                if self.number == 1 {
                    Some(pair.left.min(right))
                } else {
                    // No previous-round fact means no basis to award it, and
                    // guessing would hand somebody an advantage they did not
                    // earn. The operator decides.
                    closer_to_dial.filter(|s| pair.has(*s))
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    NoSuchPair(usize),
    NotInThisPair { position: usize, seed: Seed },
    NotABye(usize),
    RoundUnfinished(usize),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::NoSuchPair(p) => write!(f, "this round has no pair at position {p}"),
            Error::NotInThisPair { position, seed } => {
                write!(f, "seed {seed} is not in the pair at position {position}")
            }
            Error::NotABye(p) => write!(f, "the pair at position {p} has two cars"),
            Error::RoundUnfinished(n) => write!(f, "{n} pairs have not run yet"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: u32) -> Vec<Entry> {
        (1..=n)
            .map(|i| Entry {
                id: EntryId(i),
                driver: format!("driver {i}"),
                car: format!("car {i}"),
                dial_s: Some(10.0 + i as f64 / 10.0),
            })
            .collect()
    }

    fn class(seeding: Seeding, style: Style) -> Class {
        Class {
            name: "test".into(),
            format: Format::Bracket,
            seeding,
            ladder: style,
            lane_choice: LaneChoice::BetterQualifier,
            deep_staging: false,
            slowest_s: None,
            field: Vec::new(),
            min_entries: 0,
            attempts: None,
        }
    }

    /// One `et` each, quickest first, so the cut is the only thing under test.
    fn ranked(n: u32) -> Vec<Attempt> {
        (1..=n)
            .map(|i| Attempt {
                entry: EntryId(i),
                et_s: Some(10.0 + i as f64 / 100.0),
                dial_s: None,
                red: false,
                void: false,
            })
            .collect()
    }

    /// The rule a rulebook states as two sentences — "top 8 from four entries,
    /// top 16 from sixteen" — as one class setting. The cases that matter are the
    /// small ones: with six entered, a rulebook that says "top 8" means everybody,
    /// and a cut that took four instead would send two people home for nothing.
    #[test]
    fn the_field_is_the_largest_size_the_entry_list_fills() {
        let mut c = class(Seeding::QuickestEt, Style::Pro);
        c.field = vec![8, 16];
        for (entered, expected) in [
            (4, 4),
            (6, 6),
            (8, 8),
            (12, 8),
            (15, 8),
            (16, 16),
            (20, 16),
            (40, 16),
        ] {
            let f = Field::qualify(&c, &entries(entered), &ranked(entered));
            assert_eq!(
                f.seeds().count(),
                expected,
                "{entered} entered should qualify {expected}"
            );
        }
    }

    #[test]
    fn a_full_series_of_sizes_is_the_largest_bracket_that_fills() {
        // The other rule a club might write: no byes, so the field is the largest
        // power of two — which is the same setting with more sizes in it.
        let mut c = class(Seeding::QuickestEt, Style::Pro);
        c.field = vec![2, 4, 8, 16, 32];
        for (entered, expected) in [(3, 2), (6, 4), (12, 8), (31, 16)] {
            let f = Field::qualify(&c, &entries(entered), &ranked(entered));
            assert_eq!(f.seeds().count(), expected, "{entered} entered");
        }
    }

    #[test]
    fn only_the_scoring_passes_count_and_the_rest_are_still_run() {
        // "Количество зачётных попыток ограничено тремя" — *scoring* attempts. A
        // fourth pass is not forbidden by that sentence, it simply does not count,
        // so the quickest run of the day can be one that does not qualify anybody.
        let mut c = class(Seeding::QuickestEt, Style::Pro);
        c.attempts = Some(2);
        let attempts = vec![
            Attempt {
                entry: EntryId(1),
                et_s: Some(10.50),
                dial_s: None,
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(1),
                et_s: Some(10.40),
                dial_s: None,
                red: false,
                void: false,
            },
            // Third pass, quicker than both, and out of the reckoning.
            Attempt {
                entry: EntryId(1),
                et_s: Some(9.00),
                dial_s: None,
                red: false,
                void: false,
            },
            // Quicker than entry 1's two scoring passes, slower than the pass
            // that does not count — which is what makes the seeding depend on
            // the rule rather than on the numbers.
            Attempt {
                entry: EntryId(2),
                et_s: Some(10.20),
                dial_s: None,
                red: false,
                void: false,
            },
        ];
        let f = Field::qualify(&c, &entries(2), &attempts);
        assert_eq!(
            f.seeds().map(|(_, id)| id).collect::<Vec<_>>(),
            vec![EntryId(2), EntryId(1)],
            "entry 1's 9.00 was its third pass and does not score"
        );

        // And with no limit the same passes give the other ladder.
        c.attempts = None;
        let f = Field::qualify(&c, &entries(2), &attempts);
        assert_eq!(
            f.seeds().map(|(_, id)| id).collect::<Vec<_>>(),
            vec![EntryId(1), EntryId(2)]
        );
    }

    #[test]
    fn a_pass_that_produced_nothing_still_used_an_attempt_up() {
        // A rulebook that gives three attempts does not give a fourth to whoever
        // broke on the first — the run down the track is what was spent.
        let mut c = class(Seeding::QuickestEt, Style::Pro);
        c.attempts = Some(2);
        let attempts = vec![
            Attempt {
                entry: EntryId(1),
                et_s: None,
                dial_s: None,
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(1),
                et_s: Some(10.40),
                dial_s: None,
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(1),
                et_s: Some(9.00),
                dial_s: None,
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(2),
                et_s: Some(10.20),
                dial_s: None,
                red: false,
                void: false,
            },
        ];
        let f = Field::qualify(&c, &entries(2), &attempts);
        assert_eq!(
            f.seeds().map(|(_, id)| id).collect::<Vec<_>>(),
            vec![EntryId(2), EntryId(1)],
            "the dnf was attempt one, so 9.00 was attempt three"
        );
    }

    #[test]
    fn a_heads_up_field_is_ordered_by_et_and_a_bracket_field_is_not() {
        // The distinction the two seeding rules exist for. Entry 3 is the
        // quickest car and entry 1 is the one that drove closest to its dial;
        // they are different questions and they give different ladders.
        let e = entries(3);
        let attempts = vec![
            Attempt {
                entry: EntryId(1),
                et_s: Some(10.11),
                dial_s: Some(10.10),
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(2),
                et_s: Some(10.40),
                dial_s: Some(10.20),
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(3),
                et_s: Some(9.50),
                dial_s: Some(10.30),
                red: false,
                void: false,
            },
        ];

        let quick = Field::qualify(&class(Seeding::QuickestEt, Style::Pro), &e, &attempts);
        assert_eq!(quick.entry(1), Some(EntryId(3)), "quickest car is seed 1");

        let bracket = Field::qualify(&class(Seeding::ClosestToDial, Style::Pro), &e, &attempts);
        assert_eq!(bracket.entry(1), Some(EntryId(1)), "0.01 off the dial");
        assert_eq!(bracket.entry(3), Some(EntryId(3)), "0.80 off it");
    }

    #[test]
    fn a_driver_keeps_their_best_attempt_by_the_classs_own_measure() {
        let e = entries(2);
        let attempts = vec![
            Attempt {
                entry: EntryId(1),
                et_s: Some(11.00),
                dial_s: Some(10.10),
                red: false,
                void: false,
            },
            // Second attempt, much closer to the dial.
            Attempt {
                entry: EntryId(1),
                et_s: Some(10.12),
                dial_s: Some(10.10),
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(2),
                et_s: Some(10.25),
                dial_s: Some(10.20),
                red: false,
                void: false,
            },
        ];
        let f = Field::qualify(&class(Seeding::ClosestToDial, Style::Pro), &e, &attempts);
        assert_eq!(f.entry(1), Some(EntryId(1)), "0.02 beats 0.05");
    }

    #[test]
    fn a_car_that_never_made_a_time_qualifies_last_and_not_never() {
        // It is entered and it paid. A rulebook that drops it says so itself.
        let e = entries(3);
        let attempts = vec![
            Attempt {
                entry: EntryId(2),
                et_s: Some(10.5),
                dial_s: Some(10.5),
                red: false,
                void: false,
            },
            Attempt {
                entry: EntryId(3),
                et_s: None,
                dial_s: Some(10.6),
                red: false,
                void: false,
            },
        ];
        let f = Field::qualify(&class(Seeding::QuickestEt, Style::Pro), &e, &attempts);
        assert_eq!(f.len(), 3);
        assert_eq!(f.entry(1), Some(EntryId(2)));
        assert_eq!(f.seed_of(EntryId(3)).unwrap(), 3);
    }

    #[test]
    fn the_same_draw_seed_gives_the_same_ladder_forever() {
        // A disputed pairing has to be re-derivable a season later, which is the
        // same argument D26 makes about a disputed ET.
        let e = entries(8);
        let one = Field::qualify(&class(Seeding::Draw { seed: 7 }, Style::Pro), &e, &[]);
        let two = Field::qualify(&class(Seeding::Draw { seed: 7 }, Style::Pro), &e, &[]);
        let other = Field::qualify(&class(Seeding::Draw { seed: 8 }, Style::Pro), &e, &[]);
        assert_eq!(one, two);
        assert_ne!(one, other);
        // Everybody is still in it.
        let mut ids: Vec<EntryId> = one.seeds().map(|(_, id)| id).collect();
        ids.sort();
        assert_eq!(ids, e.iter().map(|x| x.id).collect::<Vec<_>>());
    }

    #[test]
    fn a_class_runs_from_first_round_to_a_winner() {
        let c = class(Seeding::QuickestEt, Style::Pro);
        let field = Field::from_order(entries(8).iter().map(|e| e.id).collect());
        let mut round = Round::open(&c, &field);
        let mut rounds = 0;

        loop {
            rounds += 1;
            for p in round.pairs.clone() {
                // The better qualifier wins every time, so the top seed takes it.
                let winner = match p.right {
                    Some(r) => p.left.min(r),
                    None => p.left,
                };
                round.win(p.position, winner).unwrap();
            }
            match round.advance(&c).unwrap() {
                Some(next) => round = next,
                None => break,
            }
        }
        assert_eq!(rounds, 3, "eight cars is three rounds");
        assert_eq!(round.survivors(), vec![1], "seed 1 wins the class");
    }

    #[test]
    fn a_round_will_not_advance_with_a_pair_still_out_there() {
        // The failure this prevents is a ladder that quietly drops a car because
        // somebody clicked Next before the last pair came back.
        let c = class(Seeding::QuickestEt, Style::Pro);
        let field = Field::from_order(entries(4).iter().map(|e| e.id).collect());
        let mut round = Round::open(&c, &field);
        round.win(0, 1).unwrap();
        assert_eq!(round.advance(&c), Err(Error::RoundUnfinished(1)));
        assert_eq!(round.outstanding().len(), 1);
    }

    #[test]
    fn a_winner_who_was_not_in_the_pair_is_refused() {
        let c = class(Seeding::QuickestEt, Style::Pro);
        let field = Field::from_order(entries(4).iter().map(|e| e.id).collect());
        let mut round = Round::open(&c, &field);
        assert_eq!(
            round.win(0, 3),
            Err(Error::NotInThisPair {
                position: 0,
                seed: 3
            })
        );
    }

    #[test]
    fn a_bye_still_has_to_be_run() {
        // Most rulebooks require the car to make a full pass to advance, and a
        // system that advanced it automatically would quietly change the rule.
        let c = class(Seeding::QuickestEt, Style::Pro);
        let field = Field::from_order(entries(3).iter().map(|e| e.id).collect());
        let mut round = Round::open(&c, &field);
        let bye = round
            .pairs
            .iter()
            .find(|p| p.is_bye())
            .expect("three cars on a four-car ladder is one bye")
            .position;

        round.bye(bye, false).unwrap();
        assert!(round.winner(bye).is_none(), "no run, no advance");
        round.bye(bye, true).unwrap();
        assert_eq!(round.winner(bye), Some(1));
    }

    #[test]
    fn lane_choice_without_a_previous_round_is_not_guessed() {
        // Handing it to somebody on no basis is handing them an advantage they
        // did not earn. The operator decides.
        let mut c = class(Seeding::QuickestEt, Style::Pro);
        c.lane_choice = LaneChoice::PreviousRound;
        let field = Field::from_order(entries(4).iter().map(|e| e.id).collect());
        let first = Round::open(&c, &field);
        let pair = first.pairs[0];
        assert_eq!(
            first.lane_choice(&c, &pair, None),
            Some(1),
            "round one falls back to the better qualifier"
        );

        let mut round = first;
        round.win(0, 1).unwrap();
        round.win(1, 2).unwrap();
        let second = round.advance(&c).unwrap().unwrap();
        let final_pair = second.pairs[0];
        assert_eq!(second.lane_choice(&c, &final_pair, None), None);
        assert_eq!(second.lane_choice(&c, &final_pair, Some(2)), Some(2));
    }
}
