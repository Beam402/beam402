//! The registration desk (**D34**).
//!
//! Beam402 does not own registration — every league does it differently and
//! several already do it in a spreadsheet they like. The **entry sheet** is the
//! interface, and this is the reference path onto it: a CSV of entries, a
//! skeleton the club keeps across a season for the classes, and a `sheet.toml`
//! out the other end.
//!
//! ## Errors carry a row number
//!
//! The point of validating here rather than at the top end is that the person who
//! can fix it is still standing in front of the desk. "Row 14: #22 is in bracket
//! class \"Street\" with no dial-in" is a thing they can act on; the same fact
//! discovered in the semi-final is a protest.
//!
//! ## What it deliberately cannot do
//!
//! Declare a class. A class is a rulebook — format, seeding, ladder, lane choice
//! — and it belongs in a file the club versions, not in a column of a spreadsheet
//! that a volunteer retypes every Saturday.

use std::fmt::Write as _;

use crate::sheet::{EntrySheet, Sheet};
use crate::Sheet as _Sheet;

/// Columns the importer understands. Extra columns are ignored, because a club's
/// registration spreadsheet has paid/tech/phone in it and none of that is ours.
const REQUIRED: [&str; 3] = ["number", "driver", "class"];

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeskError {
    Empty,
    MissingColumn(&'static str),
    /// More fields than the header declared. Almost always an unquoted
    /// delimiter inside a value, which is worth naming rather than silently
    /// shifting every column after it.
    Width {
        row: usize,
        got: usize,
        want: usize,
    },
    /// A required column is empty on this row.
    Missing {
        row: usize,
        field: &'static str,
    },
    BadNumber {
        row: usize,
        field: String,
        value: String,
    },
    /// The result does not load as an entry sheet. Carried through verbatim so
    /// the sheet's own checks are the ones reported.
    Rejected(String),
}

impl core::fmt::Display for DeskError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DeskError::Empty => write!(f, "no rows"),
            DeskError::MissingColumn(c) => write!(
                f,
                "no {c:?} column — the header must name at least number, driver and class"
            ),
            DeskError::Width { row, got, want } => write!(
                f,
                "row {row}: {got} fields where the header declared {want} — an \
                 unquoted separator inside a value?"
            ),
            DeskError::Missing { row, field } => write!(f, "row {row}: no {field}"),
            DeskError::BadNumber { row, field, value } => {
                write!(f, "row {row}: {field} {value:?} is not a number")
            }
            DeskError::Rejected(why) => write!(f, "{why}"),
        }
    }
}

/// One CSV row, before it is anything.
struct Row {
    line: usize,
    fields: Vec<String>,
}

/// Which character separates fields.
///
/// **Taken from the header, not accepted as either.** A spreadsheet set to a
/// Russian locale exports `;` and writes decimals as `13,50`, so a reader that
/// treated both characters as separators would split that dial in half — which is
/// exactly what it did before this function existed. One delimiter per file, and
/// the header is the only row whose shape is known in advance.
fn delimiter(header: &str) -> char {
    if header.contains(';') {
        ';'
    } else if header.contains('\t') {
        '\t'
    } else {
        ','
    }
}

/// Read a CSV. Quoted fields with an embedded delimiter are handled because a
/// driver name is exactly where one appears; nothing more elaborate, because a
/// registration list that needs a full CSV dialect is a list that should be
/// exported differently.
fn rows(text: &str) -> Vec<Row> {
    let mut out = Vec::new();
    let head = text
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .unwrap_or_default();
    let sep = delimiter(head);
    for (i, line) in text.lines().enumerate() {
        let line_no = i + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut fields = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                '"' => quoted = !quoted,
                c if c == sep && !quoted => fields.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            }
        }
        fields.push(cur);
        out.push(Row {
            line: line_no,
            fields: fields.iter().map(|f| f.trim().to_string()).collect(),
        });
    }
    out
}

