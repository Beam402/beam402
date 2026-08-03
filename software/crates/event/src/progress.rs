//! The day, rebuilt from what was recorded.
//!
//! Nothing here is stored. Who qualified where, which round a class is on, who is
//! still in it — all of it is **derived** by replaying [`Record`]s in order, and
//! that is the same argument **D26** makes about a bus session: a ladder rebuilt
//! from the results that produced it can be checked, while one kept in a file
//! that is rewritten as it goes can only be trusted.
//!
//! It decides the failure mode too. Race control loses power between rounds and
//! comes back exactly where it was, because the last thing written was a line
//! about a round that *finished* rather than a snapshot of one in progress.

use std::collections::BTreeMap;

use beam402_protocol::Lane;
use beam402_race::{Entry as RaceEntry, Pairing, PairingError};

use crate::sheet::{Record, Sheet};
use crate::{Attempt, Class, Entry, EntryId, Field, Round, Seed};

/// One pair, ready to be sent down the track.
#[derive(Clone, Debug)]
pub struct OnDeck {
    pub class: String,
    pub round: usize,
    pub position: usize,
    /// The seed with lane choice, and therefore the one placed in lane 1 unless
    /// somebody says otherwise. Lane choice is a right, and exercising it is an
    /// operator's call rather than an automatic one.
    pub left: Seed,
    pub right: Option<Seed>,
    pub entries: Vec<(Seed, EntryId)>,
    /// Who picks a lane, by the class's rule. `None` when the rule needs a
    /// previous round that has not happened.
    pub lane_choice: Option<Seed>,
}

impl OnDeck {
    pub fn is_bye(&self) -> bool {
        self.right.is_none()
    }
}

#[derive(Clone, Debug, Default)]
struct ClassState {
    attempts: Vec<Attempt>,
    field: Option<Field>,
    round: Option<Round>,
    /// The class is over and this seed won it.
    champion: Option<Seed>,
}

/// The whole meeting: the sheet it was entered on, and everything recorded since.
pub struct Progress {
    sheet: Sheet,
    classes: BTreeMap<String, ClassState>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Refused {
    NoSuchClass(String),
    NotDrawnYet(String),
    AlreadyDrawn(String),
    ClassFinished(String),
    NoSuchPair {
        class: String,
        position: usize,
    },
    /// Recording a winner who was not in that pair.
    NotInThisPair {
        position: usize,
        seed: Seed,
    },
    Pairing(PairingError),
}

impl core::fmt::Display for Refused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Refused::NoSuchClass(c) => write!(f, "no class named {c:?}"),
            Refused::NotDrawnYet(c) => write!(f, "{c:?} has not drawn its ladder yet"),
            Refused::AlreadyDrawn(c) => write!(f, "{c:?} has already drawn its ladder"),
            Refused::ClassFinished(c) => write!(f, "{c:?} is over"),
            Refused::NoSuchPair { class, position } => {
                write!(f, "{class:?} has no pair at position {position} this round")
            }
            Refused::NotInThisPair { position, seed } => {
                write!(f, "seed {seed} is not in the pair at position {position}")
            }
            Refused::Pairing(e) => write!(f, "{e}"),
        }
    }
}

impl Progress {
    /// A meeting that has not started.
    pub fn new(sheet: Sheet) -> Progress {
        let classes = sheet
            .classes
            .iter()
            .map(|c| (c.name.clone(), ClassState::default()))
            .collect();
        Progress { sheet, classes }
    }

