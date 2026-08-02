#![forbid(unsafe_code)]

//! `scope` — an instrument you point at a session.
//!
//! One self-contained page showing every layer of a round at the same instant:
//! the strip with the cars on it, the tree, the live beam states, the staging
//! machine, the poller's event stream, and the bus tape underneath. Scrub the
//! timeline and all of them move together.
//!
//! ## What it is not
//!
//! Three words in this project already mean specific things, and none of them is
//! this. **Bench** is the physical T1–T5 rig that **D15** gates purchases on.
//! **Console** is the operator's box at the start line (**D07**). **Scoreboard**
//! is the spectator display the race-control binary serves in-process
//! (**D23**). `scope` is a development instrument: it shows what the software
//! did, and it is named for the evidence bar `CONTRIBUTING.md` sets — a
//! datasheet, a **scope trace**, a field failure.
//!
//! **Nothing it draws has run against hardware.** Every number came from a
//! scenario file that stated it.
//!
//! ## Why it is one file with no server
//!
//! The whole session is embedded and replayed in the browser, so a scope page
//! opens from a file:// URL, survives being emailed, and can be committed beside
//! a disputed round. That also keeps **D23**'s "one static binary" honest: no
//! web framework was added to look at a simulation.
//!
//! ## What is drawn versus what was measured
//!
//! Beam crossings are measured. A car's position *between* two beams is drawn by
//! interpolating between them, and the page says so — the same rule the time
//! slip follows, applied to pixels instead of numbers.

use std::fmt::Write;

use beam402_mapping::{Beam, Mapping};

mod json;
mod render;

pub use render::{body, page};

/// One address's live state, as the master last saw it.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeState {
    pub address: u8,
    /// `input_state` from the digest: a **set** bit is an intact beam (**D17**).
    pub inputs: u16,
    pub silent: bool,
    pub identified: bool,
}

/// One transaction on the wire.
#[derive(Clone, Debug)]
pub struct Txn {
    pub write: bool,
    pub address: u8,
    /// The block the register lands in, resolved through the map — `digest`,
    /// `run_record`, `tree`. A raw address means nothing at a glance.
    pub block: String,
    pub words: u16,
    pub ok: bool,
}

/// One pass of the poll loop, frozen.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub t_ms: u64,
    pub phase: String,
    pub positions: [String; 2],
    /// The staging bits the **master** last wrote.
    pub lamps: u16,
    /// The lamp word the **tree** last reported, and how long ago.
    ///
    /// These are separate for a reason worth seeing: across the quiet window the
    /// master transmits nothing, so it does not watch the cascade run. The green
    /// happens while nobody is looking, and the reaction times are read out of
    /// the tree afterwards. A page that animated three ambers here would be
    /// drawing something the system never saw.
    pub tree_lamps: Option<u16>,
    pub tree_age_ms: u64,
    pub nodes: Vec<NodeState>,
    pub txns: Vec<Txn>,
    pub events: Vec<String>,
    pub bus_ms: f64,
}

/// A beam a car actually crossed: a distance, and the time it was crossed at.
#[derive(Clone, Debug)]
pub struct Crossing {
    pub lane: u8,
    pub beam: String,
    pub at_m: f64,
    /// Seconds from that car's own launch pulse (**D04**).
    pub t_s: f64,
}

/// Where a beam sits on the strip, and which input reports it.
#[derive(Clone, Debug)]
pub struct BeamMark {
    pub lane: u8,
    pub beam: String,
    pub address: u8,
    pub input: u8,
    /// Metres from the starting line.
    pub at_m: f64,
    /// True when the placement is the drawing's assumption rather than a
    /// measurement the mapping file holds.
    pub assumed: bool,
}

/// Everything the page needs.
#[derive(Clone, Debug)]
pub struct Capture {
    pub title: String,
    pub venue: String,
    pub lanes: u8,
    pub finish_m: f64,
    pub beams: Vec<BeamMark>,
    pub labels: Vec<(u8, String)>,
    pub format: String,
    pub dials: Option<(f64, f64)>,
    pub handicap_ms: [u16; 2],
    /// Where each lane's run sits on the loop's clock.
    ///
    /// The master is deliberately silent across the launch (`architecture.md`
    /// §3), so it cannot have watched one happen — no anchor here can be better
    /// than one poll cycle. It is taken from the **finish**: the cycle the finish
    /// crossing arrived in, less the measured ET. That puts the error where it
    /// shows least, because the eye is at the stripe, and every interval inside
    /// the run is still the node's own register to the tick.
    pub launch_ms: [Option<u64>; 2],
    pub crossings: Vec<Crossing>,
    pub frames: Vec<Frame>,
    pub slip: String,
    pub source: String,
}

