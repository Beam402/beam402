//! Carrying a day to a server (**D33**).
//!
//! The unit of synchronization is **a line of the result log**. The client
//! uploads the entry sheet once and then appends lines from an offset; the
//! receiver stores them verbatim and derives everything with [`Progress`], the
//! same code that derives them trackside.
//!
//! That is the whole design, and it answers four questions at once. Appending
//! after every recorded pair and appending the whole file that evening are the
//! same operation with different batch sizes, so **live and bulk are not modes**.
//! Qualifying and eliminations are records of different kinds in one stream, so
//! **together or separately is not a question**. An interrupted upload resumes,
//! because an offset is all the state the transfer has. And the online ladder
//! cannot contradict the one the tower is racing off, because it is not a second
//! derivation of it.
//!
//! ## Nothing here does any I/O
//!
//! [`Held`] is one event's two files as strings. Where they live — a directory, a
//! database, an object store — is the receiving program's business, and keeping
//! that out of here is what makes the refusals testable.
//!
//! ## Correctness is carried by three refusals
//!
//! - **The offset must match.** A retried upload the receiver already applied
//!   fails with the true count instead of appending twice. Idempotence by
//!   refusal rather than by de-duplication.
//! - **The prefix must match.** The client says what it believes is already
//!   there. Two writers appending to one event id would otherwise fork it in
//!   silence; this makes the fork an error at the first append.
//! - **A replaced sheet must still fit the log.** Late entries before racing
//!   starts are ordinary, so the sheet is replaceable — but only while every
//!   class and entry the log already names still exists in it.

use crate::sheet::Record;
use crate::{Progress, Sheet};

/// Where an event stands, as far as a client needs to know to continue.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cursor {
    /// Lines held. This is the `from` a client's next append must use.
    pub lines: usize,
    /// Digest of the sheet those lines belong to.
    pub sheet: String,
}

/// What an append did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Appended {
    pub lines: usize,
    pub added: usize,
    /// Lines that do not parse, mirrored anyway and counted. A torn last line
    /// after a power cut is normal, and a receiver that silently repaired the day
    /// would be a receiver whose ladder is not the one that was raced.
    pub skipped: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SyncError {
    /// The client is not where the receiver is. Carries the truth, so the answer
    /// is to continue from there rather than to give up.
    Offset { held: usize, offered: usize },
    /// The client's idea of the first `at` lines is not the receiver's. Two
    /// writers, one event id.
    Forked { at: usize },
    /// A sheet that would orphan results already recorded.
    WouldOrphan(Vec<String>),
    /// The sheet itself does not load.
    BadSheet(String),
}

impl core::fmt::Display for SyncError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SyncError::Offset { held, offered } => write!(
                f,
                "the log is {held} lines long, and an append from {offered} was offered"
            ),
            SyncError::Forked { at } => write!(
                f,
                "the first {at} lines here are not the ones being continued — \
                 two writers have appended to one event"
            ),
            SyncError::WouldOrphan(what) => write!(
                f,
                "that sheet would orphan results already recorded: {}",
                what.join(", ")
            ),
            SyncError::BadSheet(why) => write!(f, "{why}"),
        }
    }
}

/// One event as a receiver holds it: two files, and no other state.
#[derive(Clone, Default, Debug)]
pub struct Held {
    pub sheet: String,
    pub log: String,
}

impl Held {
    pub fn new(sheet: String, log: String) -> Held {
        Held { sheet, log }
    }

    /// The log as lines, blank ones dropped. This is the canonical form
    /// everything else counts and digests, so a trailing newline is never the
    /// difference between agreeing and forking.
    pub fn lines(&self) -> Vec<&str> {
        self.log
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect()
    }

    pub fn cursor(&self) -> Cursor {
        Cursor {
            lines: self.lines().len(),
            sheet: digest(&self.sheet),
        }
    }

    /// Accept a sheet — a new event, the same sheet again, or a replacement the
    /// log still fits.
    pub fn offer_sheet(&mut self, text: &str) -> Result<Cursor, SyncError> {
        let sheet = Sheet::parse(text).map_err(SyncError::BadSheet)?;
        let orphans = self.orphans(&sheet);
        if !orphans.is_empty() {
            return Err(SyncError::WouldOrphan(orphans));
        }
        self.sheet = text.to_string();
        Ok(self.cursor())
    }

    /// What the log names that a sheet does not declare.
    ///
    /// Checked against the *records*, not against the old sheet, because the
    /// question is not "did the sheet change" — a club adding a late entry
    /// between rounds is ordinary — but "does everything already written still
    /// mean something".
    fn orphans(&self, sheet: &Sheet) -> Vec<String> {
        let mut out = Vec::new();
        for line in self.lines() {
            let Some(record) = Record::parse(line) else {
                continue;
            };
            let class = record.class();
            if sheet.class(class).is_none() {
                let what = format!("class {class:?}");
                if !out.contains(&what) {
                    out.push(what);
                }
            }
            for e in record.entries() {
                if sheet.entry(e).is_none() {
                    let what = format!("entry #{}", e.0);
                    if !out.contains(&what) {
                        out.push(what);
                    }
                }
            }
        }
        out
    }

