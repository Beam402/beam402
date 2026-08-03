//! The loop: poll, decide, act. The only impure thing in race control.
//!
//! Everything it does is either a [`Poller`] call or a [`Staging`] action, and
//! that is the point of the shape. The staging machine decides *when* to arm,
//! when to go quiet, when to collect and when to give up; this module knows how
//! to say those things to a bus and nothing else. Swap the simulator for a
//! serial port and not a line here changes.
//!
//! Time is stepped rather than slept, so a round replays identically whether the
//! bus underneath is a simulator running on virtual time or a real trunk.

use beam402_bus::{Bus, Paced};
use beam402_mapping::Mapping;
use beam402_poller::{Event, Phase as BusPhase, Poller};
use beam402_protocol::{Lane, Opcode};
use beam402_race::staging::{Action, Blocked, Config, Phase, Staging};
use beam402_race::{Pairing, Round, RunBuilder};

/// How long one pass of the loop stands for. The poll cycle itself costs ~90 ms
/// on a seven-device bus (`software.md` §4), so this is the honest granularity
/// rather than a number chosen to make a test fast.
pub const STEP_MS: u64 = 100;

/// Somebody watching the loop from outside it.
///
/// `beam402 scope` is the reason this exists, and the reason it is a trait
/// rather than a flag: the loop must not learn that anyone is looking. A round
/// that runs differently when observed is not the round the recording is of.
pub trait Watch {
    fn frame(&mut self, _frame: Frame<'_>) {}
}

/// The unwatched case, and the default everywhere else.
impl Watch for () {}

/// One pass of the loop, as an observer sees it.
pub struct Frame<'a> {
    pub t_ms: u64,
    pub phase: Phase,
    pub positions: [beam402_race::Position; 2],
    pub lamps: beam402_protocol::LampFlags,
    pub events: &'a [Event],
    pub poller: &'a Poller,
    pub bus_ms: f64,
}

pub struct Report {
    pub round: Round,
    pub blocked: Option<Blocked>,
    pub abandoned: bool,
    pub cycles: u32,
    pub bus_ms: f64,
}

/// Run one round to completion.
///
/// Generic over the bus, which is the whole point: the simulator, a recorded
/// session and the serial port that does not exist yet all arrive here as the
/// same two traits, and a replayed round therefore runs the real poller and the
/// real staging machine rather than a harness that agrees with itself.
pub fn run<B: Bus + Paced>(
    mapping: &Mapping,
    sim: &mut B,
    addresses: &[u8],
    tree_address: u8,
    pairing: &Pairing,
    cfg: Config,
) -> Result<Report, String> {
    run_watched(mapping, sim, addresses, tree_address, pairing, cfg, &mut ())
}