/// Place every mapped beam along the strip.
///
/// Four of the five distances come straight from the mapping file. The trap does
/// not: the file records the **base** between its two beams, because that is all
/// trap speed needs (`architecture.md` §2), and never where the pair sits. The
/// drawing puts the exit on the finish line and the entry one base uptrack, which
/// is established strip practice — and marks both as assumed, so nobody reads a
/// picture as a survey.
pub fn beam_marks(mapping: &Mapping) -> Vec<BeamMark> {
    let g = &mapping.geometry;
    let mut out = Vec::new();
    for site in mapping.sites() {
        let (at_m, assumed) = match site.beam {
            Beam::Prestage => (-0.178, false),
            Beam::Stage => (0.0, false),
            Beam::Guard => (g.stage_to_guard.unwrap_or(0.340), false),
            Beam::Interval60 => (g.sixty_foot, false),
            Beam::Interval660 => (g.eighth_mile.unwrap_or(g.finish / 2.0), false),
            Beam::TrapExit => (g.finish, true),
            Beam::TrapEntry => (g.finish - g.trap_base.unwrap_or(20.0), true),
            Beam::Finish => (g.finish, false),
        };
        out.push(BeamMark {
            lane: site.lane.number(),
            beam: site.beam.wire_name().to_string(),
            address: site.address,
            input: site.input,
            at_m,
            assumed,
        });
    }
    out
}

/// Which block an address falls in, for the bus tape. A raw register number is
/// not something anybody reads at a glance.
pub fn block_of(reg: u16) -> String {
    for desc in beam402_protocol::REGISTER_MAP {
        for base in desc.addrs {
            if reg >= *base && reg < base + desc.len {
                return match desc.lanes.len() {
                    2 if desc.addrs.len() == 2 => {
                        let lane = if reg >= desc.addrs[1] { 2 } else { 1 };
                        format!("{}·L{lane}", desc.name)
                    }
                    _ => desc.name.to_string(),
                };
            }
        }
    }
    format!("{reg:#06x}")
}

impl Capture {
    fn to_json(&self) -> String {
        use json::{arr, num, obj, str_};
        let mut s = String::new();
        let _ = write!(
            s,
            "{{{},{},{},{},{},{},{},{},{},{},{},{},{}}}",
            str_("title", &self.title),
            str_("venue", &self.venue),
            num("lanes", self.lanes as f64),
            num("finish_m", self.finish_m),
            str_("format", &self.format),
            str_("source", &self.source),
            arr(
                "handicap",
                self.handicap_ms.iter().map(|v| format!("{v}")).collect()
            ),
            match self.dials {
                Some((a, b)) => arr("dials", vec![format!("{a}"), format!("{b}")]),
                None => "\"dials\":null".to_string(),
            },
            arr(
                "labels",
                self.labels
                    .iter()
                    .map(|(a, l)| obj(&[num("a", *a as f64), str_("label", l)]))
                    .collect()
            ),
            arr(
                "beams",
                self.beams
                    .iter()
                    .map(|b| obj(&[
                        num("lane", b.lane as f64),
                        str_("beam", &b.beam),
                        num("a", b.address as f64),
                        num("i", b.input as f64),
                        num("m", b.at_m),
                        num("assumed", if b.assumed { 1.0 } else { 0.0 }),
                    ]))
                    .collect()
            ),
            arr(
                "launch",
                self.launch_ms
                    .iter()
                    .map(|v| match v {
                        Some(t) => format!("{t}"),
                        None => "null".to_string(),
                    })
                    .collect()
            ),
            arr(
                "crossings",
                self.crossings
                    .iter()
                    .map(|c| {
                        obj(&[
                            num("lane", c.lane as f64),
                            str_("beam", &c.beam),
                            num("m", c.at_m),
                            num("t", c.t_s),
                        ])
                    })
                    .collect()
            ),
            arr("frames", self.frames.iter().map(frame_json).collect()),
        );
        s
    }
}

fn frame_json(f: &Frame) -> String {
    use json::{arr, num, obj, str_};
    obj(&[
        num("t", f.t_ms as f64),
        str_("phase", &f.phase),
        arr(
            "pos",
            f.positions
                .iter()
                .map(|p| json::quote(p))
                .collect::<Vec<_>>(),
        ),
        num("lamps", f.lamps as f64),
        match f.tree_lamps {
            Some(v) => num("tree_lamps", v as f64),
            None => "\"tree_lamps\":null".to_string(),
        },
        num("tree_age", f.tree_age_ms as f64),
        num("bus_ms", f.bus_ms),
        arr(
            "nodes",
            f.nodes
                .iter()
                .map(|n| {
                    obj(&[
                        num("a", n.address as f64),
                        num("in", n.inputs as f64),
                        num("silent", if n.silent { 1.0 } else { 0.0 }),
                        num("id", if n.identified { 1.0 } else { 0.0 }),
                    ])
                })
                .collect(),
        ),
        arr(
            "txns",
            f.txns
                .iter()
                .map(|t| {
                    obj(&[
                        num("w", if t.write { 1.0 } else { 0.0 }),
                        num("a", t.address as f64),
                        str_("b", &t.block),
                        num("n", t.words as f64),
                        num("ok", if t.ok { 1.0 } else { 0.0 }),
                    ])
                })
                .collect(),
        ),
        arr(
            "events",
            f.events.iter().map(|e| json::quote(e)).collect::<Vec<_>>(),
        ),
    ])
}