/// Entries from a CSV, checked against the classes a skeleton declares.
///
/// `skeleton` is a sheet with `[event]` and `[[class]]` and no entries — what a
/// club maintains for a season. The output is that skeleton with the day's
/// entries written into it.
pub fn import(skeleton: &str, csv: &str) -> Result<String, DeskError> {
    let rows = rows(csv);
    let (header, body) = rows.split_first().ok_or(DeskError::Empty)?;
    if body.is_empty() {
        return Err(DeskError::Empty);
    }

    let index = |name: &'static str| -> Result<usize, DeskError> {
        header
            .fields
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
            .ok_or(DeskError::MissingColumn(name))
    };
    for c in REQUIRED {
        index(c)?;
    }
    let (i_num, i_driver, i_class) = (index("number")?, index("driver")?, index("class")?);
    let i_car = index("car").ok();
    let i_dial = index("dial_s").ok().or_else(|| index("dial").ok());
    let want = header.fields.len();

    let mut entries = Vec::new();
    for row in body {
        if row.fields.len() > want {
            return Err(DeskError::Width {
                row: row.line,
                got: row.fields.len(),
                want,
            });
        }
        // Short rows are padded rather than refused: a trailing empty cell is how
        // every spreadsheet writes "no dial", and refusing it would refuse every
        // index class. A required column that lands empty is caught below.
        let at = |i: usize| row.fields.get(i).cloned().unwrap_or_default();
        for (i, name) in [(i_num, "number"), (i_driver, "driver"), (i_class, "class")] {
            if at(i).is_empty() {
                return Err(DeskError::Missing {
                    row: row.line,
                    field: name,
                });
            }
        }
        let number = at(i_num).parse::<u32>().map_err(|_| DeskError::BadNumber {
            row: row.line,
            field: "number".into(),
            value: at(i_num),
        })?;
        // An empty dial cell is a missing dial, not a zero. Whether that is an
        // error depends on the class, and the sheet's own checks decide it.
        let dial_s = match i_dial.map(at).filter(|s| !s.is_empty()) {
            Some(v) => {
                Some(
                    v.replace(',', ".")
                        .parse::<f64>()
                        .map_err(|_| DeskError::BadNumber {
                            row: row.line,
                            field: "dial".into(),
                            value: v.clone(),
                        })?,
                )
            }
            None => None,
        };
        entries.push(EntrySheet {
            number,
            driver: at(i_driver),
            car: i_car.map(at).unwrap_or_default(),
            class: at(i_class),
            dial_s,
        });
    }

    let text = write(skeleton, &entries);
    // Loaded rather than trusted: the file this produces has to pass exactly the
    // checks race control will apply to it, or the desk has signed off on
    // something that fails at the tower.
    _Sheet::parse(&text).map_err(DeskError::Rejected)?;
    Ok(text)
}

/// Render a skeleton plus entries as an entry sheet.
///
/// The skeleton's own text is kept verbatim — comments and all — because it is a
/// file the club wrote and reads. Only the entries are generated.
fn write(skeleton: &str, entries: &[EntrySheet]) -> String {
    let mut out = skeleton.trim_end().to_string();
    out.push_str("\n\n# Entries, generated by `beam402 sheet` from the day's\n");
    out.push_str("# registration list. Edit the list, not this section.\n");
    for e in entries {
        let _ = write!(out, "\n[[entry]]\nnumber = {}\n", e.number);
        let _ = writeln!(out, "driver = {}", quote(&e.driver));
        if !e.car.is_empty() {
            let _ = writeln!(out, "car = {}", quote(&e.car));
        }
        let _ = writeln!(out, "class = {}", quote(&e.class));
        if let Some(d) = e.dial_s {
            let _ = writeln!(out, "dial_s = {d:.2}");
        }
    }
    out
}

