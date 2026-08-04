#![forbid(unsafe_code)]

//! Every word the master said, and every word it heard back.
//!
//! **D26** claims that a recorded bus session is a test fixture and replays
//! deterministically — "here is the session, replay it, get the same ET". This
//! crate is that claim's implementation, and until it existed the claim was a
//! sentence in a document.
//!
//! ## Why it goes through the same seam
//!
//! [`Replay`] is a third [`Bus`] alongside the simulator and the serial port
//! that does not exist yet. Nothing above the seam learns which one it has, so a
//! replayed round runs the *actual* poller, the *actual* staging machine and the
//! *actual* race logic. A replay harness that re-derived results from a summary
//! would prove that the summary was consistent with itself, which is not the
//! question anyone asks about a disputed time slip.
//!
//! ## Why the format is text
//!
//! A session is dispute evidence. Somebody who does not have this program — the
//! other driver, a tech official, a contributor on a different branch — has to be
//! able to open it and see what happened. So it is one line per transaction, hex
//! words, no framing, no dependency:
//!
//! ```text
//! beam402-session 1
//! M [venue]
//! M name = "Sim Strip"
//! T 100
//! R 1 0000 4 0007 0000 0001 000f
//! X 3 0000 timeout
//! W 10 0100 0010 0000 02bc 0001
//! ```
//!
//! `R` is a read that answered, `W` a write that was accepted, `X` a transaction
//! that failed, `T` time passing. Any other uppercase tag is **metadata** the
//! recorder was handed and the replay gives back untouched — the mapping file and
//! the pairing ride there, which is what makes a session self-contained rather
//! than a set of numbers that needs three other files to mean anything.
//!
//! ## What a divergence means
//!
//! [`Replay`] refuses to answer a request the recording does not have at that
//! point. That is deliberate and it is the useful half: a replay that diverges
//! says **the logic changed**, which is exactly what a regression looks like from
//! the outside. A replay that quietly served the nearest matching response would
//! turn a changed poll schedule into a silently different time slip.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write;

use beam402_bus::{Bus, BusError, CallUp, Paced};

const MAGIC: &str = "beam402-session";
const VERSION: u32 = 1;

/// One thing that happened, in order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Entry {
    /// A line the recorder was handed. Tag, then the rest of the line verbatim.
    Meta(char, String),
    /// Time advanced by this many milliseconds.
    Tick(u64),
    Read {
        address: u8,
        reg: u16,
        words: Vec<u16>,
    },
    Write {
        address: u8,
        reg: u16,
        words: Vec<u16>,
    },
    Failed {
        address: u8,
        reg: u16,
        error: BusError,
    },
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseError {
    pub line: usize,
    pub why: String,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "line {}: {}", self.line, self.why)
    }
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Wraps a bus and writes down everything that crosses it.
///
/// It is transparent: errors pass through unchanged, because a session that
/// silently repaired a failed transaction would be evidence of a race that did
/// not happen.
pub struct Recorder<B, W> {
    bus: B,
    out: W,
}

impl<B, W: Write> Recorder<B, W> {
    pub fn new(bus: B, mut out: W) -> std::io::Result<Self> {
        writeln!(out, "{MAGIC} {VERSION}")?;
        Ok(Recorder { bus, out })
    }

    /// Write a metadata line. `tag` must not be one of the reserved log tags —
    /// `R`, `W`, `X`, `T` — and multi-line values are written one line each so
    /// the format stays one grammar.
    pub fn meta(&mut self, tag: char, text: &str) -> std::io::Result<()> {
        debug_assert!(!matches!(tag, 'R' | 'W' | 'X' | 'T'), "reserved tag {tag}");
        for line in text.lines() {
            writeln!(self.out, "{tag} {line}")?;
        }
        Ok(())
    }

    pub fn into_inner(self) -> (B, W) {
        (self.bus, self.out)
    }

    fn note(&mut self, entry: &Entry) {
        // A session that cannot be written is not worth failing a round for: the
        // race is the product, the recording is evidence about it. The loss is
        // reported by the file being short, which is visible.
        let _ = writeln!(self.out, "{}", render(entry));
    }
}

impl<B: Bus, W: Write> Bus for Recorder<B, W> {
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError> {
        let result = self.bus.read(address, reg, out);
        let entry = match result {
            Ok(()) => Entry::Read {
                address,
                reg,
                words: out.to_vec(),
            },
            Err(error) => Entry::Failed {
                address,
                reg,
                error,
            },
        };
        self.note(&entry);
        result
    }

    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError> {
        let result = self.bus.write(address, reg, values);
        let entry = match result {
            Ok(()) => Entry::Write {
                address,
                reg,
                words: values.to_vec(),
            },
            Err(error) => Entry::Failed {
                address,
                reg,
                error,
            },
        };
        self.note(&entry);
        result
    }
}

