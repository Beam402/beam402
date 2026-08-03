//! The entry sheet, as a file — and the day's results, as a log.
//!
//! **D23** promises a club changes a class rule without seeing a compiler, so
//! the classes and the entries are a TOML file beside the mapping file, versioned
//! in the club's own repository the same way.
//!
//! ## Why the results are an append-only log
//!
//! Everything else about the day is **derived** from that log: who qualified
//! where, which round each class is on, who is still in it. Nothing is stored as
//! state, and that is the same argument **D26** makes about a bus session — a
//! ladder rebuilt from the results that produced it can be checked, and one held
//! in a file that is rewritten as it goes can only be trusted.
//!
//! It also decides the failure mode. Race control loses power in the middle of
//! an eliminator; on restart it reads the log and is exactly where it was,
//! because the last thing it wrote was a line about a round that finished rather
//! than a snapshot of a round in progress.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use beam402_race::Format;
use serde::Deserialize;

use crate::ladder::Style;
use crate::{Class, Entry, EntryId, LaneChoice, Seed, Seeding};

/// The entry sheet: what is being run and by whom.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    pub event: Meeting,
    #[serde(rename = "class")]
    pub classes: Vec<ClassSheet>,
    #[serde(rename = "entry")]
    pub entries: Vec<EntrySheet>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Meeting {
    /// The event's identity when it is carried to a server (**D33**) — a slug the
    /// club chooses, because it ends up in a URL that people share and
    /// `kaluga-2026-08-15` is worth more there than 32 hex digits. Optional: a
    /// day that never leaves the tower needs no name outside it.
    pub id: Option<String>,
    pub name: String,
    pub date: String,
    /// The league's own key for this event, carried and **never interpreted**
    /// (**D35**). A facade is somebody else's program joining its data to these
    /// results, and it needs somewhere to put the identifier it already uses.
    /// Nothing here reads it, compares it, or requires a shape of it.
    #[serde(rename = "ref")]
    pub external: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassSheet {
    pub name: String,
    /// `heads-up`, `bracket`, or `index`.
    pub format: String,
    /// Required by `index`, refused by the others — the index is the class's
    /// number, and a class that has one and does not use it is a mistake worth
    /// hearing about at load time.
    pub index_s: Option<f64>,
    /// `quickest-et`, `closest-to-dial`, `entry-order`, `draw`.
    #[serde(default = "default_seeding")]
    pub seeding: String,
    /// Required by `draw`, so the same event replays to the same ladder.
    pub draw_seed: Option<u64>,
    /// `pro` or `sportsman`.
    #[serde(default = "default_ladder")]
    pub ladder: String,
    /// `better-qualifier` or `previous-round`.
    #[serde(default = "default_lane_choice")]
    pub lane_choice: String,
    #[serde(default)]
    pub deep_staging: bool,

    /// The field sizes this class runs, as a rulebook lists them.
    ///
    /// **The largest one the entry list fills is the field**, and if it fills none
    /// of them everybody qualifies. One setting, because rulebooks differ in what
    /// they say rather than in how they compute it:
    ///
    /// - `[8, 16]` — "top 8 from four entries, top 16 from sixteen".
    /// - `[16]` — "a sixteen-car field", with byes when fewer turn up.
    /// - `[2, 4, 8, 16, 32]` — "the largest bracket that fills, no byes".
    /// - unset — everybody runs, which is a club day and a practice day.
    ///
    /// A cut and a bye are the same list read from opposite ends, so this is the
    /// one place either is decided.
    #[serde(default)]
    pub field: Vec<usize>,
    /// Below this the class does not run and `draw` says so. "A minimum of four."
    #[serde(default)]
    pub min_entries: usize,
    /// How many passes **score**. Rulebooks say *scoring* attempts, and mean it:
    /// a fourth pass is not forbidden, it simply does not count. So a pass beyond
    /// this is recorded like any other and left out of the seeding.
    pub attempts: Option<usize>,
}

fn default_seeding() -> String {
    "quickest-et".into()
}
fn default_ladder() -> String {
    "pro".into()
}
fn default_lane_choice() -> String {
    "better-qualifier".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrySheet {
    pub number: u32,
    pub driver: String,
    #[serde(default)]
    pub car: String,
    pub class: String,
    /// The league's key for this competitor — a licence number, a row id, a UUID.
    /// Carried through to the API and never interpreted (**D35**), which is what
    /// lets a facade show a racer's history across a season without this project
    /// owning a database of people.
    #[serde(rename = "ref")]
    pub external: Option<String>,
    /// Required by a bracket class. There is no sane default: guessing one hands
    /// somebody a head start they did not earn.
    pub dial_s: Option<f64>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SheetError {
    UnknownFormat(String),
    UnknownSeeding(String),
    UnknownLadder(String),
    UnknownLaneChoice(String),
    /// An entry names a class the sheet does not declare.
    NoSuchClass {
        entry: u32,
        class: String,
    },
    /// Two entries with the same car number. A ladder that cannot tell them
    /// apart is a ladder that pairs the wrong people.
    DuplicateNumber(u32),
    MissingIndex(String),
    IndexOnANonIndexClass(String),
    MissingDrawSeed(String),
    MissingDial {
        entry: u32,
        class: String,
    },
    /// An `[event] id` that cannot be a URL. Caught here rather than at upload
    /// time, which is **D34**'s whole argument: wrong in the morning is an
    /// inconvenience, wrong at the end of the day is a day nobody can publish.
    BadId(String),
    NoClasses,
    /// A class setting that is a typo rather than a rulebook: it would produce an
    /// empty field and blame the entries.
    NotARule {
        class: String,
        what: &'static str,
    },
}

impl core::fmt::Display for SheetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SheetError::UnknownFormat(s) => {
                write!(f, "unknown format {s:?} — heads-up, bracket or index")
            }
            SheetError::UnknownSeeding(s) => write!(
                f,
                "unknown seeding {s:?} — quickest-et, closest-to-dial, entry-order or draw"
            ),
            SheetError::UnknownLadder(s) => {
                write!(f, "unknown ladder {s:?} — pro or sportsman")
            }
            SheetError::UnknownLaneChoice(s) => {
                write!(
                    f,
                    "unknown lane choice {s:?} — better-qualifier or previous-round"
                )
            }
            SheetError::NoSuchClass { entry, class } => {
                write!(
                    f,
                    "entry {entry} runs in {class:?}, which this sheet has no class for"
                )
            }
            SheetError::DuplicateNumber(n) => write!(f, "two entries are numbered {n}"),
            SheetError::MissingIndex(c) => {
                write!(f, "class {c:?} is an index class with no index_s")
            }
            SheetError::IndexOnANonIndexClass(c) => {
                write!(f, "class {c:?} has an index_s and is not an index class")
            }
            SheetError::MissingDrawSeed(c) => write!(
                f,
                "class {c:?} draws its ladder and has no draw_seed — the pairing could not be \
                 re-derived later"
            ),
            SheetError::MissingDial { entry, class } => {
                write!(
                    f,
                    "entry {entry} is in bracket class {class:?} with no dial-in"
                )
            }
            SheetError::BadId(id) => write!(
                f,
                "event id {id:?} cannot go in a URL — lower-case letters, digits, - and _, \
                 up to 64 of them"
            ),
            SheetError::NoClasses => write!(f, "an event with no classes"),
            SheetError::NotARule { class, what } => write!(
                f,
                "class {class:?}: {what} is not a rule anybody has — it would draw \
                 an empty ladder and not say why"
            ),
        }
    }
}

