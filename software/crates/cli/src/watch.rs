//! Turning a round into something you can look at.
//!
//! Two halves, and the split matters. [`Tap`] sits on the bus and writes down the
//! transactions; [`Recording`] sits beside the loop and freezes everything else
//! once per cycle. Neither changes what the round does — a round that ran
//! differently when observed would not be the round the page is of.

use std::cell::RefCell;
use std::rc::Rc;

use beam402_bus::{Bus, BusError, Paced};
use beam402_mapping::{Beam, Mapping};
use beam402_poller::Event;
use beam402_protocol::Lane;
use beam402_race::Round;
use beam402_scope::{beam_marks, block_of, Capture, Crossing, Frame, LampAt, NodeState, Txn};

use crate::round::{self, Watch};

type Shared = Rc<RefCell<Vec<Txn>>>;

/// A bus that keeps a copy of what crossed it this cycle.
pub struct Tap<B> {
    inner: B,
    seen: Shared,
}

impl<B> Tap<B> {
    pub fn new(inner: B) -> (Self, Shared) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        (
            Tap {
                inner,
                seen: Rc::clone(&seen),
            },
            seen,
        )
    }
}

impl<B: Bus> Bus for Tap<B> {
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError> {
        let r = self.inner.read(address, reg, out);
        self.seen.borrow_mut().push(Txn {
            write: false,
            address,
            block: block_of(reg),
            words: out.len() as u16,
            ok: r.is_ok(),
        });
        r
    }

    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError> {
        let r = self.inner.write(address, reg, values);
        self.seen.borrow_mut().push(Txn {
            write: true,
            address,
            block: block_of(reg),
            words: values.len() as u16,
            ok: r.is_ok(),
        });
        r
    }
}

impl<B: Paced> Paced for Tap<B> {
    fn advance_ms(&mut self, ms: u64) {
        self.inner.advance_ms(ms);
    }
}

/// The observer. Collects one [`Frame`] per cycle.
pub struct Recording {
    seen: Shared,
    addresses: Vec<u8>,
    /// Which (address, input) is each lane's finish beam.
    finish: std::collections::BTreeMap<u8, (u8, u8)>,
    pub frames: Vec<Frame>,
    /// The cycle each lane's finish crossing first arrived in.
    pub finish_seen_ms: [Option<u64>; 2],
    tree_lamps: Option<u16>,
    tree_read_ms: u64,
    /// The tree block as last read, for reconstructing the cascade afterwards.
    pub tree: Option<beam402_protocol::Tree>,
}

impl Recording {
    pub fn new(mapping: &Mapping, seen: Shared, addresses: &[u8]) -> Self {
        let finish = mapping
            .declared_lanes()
            .filter_map(|l| {
                mapping
                    .site(l, Beam::Finish)
                    .map(|s| (l.number(), (s.address, s.input)))
            })
            .collect();
        Recording {
            seen,
            addresses: addresses.to_vec(),
            finish,
            frames: Vec::new(),
            finish_seen_ms: [None; 2],
            tree_lamps: None,
            tree_read_ms: 0,
            tree: None,
        }
    }
}

impl Watch for Recording {
    fn frame(&mut self, f: round::Frame<'_>) {
        // When the finish crossing arrived. The launch is worked back from it and
        // the measured ET, because the master is silent across the launch itself
        // and never saw one happen.
        for e in f.events {
            if let Event::Run {
                address,
                lane,
                record,
            } = e
            {
                let i = lane.ord() as usize;
                let crossed = self.finish.get(&lane.number()).is_some_and(|(a, input)| {
                    a == address
                        && record
                            .inputs
                            .get(*input as usize)
                            .is_some_and(|c| c.break_at().is_some())
                });
                if crossed && self.finish_seen_ms[i].is_none() {
                    self.finish_seen_ms[i] = Some(f.t_ms);
                }
            }
        }

        for e in f.events {
            if let Event::Tree { tree, .. } = e {
                self.tree_lamps = Some(tree.lamps.bits());
                self.tree_read_ms = f.t_ms;
                self.tree = Some(*tree);
            }
        }

        let nodes = self
            .addresses
            .iter()
            .map(|a| {
                let d = f.poller.device(*a);
                NodeState {
                    address: *a,
                    inputs: d.and_then(|d| d.digest).map(|d| d.input_state).unwrap_or(0),
                    silent: d.map(|d| d.silent).unwrap_or(true),
                    identified: d.map(|d| d.identity.is_some()).unwrap_or(false),
                }
            })
            .collect();

        self.frames.push(Frame {
            t_ms: f.t_ms,
            phase: phase_name(f.phase),
            positions: [
                format!("{:?}", f.positions[0]),
                format!("{:?}", f.positions[1]),
            ]
            .map(|s| s.to_lowercase()),
            lamps: f.lamps.bits(),
            tree_lamps: self.tree_lamps,
            tree_age_ms: f.t_ms.saturating_sub(self.tree_read_ms),
            nodes,
            txns: std::mem::take(&mut self.seen.borrow_mut()),
            events: f.events.iter().map(describe).collect(),
            bus_ms: f.bus_ms,
        });
    }
}