impl<B: Paced, W: Write> Paced for Recorder<B, W> {
    fn advance_ms(&mut self, ms: u64) {
        self.note(&Entry::Tick(ms));
        self.bus.advance_ms(ms);
    }
}

impl<B: CallUp, W: Write> CallUp for Recorder<B, W> {
    /// Passed through and **not** recorded. It is not a bus transaction — it is a
    /// simulator being told the next pair pulled in — and a replay does not
    /// simulate, it re-serves what was recorded. Nothing about it is evidence.
    fn call_up(&mut self) {
        self.bus.call_up();
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// A recorded session, served back through [`Bus`].
#[derive(Clone, Debug)]
pub struct Replay {
    entries: Vec<Entry>,
    cursor: usize,
    diverged: Option<String>,
}

impl Replay {
    pub fn parse(text: &str) -> Result<Replay, ParseError> {
        let entries = parse(text)?;
        Ok(Replay {
            entries,
            cursor: 0,
            diverged: None,
        })
    }

    /// Every metadata line under one tag, rejoined. The mapping file comes back
    /// out of here exactly as it went in.
    pub fn meta(&self, tag: char) -> String {
        let mut out = String::new();
        for e in &self.entries {
            if let Entry::Meta(t, line) = e {
                if *t == tag {
                    let _ = writeln!(out, "{line}");
                }
            }
        }
        out
    }

    /// What the recording holds, for a reader that wants to look rather than
    /// replay.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Where the replay stopped agreeing with the recording, if it did.
    pub fn divergence(&self) -> Option<&str> {
        self.diverged.as_deref()
    }

    /// Every transaction has been served.
    pub fn is_exhausted(&self) -> bool {
        self.next_log().is_none()
    }

    fn next_log(&self) -> Option<usize> {
        (self.cursor..self.entries.len()).find(|i| !matches!(self.entries[*i], Entry::Meta(..)))
    }

    fn diverge<T>(&mut self, why: String) -> Result<T, BusError> {
        if self.diverged.is_none() {
            self.diverged = Some(why);
        }
        Err(BusError::Transport)
    }

    /// Take the next logged entry, or record that the recording ran out.
    fn take(&mut self, wanted: &str) -> Result<Entry, BusError> {
        match self.next_log() {
            Some(i) => {
                self.cursor = i + 1;
                Ok(self.entries[i].clone())
            }
            None => self.diverge(format!(
                "the recording ends, and the master still wants {wanted}"
            )),
        }
    }
}

impl Bus for Replay {
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError> {
        let wanted = format!(
            "a read of {} registers at {reg:#06x} from {address}",
            out.len()
        );
        match self.take(&wanted)? {
            Entry::Read {
                address: a,
                reg: r,
                words,
            } if a == address && r == reg && words.len() == out.len() => {
                out.copy_from_slice(&words);
                Ok(())
            }
            Entry::Failed {
                address: a,
                reg: r,
                error,
            } if a == address && r == reg => Err(error),
            other => self.diverge(format!("{wanted}; the recording has {}", render(&other))),
        }
    }

    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError> {
        let wanted = format!("a write of {values:?} at {reg:#06x} to {address}");
        match self.take(&wanted)? {
            Entry::Write {
                address: a,
                reg: r,
                words,
            } if a == address && r == reg && words == values => Ok(()),
            Entry::Failed {
                address: a,
                reg: r,
                error,
            } if a == address && r == reg => Err(error),
            other => self.diverge(format!("{wanted}; the recording has {}", render(&other))),
        }
    }
}

impl Paced for Replay {
    fn advance_ms(&mut self, ms: u64) {
        // Time is checked like everything else. A loop that steps differently
        // from the one that recorded the session is a loop whose results are not
        // comparable, and saying so is the whole point.
        match self.next_log() {
            Some(i) if self.entries[i] == Entry::Tick(ms) => self.cursor = i + 1,
            Some(i) => {
                let found = render(&self.entries[i]);
                if self.diverged.is_none() {
                    self.diverged =
                        Some(format!("time advanced {ms} ms; the recording has {found}"));
                }
            }
            None => {
                if self.diverged.is_none() {
                    self.diverged = Some(format!("the recording ends; time advanced {ms} ms"));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The format
// ---------------------------------------------------------------------------

fn render(entry: &Entry) -> String {
    let words = |w: &[u16]| {
        w.iter()
            .map(|v| format!("{v:04x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    match entry {
        Entry::Meta(tag, line) => format!("{tag} {line}"),
        Entry::Tick(ms) => format!("T {ms}"),
        Entry::Read {
            address,
            reg,
            words: w,
        } => format!("R {address} {reg:04x} {} {}", w.len(), words(w)),
        Entry::Write {
            address,
            reg,
            words: w,
        } => format!("W {address} {reg:04x} {}", words(w)),
        Entry::Failed {
            address,
            reg,
            error,
        } => format!("X {address} {reg:04x} {}", failure(*error)),
    }
}

fn failure(e: BusError) -> String {
    match e {
        BusError::Timeout => "timeout".into(),
        BusError::Exception(c) => format!("exception:{c}"),
        BusError::ShortFrame { asked, got } => format!("short:{asked}/{got}"),
        BusError::Decode(_) => "decode".into(),
        BusError::Transport => "transport".into(),
    }
}

fn parse_failure(s: &str) -> Option<BusError> {
    if s == "timeout" {
        return Some(BusError::Timeout);
    }
    if s == "transport" {
        return Some(BusError::Transport);
    }
    if let Some(code) = s.strip_prefix("exception:") {
        return code.parse().ok().map(BusError::Exception);
    }
    if let Some(rest) = s.strip_prefix("short:") {
        let (a, g) = rest.split_once('/')?;
        return Some(BusError::ShortFrame {
            asked: a.parse().ok()?,
            got: g.parse().ok()?,
        });
    }
    // `decode` is not reconstructible — DecodeError carries a length that the
    // line does not keep, and nothing downstream distinguishes them. It replays
    // as a transport failure, which is the same shape: the transaction produced
    // no registers.
    (s == "decode").then_some(BusError::Transport)
}

fn parse(text: &str) -> Result<Vec<Entry>, ParseError> {
    let mut lines = text.lines().enumerate();
    let (n, first) = lines.next().ok_or(ParseError {
        line: 0,
        why: "empty file".into(),
    })?;
    let mut header = first.split_whitespace();
    if header.next() != Some(MAGIC) {
        return Err(ParseError {
            line: n + 1,
            why: format!("not a session file: expected {MAGIC:?} on the first line"),
        });
    }
    match header.next().and_then(|v| v.parse::<u32>().ok()) {
        Some(VERSION) => {}
        Some(other) => {
            return Err(ParseError {
                line: n + 1,
                why: format!("session version {other}, this build reads {VERSION}"),
            })
        }
        None => {
            return Err(ParseError {
                line: n + 1,
                why: "no version on the header line".into(),
            })
        }
    }

    let mut out = Vec::new();
    for (n, line) in lines {
        let at = |why: String| ParseError { line: n + 1, why };
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (tag, rest) = line.split_at(1);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let tag = tag.chars().next().unwrap_or(' ');
        match tag {
            'T' => out.push(Entry::Tick(
                rest.trim()
                    .parse()
                    .map_err(|_| at(format!("{rest:?} is not a duration")))?,
            )),
            'R' | 'W' | 'X' => {
                let mut f = rest.split_whitespace();
                let address = f
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| at("no address".into()))?;
                let reg = f
                    .next()
                    .and_then(|v| u16::from_str_radix(v, 16).ok())
                    .ok_or_else(|| at("no register".into()))?;
                if tag == 'X' {
                    let kind = f.next().unwrap_or("");
                    let error = parse_failure(kind)
                        .ok_or_else(|| at(format!("unknown failure {kind:?}")))?;
                    out.push(Entry::Failed {
                        address,
                        reg,
                        error,
                    });
                    continue;
                }
                // A read states its count before the words; a write does not,
                // because the words are the count.
                if tag == 'R' {
                    let _declared = f.next();
                }
                let mut words = Vec::new();
                for w in f {
                    words.push(
                        u16::from_str_radix(w, 16)
                            .map_err(|_| at(format!("{w:?} is not a hex word")))?,
                    );
                }
                out.push(if tag == 'R' {
                    Entry::Read {
                        address,
                        reg,
                        words,
                    }
                } else {
                    Entry::Write {
                        address,
                        reg,
                        words,
                    }
                });
            }
            t if t.is_ascii_uppercase() => out.push(Entry::Meta(t, rest.to_string())),
            t => return Err(at(format!("unknown tag {t:?}"))),
        }
    }
    Ok(out)
}

/// How much of a session each address accounts for. Diagnostics, and the cheap
/// way to see that a silent node cost more of the round than every healthy one.
pub fn traffic(entries: &[Entry]) -> BTreeMap<u8, usize> {
    let mut out = BTreeMap::new();
    for e in entries {
        let a = match e {
            Entry::Read { address, .. }
            | Entry::Write { address, .. }
            | Entry::Failed { address, .. } => *address,
            _ => continue,
        };
        *out.entry(a).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded() -> String {
        let mut buf = Vec::new();
        {
            let mut r = Recorder::new(Nothing, &mut buf).unwrap();
            r.meta('M', "[venue]\nname = \"Sim Strip\"").unwrap();
            let mut out = [0u16; 4];
            r.read(1, 0x0000, &mut out).unwrap();
            r.advance_ms(100);
            r.write(10, 0x0100, &[0x0010, 0, 0x02bc, 1]).unwrap();
            let _ = r.read(3, 0x0000, &mut out);
        }
        String::from_utf8(buf).unwrap()
    }

    /// A bus that answers address 1 and 10 and nothing else, so a recording has
    /// both halves in it.
    struct Nothing;
    impl Bus for Nothing {
        fn read(&mut self, address: u8, _reg: u16, out: &mut [u16]) -> Result<(), BusError> {
            if address == 1 {
                out.copy_from_slice(&[7, 0, 1, 0x000f][..out.len()]);
                Ok(())
            } else {
                Err(BusError::Timeout)
            }
        }
        fn write(&mut self, _address: u8, _reg: u16, _values: &[u16]) -> Result<(), BusError> {
            Ok(())
        }
    }
    impl Paced for Nothing {
        fn advance_ms(&mut self, _ms: u64) {}
    }

    #[test]
    fn a_session_is_readable_without_this_program() {
        let text = recorded();
        assert!(text.starts_with("beam402-session 1\n"));
        assert!(text.contains("M [venue]\n"), "{text}");
        assert!(text.contains("R 1 0000 4 0007 0000 0001 000f\n"), "{text}");
        assert!(text.contains("T 100\n"), "{text}");
        assert!(text.contains("W 10 0100 0010 0000 02bc 0001\n"), "{text}");
        assert!(text.contains("X 3 0000 timeout\n"), "{text}");
    }

    #[test]
    fn replaying_it_gives_back_exactly_what_was_recorded() {
        let mut r = Replay::parse(&recorded()).unwrap();
        assert_eq!(r.meta('M'), "[venue]\nname = \"Sim Strip\"\n");

        let mut out = [0u16; 4];
        r.read(1, 0x0000, &mut out).unwrap();
        assert_eq!(out, [7, 0, 1, 0x000f]);
        r.advance_ms(100);
        r.write(10, 0x0100, &[0x0010, 0, 0x02bc, 1]).unwrap();
        // A failure replays as a failure. A session that quietly healed one
        // would be evidence of a race that did not happen.
        assert_eq!(r.read(3, 0x0000, &mut out), Err(BusError::Timeout));

        assert_eq!(r.divergence(), None);
        assert!(r.is_exhausted());
    }

    #[test]
    fn a_replay_that_asks_for_something_else_says_so_instead_of_guessing() {
        // The useful half. A changed poll schedule must not come back as a
        // quietly different time slip.
        let mut r = Replay::parse(&recorded()).unwrap();
        let mut out = [0u16; 4];
        assert_eq!(r.read(2, 0x0000, &mut out), Err(BusError::Transport));
        let why = r.divergence().unwrap();
        assert!(why.contains("from 2"), "{why}");
        assert!(why.contains("R 1 0000"), "{why}");
    }

    #[test]
    fn a_replay_that_steps_time_differently_is_a_divergence_too() {
        let mut r = Replay::parse(&recorded()).unwrap();
        let mut out = [0u16; 4];
        r.read(1, 0x0000, &mut out).unwrap();
        r.advance_ms(250);
        assert!(r.divergence().unwrap().contains("250 ms"));
    }

    #[test]
    fn running_off_the_end_is_named_rather_than_hung() {
        let mut r = Replay::parse("beam402-session 1\n").unwrap();
        let mut out = [0u16; 4];
        assert_eq!(r.read(1, 0, &mut out), Err(BusError::Transport));
        assert!(r.divergence().unwrap().contains("the recording ends"));
    }

    #[test]
    fn a_file_from_another_version_is_refused_not_guessed_at() {
        assert!(Replay::parse("beam402-session 9\n").is_err());
        assert!(Replay::parse("something else\n").is_err());
        let e = Replay::parse("beam402-session 1\nR x 0000 1 0000\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn traffic_shows_where_the_bus_time_went() {
        let r = Replay::parse(&recorded()).unwrap();
        let t = traffic(r.entries());
        assert_eq!(t.get(&1), Some(&1));
        assert_eq!(t.get(&3), Some(&1));
        assert_eq!(t.get(&10), Some(&1));
    }
}