impl Sheet {
    pub fn parse(text: &str) -> Result<Sheet, String> {
        let sheet: Sheet = toml::from_str(text).map_err(|e| e.to_string())?;
        sheet.check().map_err(|e| e.to_string())?;
        Ok(sheet)
    }

    /// Everything that can be wrong before a car turns a wheel.
    ///
    /// Checked at load rather than discovered at the top end: an entry sheet that
    /// is wrong on the morning is an inconvenience, and one that is wrong at the
    /// semi-final is a protest.
    pub fn check(&self) -> Result<(), SheetError> {
        if self.classes.is_empty() {
            return Err(SheetError::NoClasses);
        }
        if let Some(id) = &self.event.id {
            let ok = !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if !ok {
                return Err(SheetError::BadId(id.clone()));
            }
        }
        let mut seen = BTreeMap::new();
        for c in &self.classes {
            let class = c.to_class()?;
            if matches!(class.format, Format::Index { .. }) && c.index_s.is_none() {
                return Err(SheetError::MissingIndex(c.name.clone()));
            }
            if !matches!(class.format, Format::Index { .. }) && c.index_s.is_some() {
                return Err(SheetError::IndexOnANonIndexClass(c.name.clone()));
            }
            if matches!(class.seeding, Seeding::Draw { .. }) && c.draw_seed.is_none() {
                return Err(SheetError::MissingDrawSeed(c.name.clone()));
            }
            seen.insert(c.name.clone(), class);
        }

        let mut numbers = Vec::new();
        for e in &self.entries {
            if numbers.contains(&e.number) {
                return Err(SheetError::DuplicateNumber(e.number));
            }
            numbers.push(e.number);
            let Some(class) = seen.get(&e.class) else {
                return Err(SheetError::NoSuchClass {
                    entry: e.number,
                    class: e.class.clone(),
                });
            };
            if class.format == Format::Bracket && e.dial_s.is_none() {
                return Err(SheetError::MissingDial {
                    entry: e.number,
                    class: e.class.clone(),
                });
            }
        }
        Ok(())
    }

