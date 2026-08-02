//! What a scenario says, and why it says it in these terms.
//!
//! A scenario states **ground truth**: this car reacted in 0.520 s, crossed 60 ft
//! at 1.632 s, tripped the finish beam at 10.412 s. The simulator turns that into
//! register values and the master has to recover the same numbers.
//!
//! It deliberately does **not** model acceleration. A physics model here would
//! turn every test into "the master agrees with my integrator", which proves
//! nothing about the master. Stating the splits directly makes the assertion the
//! one that matters: the numbers out are the numbers in.
//!
//! Randomness comes from `seed` and nowhere else. AutoStart's delay is drawn from
//! it, so a session replays identically — without that, **D26**'s "here is the
//! session, replay it, get the same ET" dies on the first round.

use std::collections::BTreeMap;

use beam402_mapping::Beam;
use serde::Deserialize;

pub const TICK_HZ: u64 = 80_000_000;

pub fn ticks(seconds: f64) -> u64 {
    (seconds * TICK_HZ as f64).round() as u64
}

pub fn seconds(ticks: u64) -> f64 {
    ticks as f64 / TICK_HZ as f64
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 500 ms cascade.
    Standard,
    /// 400 ms, all three ambers together.
    Pro,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSetup {
    pub address: u8,
    pub mode: Mode,
    /// Upper bound on AutoStart's delay after both cars stage. The actual delay
    /// is drawn from `seed`, so it is unpredictable to a driver and identical on
    /// replay.
    pub random_delay_ms: u16,
    /// When the sequence is armed. The simulator issues `tree_arm` at this
    /// instant **the way a master would** — through the command block — so the
    /// path under test is the real one. Once the master exists it arms instead,
    /// and this becomes the default for scenarios that have no master.
    pub arm_at_s: f64,
    /// Milliseconds each lane's cascade is held back, written with
    /// `tree_handicap` before the arm. Both zero is a heads-up start.
    ///
    /// The scenario states what the tree was **told**, not what a master would
    /// compute from two dial-ins: deriving it here would test the race logic
    /// against itself. The two meet in the tests, where a `Pairing`'s computed
    /// handicap has to equal the number the tree actually ran.
    #[serde(default)]
    pub handicap_ms: [u16; 2],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Car {
    pub lane: u8,
    /// When this car finishes staging: it breaks pre-stage, then stage, and sits
    /// in both until it launches.
    pub stage_at_s: f64,
    /// Relative to green. **Negative is a red light** — the driver left early,
    /// and it is not a special case anywhere downstream.
    pub reaction_s: f64,
    /// Seconds from launch to each beam. Beam names are the closed set from the
    /// mapping file, so a typo here fails to load rather than silently dropping
    /// a split.
    #[serde(default)]
    pub splits: BTreeMap<Beam, f64>,
}

/// The ugly runs. **D26** is blunt that these are the specification and not a
/// wish list: a simulator that replays only clean runs validates nothing.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fault {
    /// The node stops answering. Every read against it times out from `from_s`.
    Silent { address: u8, from_s: f64 },
    /// The start pulse is the wrong width. **D16**: the counter has already been
    /// running for 5 ms when this is discovered.
    BadPulseWidth { lane: u8, width_us: u16 },
    /// The node reboots, possibly mid-run. `boot_count` moves and generations go
    /// back to "no run since boot", which must invalidate whatever the master
    /// was holding.
    Reboot { address: u8, at_s: f64 },
    /// A beam breaks and never makes again — a bag, a cone, a dead emitter.
    BeamStuck { address: u8, input: u8 },
    /// The capture hardware could not keep up. Arrives as an injected fault
    /// rather than being inferred, because what triggers it on silicon is
    /// `software.md` §8 #6 and nothing has run yet.
    CaptureOverrun { address: u8, lane: u8 },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub name: String,
    pub seed: u64,
    /// How long a tire keeps a beam broken. Affects `t_make` only, which nothing
    /// asserts yet — **T2** is the measurement that will care about the asymmetry
    /// between make and break, and it happens on a bench, not here.
    #[serde(default = "default_chord_ms")]
    pub chord_ms: f64,
}

fn default_chord_ms() -> f64 {
    20.6
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub scenario: Header,
    pub tree: TreeSetup,
    #[serde(rename = "car")]
    pub cars: Vec<Car>,
    #[serde(default, rename = "fault")]
    pub faults: Vec<Fault>,
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Scenario, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn car(&self, lane: u8) -> Option<&Car> {
        self.cars.iter().find(|c| c.lane == lane)
    }
}

/// SplitMix64. Small, seeded, and written out rather than pulled in, because the
/// only requirement is that the same seed gives the same sequence forever — a
/// dependency that improves its generator would break replay.
#[derive(Clone, Copy, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..bound`, or 0 for an empty bound.
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }
}
