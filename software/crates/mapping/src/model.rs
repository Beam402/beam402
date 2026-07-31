//! The mapping file as data.
//!
//! One file per venue, versioned in the club's own repository, never on a node
//! (**D08**). Parsing is strict in two places, both deliberate: an unknown beam
//! meaning is a load error, because a typo must not silently drop a beam; and
//! unknown keys are rejected, because a misspelled key is the same failure
//! wearing a different hat.

use std::collections::BTreeMap;
use std::fmt;

use beam402_protocol::Lane;
use serde::Deserialize;

/// A 48-bit factory MAC. Inventory, per-board fault history and the
/// crystal-correction key (**D08**, **D13**) — never an address.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct Mac(pub u64);

impl fmt::Display for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.0.to_be_bytes();
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            b[2], b[3], b[4], b[5], b[6], b[7]
        )
    }
}

impl fmt::Debug for Mac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl TryFrom<String> for Mac {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 6 {
            return Err(format!("expected six colon-separated octets, got {s:?}"));
        }
        let mut v = 0u64;
        for p in parts {
            let byte = u8::from_str_radix(p, 16)
                .map_err(|_| format!("{p:?} is not a hex octet in {s:?}"))?;
            v = (v << 8) | byte as u64;
        }
        Ok(Mac(v))
    }
}

/// Beam meanings are a **closed set**. An unknown value is a load error, not a
/// warning: a typo must not silently drop a beam.
///
/// The set is cross-checked against `beam402_protocol::map::BEAM_MEANINGS` in the
/// tests, so the vocabulary cannot drift between the register spec and here.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash, Deserialize)]
pub enum Beam {
    #[serde(rename = "prestage")]
    Prestage,
    #[serde(rename = "stage")]
    Stage,
    #[serde(rename = "guard")]
    Guard,
    #[serde(rename = "interval_60")]
    Interval60,
    #[serde(rename = "interval_660")]
    Interval660,
    #[serde(rename = "trap_entry")]
    TrapEntry,
    #[serde(rename = "trap_exit")]
    TrapExit,
    #[serde(rename = "finish")]
    Finish,
}

impl Beam {
    pub const ALL: [Beam; 8] = [
        Beam::Prestage,
        Beam::Stage,
        Beam::Guard,
        Beam::Interval60,
        Beam::Interval660,
        Beam::TrapEntry,
        Beam::TrapExit,
        Beam::Finish,
    ];

    pub const fn wire_name(self) -> &'static str {
        match self {
            Beam::Prestage => "prestage",
            Beam::Stage => "stage",
            Beam::Guard => "guard",
            Beam::Interval60 => "interval_60",
            Beam::Interval660 => "interval_660",
            Beam::TrapEntry => "trap_entry",
            Beam::TrapExit => "trap_exit",
            Beam::Finish => "finish",
        }
    }

    /// Does this beam measure time, or is it a staging indicator / validity
    /// check? `architecture.md` §6: pre-stage and guard need no capture channel.
    pub const fn is_timed(self) -> bool {
        !matches!(self, Beam::Prestage | Beam::Guard)
    }

    pub const fn is_trap(self) -> bool {
        matches!(self, Beam::TrapEntry | Beam::TrapExit)
    }
}

impl fmt::Display for Beam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Venue {
    pub name: String,
    pub lanes: u8,
}

/// All distances laser-measured, in metres. `architecture.md` §2: 5 cm of error
/// in the trap base is 0.25 % of speed, which dwarfs the electronics.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Geometry {
    pub sixty_foot: f64,
    pub eighth_mile: Option<f64>,
    pub finish: f64,
    pub trap_base: Option<f64>,
    pub stage_to_guard: Option<f64>,
}

/// Which node's pulse observation provides the launch margin (**D20**).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Margin {
    pub source_address: u8,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputMap {
    pub index: u8,
    pub beam: Beam,
    /// 1 or 2 in the file; parsed to a [`Lane`] by [`InputMap::lane`].
    pub lane: u8,
}

impl InputMap {
    pub fn lane(&self) -> Option<Lane> {
        Lane::from_number(self.lane)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub address: u8,
    pub label: String,
    /// Expected MAC. A mismatch is a **warning**, never an error: swapping a dead
    /// node in the field means copying DIP positions, and **D08** exists so that
    /// works without editing this file.
    pub mac: Option<Mac>,
    /// Measured once per board (**D13**) — "passport, not job".
    pub crystal_ppm: Option<f64>,
    /// Physical end of the bus (**D09**). Exactly two, no more.
    #[serde(default)]
    pub terminated: bool,
    #[serde(default, rename = "input")]
    pub inputs: Vec<InputMap>,
}

/// Only if **T4** finds a drift stable enough to calibrate (**D19**).
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemperatureCorrection {
    pub mac: Mac,
    pub input: u8,
    pub ref_c: f64,
    pub us_per_c: f64,
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Corrections {
    #[serde(default, rename = "temperature")]
    temperature: Vec<TemperatureCorrection>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub venue: Venue,
    pub geometry: Geometry,
    pub margin: Margin,
    #[serde(rename = "node")]
    pub nodes: Vec<Node>,
    #[serde(default)]
    correction: Corrections,
}

/// A beam's location on the bus, as the master needs it: which address, which
/// input, which lane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BeamSite {
    pub address: u8,
    pub input: u8,
    pub lane: Lane,
    pub beam: Beam,
}

impl Mapping {
    pub fn parse(text: &str) -> Result<Mapping, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn temperature_corrections(&self) -> &[TemperatureCorrection] {
        &self.correction.temperature
    }

    pub fn declared_lanes(&self) -> impl Iterator<Item = Lane> + '_ {
        Lane::ALL
            .into_iter()
            .filter(move |l| l.number() <= self.venue.lanes)
    }

    pub fn node(&self, address: u8) -> Option<&Node> {
        self.nodes.iter().find(|n| n.address == address)
    }

    /// Every mapped beam, flattened. Inputs whose `lane` is not 1 or 2 are left
    /// out here and reported as a structural error by validation instead — this
    /// iterator is for consumers that have already checked.
    pub fn sites(&self) -> impl Iterator<Item = BeamSite> + '_ {
        self.nodes.iter().flat_map(|n| {
            n.inputs.iter().filter_map(move |i| {
                Some(BeamSite {
                    address: n.address,
                    input: i.index,
                    lane: i.lane()?,
                    beam: i.beam,
                })
            })
        })
    }

    /// Where a given beam lives for a given lane, if it is mapped at all.
    pub fn site(&self, lane: Lane, beam: Beam) -> Option<BeamSite> {
        self.sites().find(|s| s.lane == lane && s.beam == beam)
    }

    /// Beams grouped by `(lane, beam)`, which is how duplicates surface.
    pub(crate) fn by_meaning(&self) -> BTreeMap<(u8, Beam), Vec<BeamSite>> {
        let mut out: BTreeMap<(u8, Beam), Vec<BeamSite>> = BTreeMap::new();
        for s in self.sites() {
            out.entry((s.lane.number(), s.beam)).or_default().push(s);
        }
        out
    }
}