    pub fn class(&self, name: &str) -> Option<Class> {
        self.classes
            .iter()
            .find(|c| c.name == name)
            .and_then(|c| c.to_class().ok())
    }

    /// Entries in one class, as the ladder layer wants them.
    pub fn entries_in(&self, class: &str) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.class == class)
            .map(|e| Entry {
                id: EntryId(e.number),
                driver: e.driver.clone(),
                car: e.car.clone(),
                dial_s: e.dial_s,
            })
            .collect()
    }

    pub fn entry(&self, id: EntryId) -> Option<&EntrySheet> {
        self.entries.iter().find(|e| e.number == id.0)
    }
}

impl ClassSheet {
    pub fn to_class(&self) -> Result<Class, SheetError> {
        let format = match self.format.as_str() {
            "heads-up" => Format::HeadsUp,
            "bracket" => Format::Bracket,
            "index" => Format::Index {
                seconds: self.index_s.unwrap_or(0.0),
            },
            other => return Err(SheetError::UnknownFormat(other.into())),
        };
        let seeding = match self.seeding.as_str() {
            "quickest-et" => Seeding::QuickestEt,
            "closest-to-dial" => Seeding::ClosestToDial,
            "entry-order" => Seeding::EntryOrder,
            "draw" => Seeding::Draw {
                seed: self.draw_seed.unwrap_or(0),
            },
            other => return Err(SheetError::UnknownSeeding(other.into())),
        };
        let ladder = match self.ladder.as_str() {
            "pro" => Style::Pro,
            "sportsman" => Style::Sportsman,
            other => return Err(SheetError::UnknownLadder(other.into())),
        };
        let lane_choice = match self.lane_choice.as_str() {
            "better-qualifier" => LaneChoice::BetterQualifier,
            "previous-round" => LaneChoice::PreviousRound,
            other => return Err(SheetError::UnknownLaneChoice(other.into())),
        };
        // A field of nobody and a scoring limit of no passes are not rules, they
        // are typos — and both would produce an empty ladder without saying why.
        if self.field.contains(&0) {
            return Err(SheetError::NotARule {
                class: self.name.clone(),
                what: "a field size of 0",
            });
        }
        if self.attempts == Some(0) {
            return Err(SheetError::NotARule {
                class: self.name.clone(),
                what: "attempts = 0",
            });
        }
        Ok(Class {
            name: self.name.clone(),
            format,
            seeding,
            ladder,
            lane_choice,
            deep_staging: self.deep_staging,
            field: {
                let mut f = self.field.clone();
                f.sort_unstable();
                f
            },
            min_entries: self.min_entries,
            attempts: self.attempts,
        })
    }
}

// ---------------------------------------------------------------------------
// The day, as a log
// ---------------------------------------------------------------------------

/// One thing that happened, appended as it happened.
#[derive(Clone, PartialEq, Debug)]
pub enum Record {
    /// A qualifying attempt.
    Qualified {
        class: String,
        entry: EntryId,
        et_s: Option<f64>,
        dial_s: Option<f64>,
        red: bool,
    },
    /// Qualifying is over for this class and the ladder is drawn. Written so the
    /// field is fixed at a moment rather than recomputed from attempts that
    /// might arrive late.
    Drawn { class: String, order: Vec<EntryId> },
    /// A round's pair produced a winner.
    Won {
        class: String,
        round: usize,
        position: usize,
        seed: Seed,
    },
    /// A bye that was run, or not.
    Bye {
        class: String,
        round: usize,
        position: usize,
        completed: bool,
    },
}