    /// Append lines starting at `from`.
    ///
    /// `prefix` is the client's digest of the first `from` lines. Both checks are
    /// the point of the function: without the offset an upload retried after a
    /// timeout appends twice, and without the prefix two clubs sharing an event
    /// id interleave their days into one log that replays as neither.
    pub fn append(&mut self, from: usize, prefix: &str, body: &str) -> Result<Appended, SyncError> {
        let held = self.lines();
        if from != held.len() {
            return Err(SyncError::Offset {
                held: held.len(),
                offered: from,
            });
        }
        if digest_lines(&held) != prefix {
            return Err(SyncError::Forked { at: from });
        }

        let fresh: Vec<&str> = body
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect();
        let skipped = fresh
            .iter()
            .filter(|l| !l.starts_with('#') && Record::parse(l).is_none())
            .count();

        let mut all: Vec<&str> = held;
        all.extend(fresh.iter().copied());
        let added = fresh.len();
        let lines = all.len();
        self.log = all.join("\n");
        if !self.log.is_empty() {
            self.log.push('\n');
        }
        Ok(Appended {
            lines,
            added,
            skipped,
        })
    }

    /// The day, derived. This is the same call the tower makes, which is the
    /// reason the two cannot disagree.
    pub fn day(&self) -> Result<(Progress, usize), String> {
        let sheet = Sheet::parse(&self.sheet)?;
        Ok(Progress::replay(sheet, &self.log))
    }
}

/// What a client has to send to continue from `from`.
pub fn prefix_digest(log: &str, from: usize) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .take(from)
        .collect();
    digest_lines(&lines)
}