/// The same round, with somebody looking.
#[allow(clippy::too_many_arguments)]
pub fn run_watched<B: Bus + Paced, O: Watch>(
    mapping: &Mapping,
    sim: &mut B,
    addresses: &[u8],
    tree_address: u8,
    pairing: &Pairing,
    cfg: Config,
    watch: &mut O,
) -> Result<Report, String> {
    let mut poller = Poller::new(addresses.iter().copied());
    let mut builder = RunBuilder::new(mapping);
    let mut staging = Staging::with_config(mapping, cfg);
    let handicap = pairing.handicap_ms().map_err(|e| e.to_string())?;

    let mut blocked = None;
    let mut abandoned = false;
    let mut bus = beam402_poller::CycleStats::default();
    // Long enough for a settle, a cascade and a quarter mile, with a bracket's
    // spot on top; the staging machine's own timeout is what actually ends a
    // round that goes wrong.
    let deadline = 120_000 / STEP_MS;

    // Wall time as the loop actually advanced it. Counting cycles instead would
    // undercount, because arming steps the world too — and an observer drawing a
    // car against a clock that lags puts it in the wrong place.
    let mut clock_ms = 0u64;

    for cycles in 1..=deadline {
        let mut events = Vec::new();
        bus.merge(poller.cycle(sim, &mut events));

        let mut actions = Vec::new();
        for e in &events {
            builder.apply(e);
            actions.extend(staging.apply(e));
        }
        actions.extend(staging.tick(STEP_MS));

        for action in actions {
            match action {
                Action::ShowStaging(lamps) => {
                    poller.send(tree_address, Opcode::TreeStaging, lamps.bits(), 0);
                }
                Action::Arm => {
                    clock_ms += arm(&mut poller, sim, tree_address, handicap, &mut builder)?;
                    staging.armed(handicap);
                    poller.set_phase(BusPhase::Quiet);
                }
                Action::Quiet => poller.set_phase(BusPhase::Quiet),
                Action::Collect => {
                    poller.set_phase(BusPhase::Live);
                    for a in addresses {
                        poller.refetch(*a);
                    }
                }
                Action::Abandon => abandoned = true,
            }
        }

        watch.frame(Frame {
            t_ms: clock_ms,
            phase: staging.phase(),
            positions: [staging.position(Lane::L1), staging.position(Lane::L2)],
            lamps: staging.lamps(),
            events: &events,
            poller: &poller,
            bus_ms: bus.millis(),
        });

        if let Phase::Blocked(why) = staging.phase() {
            blocked = Some(why);
        }
        if staging.phase() == Phase::Complete {
            // One more pass with the bus live, so anything that landed on the
            // last cycle is read before the round is closed.
            poller.set_phase(BusPhase::Live);
            for a in addresses {
                poller.refetch(*a);
            }
            let mut last = Vec::new();
            bus.merge(poller.cycle(sim, &mut last));
            for e in &last {
                builder.apply(e);
            }
            poller.release_tree(tree_address);
            // The round is over; say so in a frame, or the last thing an observer
            // ever sees is a round still running.
            watch.frame(Frame {
                t_ms: clock_ms,
                phase: staging.phase(),
                positions: [staging.position(Lane::L1), staging.position(Lane::L2)],
                lamps: staging.lamps(),
                events: &last,
                poller: &poller,
                bus_ms: bus.millis(),
            });
            return Ok(Report {
                round: builder.round(),
                blocked,
                abandoned,
                cycles: cycles as u32,
                bus_ms: bus.millis(),
            });
        }

        sim.advance_ms(STEP_MS);
        clock_ms += STEP_MS;
    }
    Err(format!(
        "the round never completed; staging stalled in {:?}",
        staging.phase()
    ))
}

/// The arm sequence, shared with the live runtime: every handicap first, each
/// confirmed, then `tree_arm`, which latches them.
pub fn arm<B: Bus + Paced>(
    poller: &mut Poller,
    sim: &mut B,
    tree: u8,
    handicap: [u16; 2],
    builder: &mut RunBuilder,
) -> Result<u64, String> {
    let mut advanced = 0;
    for lane in Lane::ALL {
        let ms = handicap[lane.ord() as usize];
        if ms == 0 {
            continue;
        }
        poller.send(tree, Opcode::TreeHandicap, lane.number() as u16, ms);
        advanced += confirm(poller, sim, tree, Opcode::TreeHandicap, builder)?;
    }
    poller.send(tree, Opcode::TreeArm, 0, 700);
    advanced += confirm(poller, sim, tree, Opcode::TreeArm, builder)?;
    Ok(advanced)
}

fn confirm<B: Bus + Paced>(
    poller: &mut Poller,
    sim: &mut B,
    address: u8,
    what: Opcode,
    builder: &mut RunBuilder,
) -> Result<u64, String> {
    let mut advanced = 0;
    for _ in 0..20 {
        let mut events = Vec::new();
        poller.cycle(sim, &mut events);
        for e in &events {
            builder.apply(e);
        }
        for e in &events {
            match e {
                Event::Commanded { opcode, status, .. }
                    if *opcode == what && address == e.address() =>
                {
                    return if status.is_settled()
                        && *status == beam402_protocol::CommandStatus::Accepted
                    {
                        Ok(advanced)
                    } else {
                        Err(format!("{what:?} was refused by device {address}"))
                    };
                }
                Event::CommandLost { opcode, .. } if *opcode == what => {
                    return Err(format!("{what:?} was never confirmed by device {address}"))
                }
                _ => {}
            }
        }
        sim.advance_ms(STEP_MS);
        advanced += STEP_MS;
    }
    Err(format!("{what:?} never confirmed"))
}