impl Record {
    /// Which class this is about.
    pub fn class(&self) -> &str {
        match self {
            Record::Qualified { class, .. }
            | Record::Drawn { class, .. }
            | Record::Won { class, .. }
            | Record::Bye { class, .. } => class,
        }
    }

    /// Which entries this names. Used by **D33** to decide whether a replaced
    /// entry sheet still fits a log that has already been written: an edit that
    /// would orphan a recorded result is refused rather than applied.
    pub fn entries(&self) -> Vec<EntryId> {
        match self {
            Record::Qualified { entry, .. } => vec![*entry],
            Record::Drawn { order, .. } => order.clone(),
            Record::Won { .. } | Record::Bye { .. } => Vec::new(),
        }
    }

    /// One line, readable by a person. The same argument as the session log:
    /// this is what somebody looks at when they disagree about a ladder.
    pub fn line(&self) -> String {
        let mut s = String::new();
        match self {
            Record::Qualified {
                class,
                entry,
                et_s,
                dial_s,
                red,
            } => {
                let _ = write!(
                    s,
                    "Q {} {} {} {} {}",
                    esc(class),
                    entry.0,
                    opt(*et_s),
                    opt(*dial_s),
                    if *red { "red" } else { "-" }
                );
            }
            Record::Drawn { class, order } => {
                let _ = write!(s, "D {}", esc(class));
                for e in order {
                    let _ = write!(s, " {}", e.0);
                }
            }
            Record::Won {
                class,
                round,
                position,
                seed,
            } => {
                let _ = write!(s, "W {} {round} {position} {seed}", esc(class));
            }
            Record::Bye {
                class,
                round,
                position,
                completed,
            } => {
                let _ = write!(
                    s,
                    "B {} {round} {position} {}",
                    esc(class),
                    if *completed { "run" } else { "not-run" }
                );
            }
        }
        s
    }

    pub fn parse(line: &str) -> Option<Record> {
        let mut f = line.split_whitespace();
        let tag = f.next()?;
        let class = unesc(f.next()?);
        match tag {
            "Q" => Some(Record::Qualified {
                class,
                entry: EntryId(f.next()?.parse().ok()?),
                et_s: num(f.next()?),
                dial_s: num(f.next()?),
                red: f.next()? == "red",
            }),
            "D" => Some(Record::Drawn {
                class,
                order: f.filter_map(|v| v.parse().ok()).map(EntryId).collect(),
            }),
            "W" => Some(Record::Won {
                class,
                round: f.next()?.parse().ok()?,
                position: f.next()?.parse().ok()?,
                seed: f.next()?.parse().ok()?,
            }),
            "B" => Some(Record::Bye {
                class,
                round: f.next()?.parse().ok()?,
                position: f.next()?.parse().ok()?,
                completed: f.next()? == "run",
            }),
            _ => None,
        }
    }
}

/// A class name with no spaces in it, so a line stays one line.
fn esc(s: &str) -> String {
    s.replace(' ', "_")
}
fn unesc(s: &str) -> String {
    s.replace('_', " ")
}
fn opt(v: Option<f64>) -> String {
    match v {
        Some(v) => format!("{v:.4}"),
        None => "-".into(),
    }
}
fn num(s: &str) -> Option<f64> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHEET: &str = r#"
[event]
name = "Club day"
date = "2026-08-15"

[[class]]
name = "Street bracket"
format = "bracket"
seeding = "closest-to-dial"
ladder = "sportsman"

[[class]]
name = "Super Gas"
format = "index"
index_s = 9.90

[[entry]]
number = 7
driver = "A"
car = "Nova"
class = "Street bracket"
dial_s = 12.34

[[entry]]
number = 11
driver = "B"
class = "Street bracket"
dial_s = 7.50