fn phase_name(p: beam402_race::Phase) -> String {
    use beam402_race::Phase;
    match p {
        Phase::Blocked(why) => format!("blocked: {why}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// One line per event, in the register the rest of the project uses: what
/// happened, to whom, and nothing inferred.
fn describe(e: &Event) -> String {
    match e {
        Event::Identified { address, identity } => format!(
            "{address}  identified   class {:?}, {} MHz",
            identity.device_class,
            identity.tick_hz / 1_000_000
        ),
        Event::Unsupported {
            address,
            protocol_version,
        } => format!("{address}  REFUSED      protocol version {protocol_version}"),
        Event::Silent { address, error } => format!("{address}  SILENT       {error}"),
        Event::Returned { address } => format!("{address}  returned"),
        Event::Reset { address, evidence } => format!("{address}  RESET        {evidence:?}"),
        Event::Digest { address, digest } => {
            format!(
                "{address}  digest       beams {:04b}",
                digest.input_state & 0xf
            )
        }
        Event::Run {
            address,
            lane,
            record,
        } => format!(
            "{address}  run L{}       inputs {:04b}{}",
            lane.number(),
            record.input_mask & 0xf,
            if record.is_timing_valid() {
                ""
            } else {
                "  NOT VALID"
            }
        ),
        Event::Pulse { address, .. } => format!("{address}  pulse"),
        Event::Tree { address, tree } => format!("{address}  tree         {:?}", tree.state),
        Event::Telemetry { address, telemetry } => format!(
            "{address}  telemetry    {} mV, {:.1} C",
            telemetry.battery_mv,
            telemetry.temp_interior as f64 / 10.0
        ),
        Event::Status { address, status } => {
            format!("{address}  status       boot {}", status.boot_count)
        }
        Event::Commanded {
            address,
            opcode,
            status,
        } => format!("{address}  command      {opcode:?} {status:?}"),
        Event::CommandLost { address, opcode } => format!("{address}  LOST         {opcode:?}"),
        Event::ReadFailed {
            address,
            block,
            error,
        } => format!("{address}  read failed  {block}: {error}"),
    }
}

/// Every beam a car actually crossed, as a distance and a time.
///
/// Only what was measured goes in: a split with no time contributes nothing,
/// because the page draws lines between these points and a guessed point would
/// become a guessed line.
pub fn crossings(mapping: &Mapping, round: &Round) -> Vec<Crossing> {
    let marks = beam_marks(mapping);
    let mut out = Vec::new();
    for lane in mapping.declared_lanes() {
        let Some(run) = round.lane(lane) else {
            continue;
        };
        let mut add = |beam: Beam, t: f64| {
            if let Some(m) = marks
                .iter()
                .find(|m| m.lane == lane.number() && m.beam == beam.wire_name())
            {
                out.push(Crossing {
                    lane: lane.number(),
                    beam: beam.wire_name().to_string(),
                    at_m: m.at_m,
                    t_s: t,
                });
            }
        };
        for (beam, t) in &run.splits_s {
            add(*beam, *t);
        }
        if let Some(et) = run.et_s {
            add(Beam::Finish, et);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
/// Where the whole round sits on the loop's clock, and where every lamp came on.
///
/// **One** number here is approximate and everything else is a register.
///
/// The master is silent across the launch on purpose, so nothing it saw can date
/// the green; the only anchor available is the cycle a finish crossing arrived
/// in, which is quantised to a poll. Anchoring each lane on its own finish gives
/// two independently-rounded instants, and then *every* relationship between
/// them inherits the error: the greens came out 4.900 s apart when the handicap
/// register says 4.840, and splitting the difference instead moved the reaction
/// times to 0.47 and 0.57 when the slip says 0.500 and 0.540.
///
/// So a single instant is anchored — the cascade start, averaged over whatever
/// anchors exist — and the rest is derived from what the tree measured:
///
/// ```text
/// green[lane]  = base + handicap[lane]      // exact, tree register
/// launch[lane] = green[lane] + reaction     // exact, tree register
/// position     = launch[lane] + crossings   // exact, node registers
/// ```
///
/// The picture can still be shifted bodily by up to a poll cycle. Nothing inside
/// it disagrees with the slip.
fn timeline(
    round: &Round,
    tree: Option<&beam402_protocol::Tree>,
    finish_seen_ms: [Option<u64>; 2],
) -> ([Option<u64>; 2], Vec<LampAt>) {
    use beam402_protocol::TreeMode;

    let mut launch_ms = [None; 2];
    let Some(tree) = tree else {
        return (launch_ms, Vec::new());
    };

    let known = |lane: Lane| -> Option<(f64, f64, i64)> {
        let run = round.lane(lane)?;
        Some((run.et_s?, run.reaction_s?, tree.handicap_ms(lane) as i64))
    };

    let mut anchors = Vec::new();
    for lane in Lane::ALL {
        let i = lane.ord() as usize;
        if let (Some(seen), Some((et, rt, spot))) = (finish_seen_ms[i], known(lane)) {
            anchors.push(seen as i64 - ((et + rt) * 1000.0).round() as i64 - spot);
        }
    }
    if anchors.is_empty() {
        return (launch_ms, Vec::new());
    }
    let base = anchors.iter().sum::<i64>() / anchors.len() as i64;

    // Standard is three ambers half a second apart; pro flashes all three and
    // waits 0.4 s. `protocol.md` §3 fixes both.
    let ambers: [i64; 3] = match tree.mode {
        TreeMode::Pro => [-400, -400, -400],
        _ => [-1500, -1000, -500],
    };
    let at = |t: i64| t.max(0) as u64;

    let mut lamps = Vec::new();
    for lane in Lane::ALL {
        let i = lane.ord() as usize;
        let Some((_, rt, spot)) = known(lane) else {
            continue;
        };
        let green = base + spot;
        launch_ms[i] = Some(at(green + (rt * 1000.0).round() as i64));

        for (n, off) in ambers.iter().enumerate() {
            lamps.push(LampAt {
                t_ms: at(green + off),
                lane: lane.number(),
                lamp: 2 + n as u8,
            });
        }
        lamps.push(LampAt {
            t_ms: at(green),
            lane: lane.number(),
            lamp: 5,
        });
        // A red light is not a special case; it is a negative reaction time, and
        // the tree knows it at the instant its own green lights.
        if rt < 0.0 {
            lamps.push(LampAt {
                t_ms: at(green),
                lane: lane.number(),
                lamp: 6,
            });
        }
    }
    lamps.sort_by_key(|l| l.t_ms);
    (launch_ms, lamps)
}

#[allow(clippy::too_many_arguments)]
pub fn capture(
    mapping: &Mapping,
    round: &Round,
    frames: Vec<Frame>,
    finish_seen_ms: [Option<u64>; 2],
    tree: Option<&beam402_protocol::Tree>,
    slip: String,
    format: String,
    dials: Option<(f64, f64)>,
    handicap_ms: [u16; 2],
    source: String,
) -> Capture {
    let (launch_ms, lamp_at) = timeline(round, tree, finish_seen_ms);

    Capture {
        lamp_at,
        title: mapping.venue.name.clone(),
        venue: format!("{} · {} lanes", mapping.venue.name, mapping.venue.lanes),
        lanes: mapping.venue.lanes,
        finish_m: mapping.geometry.finish,
        beams: beam_marks(mapping),
        labels: mapping
            .nodes
            .iter()
            .map(|n| (n.address, n.label.clone()))
            .collect(),
        format,
        dials,
        handicap_ms,
        launch_ms,
        crossings: crossings(mapping, round),
        frames,
        slip,
        source,
    }
}