/// TOML basic string. Enough for a name and a car: anything a driver's name can
/// contain that this would get wrong is also something a CSV would get wrong.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The entry list as a person reads it, for checking at the desk by somebody who
/// does not read TOML.
pub fn entry_list(sheet: &Sheet) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} — {}{}\n",
        sheet.event.name,
        sheet.event.date,
        match &sheet.event.id {
            Some(id) => format!("  [{id}]"),
            None => String::new(),
        }
    );
    for class in &sheet.classes {
        let mut in_class: Vec<&EntrySheet> = sheet
            .entries
            .iter()
            .filter(|e| e.class == class.name)
            .collect();
        in_class.sort_by_key(|e| e.number);
        let _ = writeln!(
            out,
            "{}  —  {}{}, {} ladder, {} entered",
            class.name,
            class.format,
            match class.index_s {
                Some(i) => format!(" {i:.2}"),
                None => String::new(),
            },
            class.ladder,
            in_class.len()
        );
        for e in &in_class {
            let _ = writeln!(
                out,
                "  {:>4}  {:<24}{:<20}{}",
                format!("#{}", e.number),
                e.driver,
                e.car,
                match e.dial_s {
                    Some(d) => format!("dial {d:.2}"),
                    None => String::new(),
                }
            );
        }
        let _ = writeln!(out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKELETON: &str = r#"# Kept across a season: the classes are a rulebook.
[event]
id = "club-day"
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
ladder = "pro"
"#;

    #[test]
    fn a_registration_spreadsheet_becomes_an_entry_sheet() {
        // Semicolons and decimal commas: what a Russian-locale spreadsheet
        // actually exports, and the case a reader that accepted both separators
        // split down the middle.
        let csv = "number;driver;car;class;dial_s\n\
                   7;A. Ivanov;VAZ 2108;Street bracket;12.35\n\
                   22;\"Sidorov; V.\";Lada;Street bracket;13,50\n\
                   9;D. Kuznetsov;Camaro;Super Gas;\n";
        let text = import(SKELETON, csv).unwrap();
        let sheet = Sheet::parse(&text).unwrap();
        assert_eq!(sheet.entries.len(), 3);
        assert_eq!(sheet.entries[1].driver, "Sidorov; V.");
        assert_eq!(sheet.entries[1].dial_s, Some(13.50));
        assert_eq!(
            sheet.entries[2].dial_s, None,
            "an index class needs no dial"
        );
        assert!(
            text.contains("# Kept across a season"),
            "the skeleton survives"
        );
        assert_eq!(sheet.event.id.as_deref(), Some("club-day"));
    }

    #[test]
    fn a_bracket_entry_with_no_dial_is_refused_at_the_desk() {
        // The whole reason to validate here: the person who can fix it is still
        // standing there, and the alternative is finding out in a semi-final.
        let csv = "number,driver,class,dial_s\n7,A. Ivanov,Street bracket,\n";
        let err = import(SKELETON, csv).unwrap_err();
        assert!(
            matches!(err, DeskError::Rejected(ref why) if why.contains("no dial-in")),
            "{err}"
        );
    }

    #[test]
    fn a_class_the_skeleton_does_not_declare_is_refused() {
        let csv = "number,driver,class\n7,A. Ivanov,Pro Mod\n";
        let err = import(SKELETON, csv).unwrap_err();
        assert!(
            matches!(err, DeskError::Rejected(ref w) if w.contains("Pro Mod")),
            "{err}"
        );
    }

    #[test]
    fn two_cars_with_one_number_are_refused() {
        // A ladder that cannot tell them apart pairs the wrong people.
        let csv = "number,driver,class\n9,A,Super Gas\n9,B,Super Gas\n";
        let err = import(SKELETON, csv).unwrap_err();
        assert!(
            matches!(err, DeskError::Rejected(ref w) if w.contains("numbered 9")),
            "{err}"
        );
    }

    #[test]
    fn a_bad_row_names_its_line_number() {
        let csv = "number,driver,class\n7,A,Super Gas\nnine,B,Super Gas\n";
        assert_eq!(
            import(SKELETON, csv),
            Err(DeskError::BadNumber {
                row: 3,
                field: "number".into(),
                value: "nine".into()
            })
        );

        assert_eq!(
            import(SKELETON, "number,driver,class\n7,A\n"),
            Err(DeskError::Missing {
                row: 2,
                field: "class"
            })
        );
        // The case a comma-delimited file with a decimal comma produces, named
        // rather than silently shifting every column after it.
        assert_eq!(
            import(
                SKELETON,
                "number,driver,class,dial_s\n7,A,Street bracket,12,35\n"
            ),
            Err(DeskError::Width {
                row: 2,
                got: 5,
                want: 4
            })
        );
    }

    #[test]
    fn a_header_without_the_three_it_needs_says_which() {
        assert_eq!(
            import(SKELETON, "name,dial\nA,12.0\n"),
            Err(DeskError::MissingColumn("number"))
        );
        assert_eq!(
            import(SKELETON, "number,driver,class\n"),
            Err(DeskError::Empty)
        );
    }

    #[test]
    fn columns_the_club_needs_and_we_do_not_are_ignored() {
        // Every real registration list has paid/tech/phone in it.
        let csv = "paid,number,phone,driver,class,tech\n\
                   yes,9,+7900,D. Kuznetsov,Super Gas,ok\n";
        let sheet = Sheet::parse(&import(SKELETON, csv).unwrap()).unwrap();
        assert_eq!(sheet.entries[0].number, 9);
        assert_eq!(sheet.entries[0].driver, "D. Kuznetsov");
    }

    #[test]
    fn an_event_id_that_cannot_be_a_url_is_caught_at_the_desk() {
        // The alternative is finding out at the end of the day, when the day is
        // the thing that will not upload.
        let bad = SKELETON.replace("club-day", "Club Day 2026");
        let csv = "number,driver,class\n9,A,Super Gas\n";
        let err = import(&bad, csv).unwrap_err();
        assert!(
            matches!(err, DeskError::Rejected(ref w) if w.contains("cannot go in a URL")),
            "{err}"
        );
    }

    #[test]
    fn the_entry_list_reads_without_knowing_toml() {
        let csv = "number,driver,class,dial_s\n7,A. Ivanov,Street bracket,12.35\n";
        let sheet = Sheet::parse(&import(SKELETON, csv).unwrap()).unwrap();
        let list = entry_list(&sheet);
        assert!(list.contains("Club day — 2026-08-15  [club-day]"), "{list}");
        assert!(list.contains("#7  A. Ivanov"), "{list}");
        assert!(list.contains("dial 12.35"), "{list}");
        assert!(list.contains("Super Gas  —  index 9.90"), "{list}");
    }
}