[[entry]]
number = 3
driver = "C"
class = "Super Gas"
"#;

    #[test]
    fn a_sheet_parses_into_classes_and_entries() {
        let s = Sheet::parse(SHEET).unwrap();
        assert_eq!(s.event.name, "Club day");
        assert_eq!(s.classes.len(), 2);
        assert_eq!(s.entries_in("Street bracket").len(), 2);
        let c = s.class("Street bracket").unwrap();
        assert_eq!(c.format, Format::Bracket);
        assert_eq!(c.seeding, Seeding::ClosestToDial);
        assert_eq!(c.ladder, Style::Sportsman);
        // A defaulted field is still a decision, and the default is the common one.
        assert_eq!(c.lane_choice, LaneChoice::BetterQualifier);
    }

    #[test]
    fn a_bracket_entry_without_a_dial_is_a_load_error() {
        // The same rule the pairing enforces, moved to the morning: guessing a
        // dial hands somebody a head start, and the error would show up as a
        // lost race rather than as a broken sheet.
        let bad = SHEET.replace("dial_s = 12.34", "");
        assert_eq!(
            Sheet::parse(&bad).unwrap_err(),
            SheetError::MissingDial {
                entry: 7,
                class: "Street bracket".into()
            }
            .to_string()
        );
    }

    #[test]
    fn an_entry_in_a_class_that_does_not_exist_is_refused() {
        let bad = SHEET.replace(r#"class = "Super Gas""#, r#"class = "Super Comp""#);
        assert!(Sheet::parse(&bad).unwrap_err().contains("Super Comp"));
    }

    #[test]
    fn two_cars_with_the_same_number_are_refused() {
        // A ladder that cannot tell two entries apart pairs the wrong people.
        let bad = SHEET.replace("number = 11", "number = 7");
        assert!(Sheet::parse(&bad).unwrap_err().contains("numbered 7"));
    }

    #[test]
    fn an_index_class_needs_its_index_and_the_others_must_not_have_one() {
        let bad = SHEET.replace("index_s = 9.90", "");
        assert!(Sheet::parse(&bad).unwrap_err().contains("no index_s"));

        let bad = SHEET.replace(
            "format = \"bracket\"\nseeding",
            "format = \"bracket\"\nindex_s = 9.9\nseeding",
        );
        assert!(Sheet::parse(&bad)
            .unwrap_err()
            .contains("not an index class"));
    }

    #[test]
    fn a_drawn_ladder_without_a_seed_is_refused() {
        // Without it the pairing cannot be re-derived a season later, which is
        // the same argument D26 makes about a disputed ET.
        let bad = SHEET.replace(r#"seeding = "closest-to-dial""#, r#"seeding = "draw""#);
        assert!(Sheet::parse(&bad).unwrap_err().contains("re-derived"));
    }

    #[test]
    fn an_unknown_key_is_a_load_error_rather_than_a_shrug() {
        let bad = SHEET.replace("driver = \"A\"", "driver = \"A\"\nnickname = \"Ace\"");
        assert!(Sheet::parse(&bad).is_err());
    }

    #[test]
    fn every_record_survives_the_round_trip_through_a_line() {
        // The day is rebuilt from these, so a line that does not read back is a
        // ladder that cannot be reconstructed.
        let records = vec![
            Record::Qualified {
                class: "Street bracket".into(),
                entry: EntryId(7),
                et_s: Some(12.3456),
                dial_s: Some(12.34),
                red: true,
            },
            Record::Qualified {
                class: "A".into(),
                entry: EntryId(1),
                et_s: None,
                dial_s: None,
                red: false,
            },
            Record::Drawn {
                class: "Street bracket".into(),
                order: vec![EntryId(11), EntryId(7)],
            },
            Record::Won {
                class: "Street bracket".into(),
                round: 2,
                position: 1,
                seed: 3,
            },
            Record::Bye {
                class: "Street bracket".into(),
                round: 1,
                position: 0,
                completed: true,
            },
        ];
        for r in records {
            let line = r.line();
            assert!(!line.contains('\n'), "one record is one line: {line:?}");
            assert_eq!(Record::parse(&line), Some(r), "{line}");
        }
    }

    #[test]
    fn a_line_that_is_not_a_record_is_none_rather_than_a_guess() {
        for line in ["", "X a 1", "W", "Q class", "W class notanumber 0 1"] {
            assert_eq!(Record::parse(line), None, "{line:?}");
        }
    }
}