/// Everything from `from` onward, as a body to append.
pub fn tail(log: &str, from: usize) -> String {
    let lines: Vec<&str> = log
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .skip(from)
        .collect();
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn digest_lines(lines: &[&str]) -> String {
    digest(&lines.join("\n"))
}

/// FNV-1a, 64-bit, written out.
///
/// **A mismatch detector, not a security boundary.** What it has to catch is the
/// wrong file and a forked log; anybody who can append to an event can append
/// whatever they like, and the answer to that is whatever authentication the
/// receiver puts in front of this — not a timing question. Written out rather
/// than depended on for the same reason as everything else here (**D23**), and
/// because a dependency that improved its hash would silently invalidate every
/// cursor a client is holding.
pub fn digest(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EntryId;

    const SHEET: &str = r#"
[event]
id = "club-day"
name = "Club day"
date = "2026-08-15"
[[class]]
name = "Super Gas"
format = "index"
index_s = 9.90
seeding = "quickest-et"
ladder = "pro"
[[entry]]
number = 1
driver = "A"
class = "Super Gas"
[[entry]]
number = 2
driver = "B"
class = "Super Gas"
[[entry]]
number = 3
driver = "C"
class = "Super Gas"
[[entry]]
number = 4
driver = "D"
class = "Super Gas"
"#;

    /// A whole day's log, produced the way race control produces it.
    fn a_days_log() -> String {
        let mut day = Progress::new(Sheet::parse(SHEET).unwrap());
        let mut lines = Vec::new();
        for (n, et) in [(1u32, 9.91), (2, 9.95), (3, 9.99), (4, 10.20)] {
            lines.push(
                day.qualified("Super Gas", EntryId(n), Some(et), None, false)
                    .unwrap()
                    .line(),
            );
        }
        lines.push(day.draw("Super Gas").unwrap().line());
        while day.champion("Super Gas").is_none() {
            let deck = day.next_pair_in("Super Gas").unwrap();
            let r = if deck.is_bye() {
                day.bye("Super Gas", deck.position, true).unwrap()
            } else {
                day.won("Super Gas", deck.position, deck.left).unwrap()
            };
            lines.push(r.line());
        }
        lines.join("\n") + "\n"
    }

    /// A client pushing everything it has, from wherever the receiver is.
    fn push(held: &mut Held, sheet: &str, log: &str) -> Result<Appended, SyncError> {
        let cursor = held.offer_sheet(sheet)?;
        held.append(
            cursor.lines,
            &prefix_digest(log, cursor.lines),
            &tail(log, cursor.lines),
        )
    }

    #[test]
    fn uploading_line_by_line_and_all_at_once_end_in_the_same_place() {
        // **D33**'s central claim: live and bulk are the same operation. A club
        // with signal pushes after every pair; a club without pushes that
        // evening; the server cannot tell and the ladder is identical.
        let log = a_days_log();

        let mut live = Held::default();
        live.offer_sheet(SHEET).unwrap();
        for (i, line) in log.lines().enumerate() {
            live.append(i, &prefix_digest(&log, i), &format!("{line}\n"))
                .unwrap();
        }

        let mut bulk = Held::default();
        push(&mut bulk, SHEET, &log).unwrap();

        assert_eq!(live.log, bulk.log);
        let (a, _) = live.day().unwrap();
        let (b, _) = bulk.day().unwrap();
        assert_eq!(a.champion("Super Gas"), b.champion("Super Gas"));
        assert!(a.champion("Super Gas").is_some(), "and it is a real day");
    }

    #[test]
    fn a_retried_upload_is_refused_rather_than_applied_twice() {
        // The failure a network makes inevitable: the receiver applied it, the
        // response never arrived, the client sends it again.
        let log = a_days_log();
        let mut held = Held::default();
        push(&mut held, SHEET, &log).unwrap();
        let once = held.log.clone();

        let err = held
            .append(0, &prefix_digest(&log, 0), &tail(&log, 0))
            .unwrap_err();
        assert_eq!(
            err,
            SyncError::Offset {
                held: log.lines().count(),
                offered: 0
            }
        );
        assert_eq!(held.log, once, "and nothing was duplicated");

        // Resuming from where it actually is adds nothing, which is what makes
        // "push everything you have" safe to run on a timer.
        let a = push(&mut held, SHEET, &log).unwrap();
        assert_eq!(a.added, 0);
        assert_eq!(held.log, once);
    }

    #[test]
    fn an_interrupted_upload_resumes() {
        let log = a_days_log();
        let all = log.lines().count();
        let mut held = Held::default();
        held.offer_sheet(SHEET).unwrap();

        // Three lines get through before the connection dies.
        let partial: String = log
            .lines()
            .take(3)
            .map(|l| format!("{l}\n"))
            .collect::<String>();
        held.append(0, &prefix_digest(&log, 0), &partial).unwrap();
        assert_eq!(held.cursor().lines, 3);

        let a = push(&mut held, SHEET, &log).unwrap();
        assert_eq!((a.lines, a.added), (all, all - 3));
        assert!(held.day().unwrap().0.champion("Super Gas").is_some());
    }

    #[test]
    fn two_writers_on_one_event_id_are_caught_at_the_first_append() {
        // Without this the two days interleave into one log that replays as
        // neither, and nothing ever says so.
        let log = a_days_log();
        let mut held = Held::default();
        held.offer_sheet(SHEET).unwrap();
        let first_two: String = log.lines().take(2).map(|l| format!("{l}\n")).collect();
        held.append(0, &prefix_digest(&log, 0), &first_two).unwrap();

        // A second client that has its own first two lines and agrees about the
        // count — which is exactly the case an offset alone cannot catch.
        let mine = "Q Super_Gas 4 10.2000 - -\nQ Super_Gas 3 9.9900 - -\n";
        let err = held
            .append(2, &prefix_digest(mine, 2), "Q Super_Gas 3 9.9900 - -\n")
            .unwrap_err();
        assert_eq!(err, SyncError::Forked { at: 2 });
    }

    #[test]
    fn a_torn_line_is_mirrored_and_counted_rather_than_rejected() {
        // The receiver's job is to be a faithful copy of that file. A batch
        // refused for one bad line is a day that can never be uploaded.
        let mut held = Held::default();
        held.offer_sheet(SHEET).unwrap();
        let a = held
            .append(
                0,
                &prefix_digest("", 0),
                "Q Super_Gas 1 9.9100 - -\nW Super_Ga\n",
            )
            .unwrap();
        assert_eq!((a.added, a.skipped), (2, 1));
        assert_eq!(held.day().unwrap().1, 1, "and the derivation says so too");
    }

    #[test]
    fn a_late_entry_may_be_added_but_a_recorded_one_may_not_be_removed() {
        // Both halves matter. Registration is still open at ten in the morning,
        // and a sheet edit that orphans a result somebody has already been told
        // about is a different thing entirely.
        let log = a_days_log();
        let mut held = Held::default();
        push(&mut held, SHEET, &log).unwrap();

        let with_a_late_entry =
            SHEET.to_string() + "[[entry]]\nnumber = 5\ndriver = \"E\"\nclass = \"Super Gas\"\n";
        assert!(held.offer_sheet(&with_a_late_entry).is_ok());

        let without_entry_2 = SHEET.replace(
            "[[entry]]\nnumber = 2\ndriver = \"B\"\nclass = \"Super Gas\"\n",
            "",
        );
        assert_eq!(
            held.offer_sheet(&without_entry_2),
            Err(SyncError::WouldOrphan(vec!["entry #2".into()]))
        );
    }

    #[test]
    fn the_wrong_sheet_is_a_mismatch_the_client_can_see() {
        // The cursor carries the sheet digest so a client pushing yesterday's log
        // at today's event finds out before it appends anything.
        let mut held = Held::default();
        let a = held.offer_sheet(SHEET).unwrap();
        let other = SHEET.replace("Club day", "Another day");
        let b = held.offer_sheet(&other).unwrap();
        assert_ne!(a.sheet, b.sheet);
        assert_eq!(a.sheet, digest(SHEET));
    }

    #[test]
    fn a_sheet_that_does_not_load_is_refused_before_it_replaces_anything() {
        let mut held = Held::default();
        held.offer_sheet(SHEET).unwrap();
        let kept = held.sheet.clone();
        assert!(matches!(
            held.offer_sheet("[event]\nname = \"x\"\n"),
            Err(SyncError::BadSheet(_))
        ));
        assert_eq!(held.sheet, kept);
    }
}