    /// A meeting rebuilt from its log. Unparseable lines are **skipped**, and
    /// the count is returned rather than swallowed: a truncated final line after
    /// a power cut is normal, and a hundred of them is a corrupted file that
    /// somebody has to look at.
    pub fn replay(sheet: Sheet, log: &str) -> (Progress, usize) {
        let mut p = Progress::new(sheet);
        let mut skipped = 0;
        for line in log.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match Record::parse(line) {
                Some(r) => p.apply(&r),
                None => skipped += 1,
            }
        }
        (p, skipped)
    }

    pub fn sheet(&self) -> &Sheet {
        &self.sheet
    }

    pub fn class_names(&self) -> impl Iterator<Item = &str> {
        self.sheet.classes.iter().map(|c| c.name.as_str())
    }

    fn class(&self, name: &str) -> Result<Class, Refused> {
        self.sheet
            .class(name)
            .ok_or_else(|| Refused::NoSuchClass(name.to_string()))
    }

    /// Fold one record in. Replay and live recording share this, so a day
    /// rebuilt from the log is the same day that was run.
    fn apply(&mut self, record: &Record) {
        let name = match record {
            Record::Qualified { class, .. }
            | Record::Drawn { class, .. }
            | Record::Won { class, .. }
            | Record::Bye { class, .. } => class.clone(),
        };
        let Ok(class) = self.class(&name) else { return };
        let Some(state) = self.classes.get_mut(&name) else {
            return;
        };

        match record {
            Record::Qualified {
                entry,
                et_s,
                dial_s,
                red,
                ..
            } => state.attempts.push(Attempt {
                entry: *entry,
                et_s: *et_s,
                dial_s: *dial_s,
                red: *red,
            }),
            Record::Drawn { order, .. } => {
                let field = Field::from_order(order.clone());
                state.round = Some(Round::open(&class, &field));
                state.field = Some(field);
            }
            Record::Won { position, seed, .. } => {
                if let Some(round) = state.round.as_mut() {
                    let _ = round.win(*position, *seed);
                }
                Self::advance(state, &class);
            }
            Record::Bye {
                position,
                completed,
                ..
            } => {
                if let Some(round) = state.round.as_mut() {
                    let _ = round.bye(*position, *completed);
                }
                Self::advance(state, &class);
            }
        }
    }

    fn advance(state: &mut ClassState, class: &Class) {
        let Some(round) = state.round.as_ref() else {
            return;
        };
        if !round.is_complete() {
            return;
        }
        match round.advance(class) {
            Ok(Some(next)) => state.round = Some(next),
            // Nothing left to pair: whoever is standing has won the class.
            Ok(None) => {
                state.champion = round.survivors().first().copied();
            }
            Err(_) => {}
        }
    }

    // -- recording -------------------------------------------------------

    /// Record a qualifying attempt. Returns the line to append.
    pub fn qualified(
        &mut self,
        class: &str,
        entry: EntryId,
        et_s: Option<f64>,
        dial_s: Option<f64>,
        red: bool,
    ) -> Result<Record, Refused> {
        self.class(class)?;
        if self.classes.get(class).is_some_and(|s| s.field.is_some()) {
            return Err(Refused::AlreadyDrawn(class.into()));
        }
        let r = Record::Qualified {
            class: class.into(),
            entry,
            et_s,
            dial_s,
            red,
        };
        self.apply(&r);
        Ok(r)
    }

    /// Close qualifying and draw the ladder.
    ///
    /// The order is written down rather than recomputed later, so a late
    /// qualifying slip cannot silently reshuffle a ladder that people have
    /// already been told about.
    pub fn draw(&mut self, class: &str) -> Result<Record, Refused> {
        let c = self.class(class)?;
        let state = self
            .classes
            .get(class)
            .ok_or_else(|| Refused::NoSuchClass(class.into()))?;
        if state.field.is_some() {
            return Err(Refused::AlreadyDrawn(class.into()));
        }
        let entries: Vec<Entry> = self.sheet.entries_in(class);
        let field = Field::qualify(&c, &entries, &state.attempts);
        let r = Record::Drawn {
            class: class.into(),
            order: field.seeds().map(|(_, e)| e).collect(),
        };
        self.apply(&r);
        Ok(r)
    }

    /// Record who won a pair.
    pub fn won(&mut self, class: &str, position: usize, seed: Seed) -> Result<Record, Refused> {
        let state = self
            .classes
            .get(class)
            .ok_or_else(|| Refused::NoSuchClass(class.into()))?;
        let round = state
            .round
            .as_ref()
            .ok_or_else(|| Refused::NotDrawnYet(class.into()))?;
        let pair = round
            .pairs
            .iter()
            .find(|p| p.position == position)
            .ok_or_else(|| Refused::NoSuchPair {
                class: class.into(),
                position,
            })?;
        if !pair.has(seed) {
            return Err(Refused::NotInThisPair { position, seed });
        }
        let r = Record::Won {
            class: class.into(),
            round: round.number,
            position,
            seed,
        };
        self.apply(&r);
        Ok(r)
    }

    /// Record a bye. `completed` is whether the car actually made the pass —
    /// most rulebooks require one to advance, so it is asked rather than assumed.
    pub fn bye(
        &mut self,
        class: &str,
        position: usize,
        completed: bool,
    ) -> Result<Record, Refused> {
        let state = self
            .classes
            .get(class)
            .ok_or_else(|| Refused::NoSuchClass(class.into()))?;
        let round = state
            .round
            .as_ref()
            .ok_or_else(|| Refused::NotDrawnYet(class.into()))?;
        let r = Record::Bye {
            class: class.into(),
            round: round.number,
            position,
            completed,
        };
        self.apply(&r);
        Ok(r)
    }

    // -- reading ---------------------------------------------------------

    pub fn is_drawn(&self, class: &str) -> bool {
        self.classes.get(class).is_some_and(|s| s.field.is_some())
    }

    pub fn champion(&self, class: &str) -> Option<Seed> {
        self.classes.get(class).and_then(|s| s.champion)
    }

    pub fn round_number(&self, class: &str) -> Option<usize> {
        self.classes
            .get(class)
            .and_then(|s| s.round.as_ref())
            .map(|r| r.number)
    }

    pub fn field(&self, class: &str) -> Option<&Field> {
        self.classes.get(class).and_then(|s| s.field.as_ref())
    }

    pub fn round(&self, class: &str) -> Option<&Round> {
        self.classes.get(class).and_then(|s| s.round.as_ref())
    }

    /// The next pair that has not run, in any class — the operator's queue.
    pub fn next_pair(&self) -> Option<OnDeck> {
        self.class_names()
            .collect::<Vec<_>>()
            .into_iter()
            .find_map(|c| self.next_pair_in(c))
    }

    pub fn next_pair_in(&self, class: &str) -> Option<OnDeck> {
        let state = self.classes.get(class)?;
        if state.champion.is_some() {
            return None;
        }
        let round = state.round.as_ref()?;
        let field = state.field.as_ref()?;
        let c = self.sheet.class(class)?;
        let pair = round.outstanding().first().copied().copied()?;

        let mut entries = Vec::new();
        for seed in [Some(pair.left), pair.right].into_iter().flatten() {
            if let Some(id) = field.entry(seed) {
                entries.push((seed, id));
            }
        }
        Some(OnDeck {
            class: class.to_string(),
            round: round.number,
            position: pair.position,
            left: pair.left,
            right: pair.right,
            entries,
            lane_choice: round.lane_choice(&c, &pair, None),
        })
    }

    /// The pair, as the race logic wants it: lanes, dials and a format.
    ///
    /// Lane assignment is the caller's, because lane **choice** is a right the
    /// operator exercises rather than something to work out — `swap` is that
    /// decision arriving.
    pub fn pairing_for(&self, deck: &OnDeck, swap: bool) -> Result<Pairing, Refused> {
        let class = self.class(&deck.class)?;
        let lanes = if swap {
            [Lane::L2, Lane::L1]
        } else {
            [Lane::L1, Lane::L2]
        };
        let entries: Vec<RaceEntry> = deck
            .entries
            .iter()
            .zip(lanes)
            .map(|((_, id), lane)| RaceEntry {
                lane,
                dial_s: self.sheet.entry(*id).and_then(|e| e.dial_s),
            })
            .collect();
        Pairing::new(class.format, entries).map_err(Refused::Pairing)
    }

    pub fn driver(&self, id: EntryId) -> String {
        self.sheet
            .entry(id)
            .map(|e| format!("#{} {}", e.number, e.driver))
            .unwrap_or_else(|| format!("#{}", id.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r#"
[event]
name = "Club day"
date = "2026-08-15"

[[class]]
name = "Bracket"
format = "bracket"
seeding = "closest-to-dial"
ladder = "pro"

[[entry]]
number = 1
driver = "A"
class = "Bracket"
dial_s = 12.34
[[entry]]
number = 2
driver = "B"
class = "Bracket"
dial_s = 7.50
[[entry]]
number = 3
driver = "C"
class = "Bracket"
dial_s = 9.90
[[entry]]
number = 4
driver = "D"
class = "Bracket"
dial_s = 10.50
"#;

    fn sheet() -> Sheet {
        Sheet::parse(SHEET).unwrap()
    }

    /// Run a whole class, keeping the log as it is written.
    fn run_a_class(better_always_wins: bool) -> (Progress, Vec<String>) {
        let mut p = Progress::new(sheet());
        let mut log = Vec::new();
        let push = |log: &mut Vec<String>, r: Record| log.push(r.line());

        for (n, off) in [(1u32, 0.01), (2, 0.40), (3, 0.02), (4, 0.30)] {
            let dial = p.sheet().entry(EntryId(n)).unwrap().dial_s.unwrap();
            push(
                &mut log,
                p.qualified("Bracket", EntryId(n), Some(dial + off), Some(dial), false)
                    .unwrap(),
            );
        }
        push(&mut log, p.draw("Bracket").unwrap());

        while p.champion("Bracket").is_none() {
            let deck = p.next_pair_in("Bracket").expect("a pair to run");
            let winner = match deck.right {
                Some(r) if better_always_wins => deck.left.min(r),
                Some(r) => deck.left.max(r),
                None => deck.left,
            };
            let r = if deck.is_bye() {
                p.bye("Bracket", deck.position, true).unwrap()
            } else {
                p.won("Bracket", deck.position, winner).unwrap()
            };
            push(&mut log, r);
        }
        (p, log)
    }

    #[test]
    fn a_class_runs_from_qualifying_to_a_champion() {
        let (p, _) = run_a_class(true);
        // Entry 1 was 0.01 off its dial and entry 3 was 0.02 — closest to dial
        // is what a bracket measures, so 1 is the top qualifier and wins it.
        let field = p.field("Bracket").unwrap();
        assert_eq!(field.entry(1), Some(EntryId(1)));
        assert_eq!(field.entry(2), Some(EntryId(3)));
        assert_eq!(p.champion("Bracket"), Some(1));
    }

    #[test]
    fn the_day_rebuilds_from_its_log_exactly() {
        // The property the whole module exists for, and the same one D26 claims
        // about a bus session: state that is derived can be checked.
        let (live, log) = run_a_class(false);
        let (replayed, skipped) = Progress::replay(sheet(), &log.join("\n"));
        assert_eq!(skipped, 0);
        assert_eq!(replayed.champion("Bracket"), live.champion("Bracket"));
        assert_eq!(
            replayed
                .field("Bracket")
                .unwrap()
                .seeds()
                .collect::<Vec<_>>(),
            live.field("Bracket").unwrap().seeds().collect::<Vec<_>>()
        );
        assert_eq!(
            replayed.round_number("Bracket"),
            live.round_number("Bracket")
        );
    }

    #[test]
    fn a_power_cut_mid_round_resumes_where_it_was() {
        // The failure mode this design is chosen for. The last line written is
        // about a round that finished, never one in progress.
        let (_, log) = run_a_class(true);
        let cut = &log[..log.len() - 1];
        let (p, _) = Progress::replay(sheet(), &cut.join("\n"));
        assert_eq!(
            p.champion("Bracket"),
            None,
            "the final has not been recorded"
        );
        let deck = p.next_pair_in("Bracket").expect("and it is still on deck");
        assert_eq!(deck.round, 2);
    }

    #[test]
    fn a_torn_last_line_costs_one_record_and_says_so() {
        // A truncated write after a power cut is normal; a hundred of them is a
        // corrupted file somebody has to look at, and the count is the
        // difference between the two.
        let (_, log) = run_a_class(true);
        let mut text = log.join("\n");
        text.push_str("\nW Bracket 9");
        let (p, skipped) = Progress::replay(sheet(), &text);
        assert_eq!(skipped, 1);
        assert_eq!(p.champion("Bracket"), Some(1), "the rest still stands");
    }

    #[test]
    fn a_winner_who_was_not_in_the_pair_is_refused_before_it_is_written() {
        let mut p = Progress::new(sheet());
        p.draw("Bracket").unwrap();
        let deck = p.next_pair_in("Bracket").unwrap();
        let absent = (1..=4)
            .find(|s| !deck.entries.iter().any(|(x, _)| x == s))
            .unwrap();
        assert_eq!(
            p.won("Bracket", deck.position, absent),
            Err(Refused::NotInThisPair {
                position: deck.position,
                seed: absent
            })
        );
    }

    #[test]
    fn qualifying_closes_when_the_ladder_is_drawn() {
        // A late slip must not reshuffle a ladder people have been told about.
        let mut p = Progress::new(sheet());
        p.draw("Bracket").unwrap();
        assert_eq!(
            p.qualified("Bracket", EntryId(1), Some(9.0), Some(12.34), false),
            Err(Refused::AlreadyDrawn("Bracket".into()))
        );
        assert_eq!(
            p.draw("Bracket"),
            Err(Refused::AlreadyDrawn("Bracket".into()))
        );
    }

    #[test]
    fn an_on_deck_pair_becomes_the_pairing_the_race_logic_wants() {
        // Where the tournament meets the timing: dials off the entry sheet, and
        // a handicap that follows from them.
        let mut p = Progress::new(sheet());
        p.draw("Bracket").unwrap();
        let deck = p.next_pair_in("Bracket").unwrap();
        let pairing = p.pairing_for(&deck, false).unwrap();
        assert_eq!(pairing.entries().len(), 2);

        // Entry order seeds 1,2,3,4; the pro ladder pairs 1 v 4, so the dials
        // are 12.34 against 10.50 and the quicker car waits 1.84 s.
        let spot = pairing.handicap_ms().unwrap();
        assert_eq!(spot, [0, 1840]);

        // Lane choice is a right, and exercising it swaps who is where.
        let swapped = p.pairing_for(&deck, true).unwrap();
        assert_eq!(swapped.handicap_ms().unwrap(), [1840, 0]);
    }

    #[test]
    fn a_class_with_no_ladder_drawn_has_nothing_on_deck() {
        let p = Progress::new(sheet());
        assert!(p.next_pair().is_none());
        assert!(!p.is_drawn("Bracket"));
    }
}
