//! The poller against a bus that behaves badly on purpose.
//!
//! Every assertion here is about *traffic* or *events* — never about the
//! simulator's internals. What is being tested is the claim `software.md` §4
//! makes and cannot check: that a steady-state cycle is one four-register read
//! per device, that results are still recovered in full, and that the quiet
//! window is genuinely quiet.

use beam402_bus::{Bus, BusError};
use beam402_poller::{CycleStats, Event, Phase, Poller, ResetEvidence};
use beam402_protocol::map::LINK;
use beam402_protocol::{
    Block, CommandStatus, Digest, Generation, Identity, Lane, Opcode, RunRecord, Telemetry, Ticks,
};
use beam402_sim::reference::*;
use beam402_sim::{ticks, Simulator};

// ---------------------------------------------------------------------------
// A bus that keeps the receipts
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Txn {
    address: u8,
    reg: u16,
    len: u16,
    write: bool,
}

struct Tapped<B> {
    inner: B,
    log: Vec<Txn>,
    /// Rewrite `identity.protocol_version` on the way past, for the one thing the
    /// simulator cannot be asked to do: answer a contract this build predates.
    fake_version: Option<(u8, u16)>,
}

impl<B: Bus> Tapped<B> {
    fn new(inner: B) -> Self {
        Tapped {
            inner,
            log: Vec::new(),
            fake_version: None,
        }
    }

    fn take(&mut self) -> Vec<Txn> {
        std::mem::take(&mut self.log)
    }

    fn reads_of(&self, block: &str) -> Vec<Txn> {
        let desc = beam402_protocol::map::block(block).expect("known block");
        self.log
            .iter()
            .copied()
            .filter(|t| !t.write && desc.addrs.contains(&t.reg))
            .collect()
    }
}

impl<B: Bus> Bus for Tapped<B> {
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError> {
        self.log.push(Txn {
            address,
            reg,
            len: out.len() as u16,
            write: false,
        });
        self.inner.read(address, reg, out)?;
        if let Some((faked, version)) = self.fake_version {
            if address == faked && reg == Identity::ADDR {
                out[0] = version;
            }
        }
        Ok(())
    }

    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError> {
        self.log.push(Txn {
            address,
            reg,
            len: values.len() as u16,
            write: true,
        });
        self.inner.write(address, reg, values)
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn bus(text: &str) -> Tapped<Simulator> {
    Tapped::new(Simulator::new(&venue(), scenario(text)).expect("scenario must build"))
}

fn poller() -> Poller {
    Poller::new(ADDRESSES)
}

/// Cycle until first contact has settled: identity read, digests adopted.
fn settle(p: &mut Poller, b: &mut Tapped<Simulator>) -> Vec<Event> {
    let mut out = Vec::new();
    for _ in 0..3 {
        p.cycle(b, &mut out);
    }
    b.take();
    out
}

fn runs(events: &[Event]) -> Vec<(u8, Lane, RunRecord)> {
    events
        .iter()
        .filter_map(|e| match *e {
            Event::Run {
                address,
                lane,
                record,
            } => Some((address, lane, record)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The steady state, priced
// ---------------------------------------------------------------------------

#[test]
fn a_live_cycle_is_one_digest_per_device() {
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    let stats = p.cycle(&mut b, &mut out);
    let log = b.take();

    let digests: Vec<_> = log
        .iter()
        .filter(|t| t.reg == Digest::ADDR && !t.write)
        .collect();
    assert_eq!(
        digests.len(),
        ADDRESSES.len(),
        "one digest per device and no more: {log:?}"
    );
    // Plus at most one rotation read. Nothing else may ride the steady state.
    assert!(
        log.len() <= ADDRESSES.len() + 2,
        "the quiet loop grew extra traffic: {log:?}"
    );
    assert_eq!(stats.reads, log.len() as u32);
}

/// Per-device cost of the four-register digest, arithmetic rather than
/// measurement: one FC3 asks in 8 characters and answers in 13, and must be
/// preceded by 3.5 characters of silence. 24.5 characters is 12.76 ms at 19,200
/// bps 8N1.
const DIGEST_MS: f64 = 12.76;

#[test]
fn a_digest_sweep_costs_what_the_arithmetic_says() {
    // `software.md` §4 priced this row at "~13 chars, ~7 ms/device", which counted
    // the response frame and forgot both the request and the inter-frame silence.
    // The true figure is ~1.8x that, and the numbers downstream of it — cycle
    // time, staging-lamp latency — move with it. Pinned here so the document
    // cannot drift back.
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);

    let digests = b.reads_of(Digest::NAME).len() as u32;
    assert_eq!(digests, ADDRESSES.len() as u32, "one per device");
    let sweep = CycleStats {
        reads: digests,
        registers: digests * Digest::LEN as u32,
        ..Default::default()
    };
    let per_device = sweep.millis() / digests as f64;
    assert!(
        (per_device - DIGEST_MS).abs() < 0.05,
        "a digest costs {per_device:.2} ms at {} bps, expected {DIGEST_MS}",
        LINK.baud
    );
    // Seven devices is `architecture.md` §12's final shape, and the figure that
    // sizes liveness detection and staging-lamp response.
    assert!(
        (7.0 * DIGEST_MS - 89.3).abs() < 0.5,
        "seven devices is {:.1} ms of digest, not the 50 ms once claimed",
        7.0 * DIGEST_MS
    );
}

#[test]
fn a_steady_state_cycle_stays_inside_the_response_timeout() {
    // The loop must not be slower than the timeout that guards it, or a healthy
    // cycle becomes indistinguishable from a stalled one.
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    let mut worst: f64 = 0.0;
    for _ in 0..ADDRESSES.len() * 2 {
        worst = worst.max(p.cycle(&mut b, &mut out).millis());
    }
    assert!(
        worst < 150.0,
        "the worst steady-state cycle costs {worst:.1} ms at {} bps",
        LINK.baud
    );
}

#[test]
fn one_silent_node_costs_more_than_the_entire_healthy_cycle() {
    // Worth knowing before a parking lot teaches it: a dead node is not a gap in
    // the panel, it is 300 ms of bus time every cycle — timeout times retries —
    // which is six times what the whole healthy sweep costs. Any future "why did
    // the lamps get sluggish" starts here.
    let text = clean_pair().replace(
        "[[car]]",
        "[[fault]]\nkind = \"silent\"\naddress = 3\nfrom_s = 0.0\n\n[[car]]",
    );
    let mut b = bus(&text);
    let mut p = poller();
    b.inner.advance_to(ticks(0.5));
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    let stats = p.cycle(&mut b, &mut out);
    assert_eq!(stats.timeouts, 1);
    assert!(
        stats.timeout_millis() >= 300.0,
        "one silent node costs {:.0} ms",
        stats.timeout_millis()
    );
    assert!(
        stats.timeout_millis() > stats.millis() - stats.timeout_millis(),
        "silence should dominate the cycle, and does"
    );
}

// ---------------------------------------------------------------------------
// Read on change
// ---------------------------------------------------------------------------

#[test]
fn a_run_arrives_once_and_in_one_transaction() {
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    b.inner.run();
    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);

    // protocol.md §2: a lane's record is one FC3 of exactly 28 registers, so it
    // cannot be assembled from two reads that straddle a new run.
    let records = b.reads_of(RunRecord::NAME);
    assert!(!records.is_empty(), "the run must have been fetched");
    for t in &records {
        assert_eq!(t.len, RunRecord::LEN, "a record was split: {t:?}");
        assert!(t.reg == RunRecord::addr(Lane::L1) || t.reg == RunRecord::addr(Lane::L2));
    }

    let fetched = runs(&out);
    assert_eq!(
        fetched.len(),
        records.len(),
        "every record read should surface as an event"
    );

    // ET is the finish node's own register, read straight off the wire.
    let (_, _, finish_l1) = fetched
        .iter()
        .copied()
        .find(|(a, l, _)| *a == FINISH && *l == Lane::L1)
        .expect("lane 1 finished");
    assert_eq!(
        finish_l1.inputs[0].break_at(),
        Some(Ticks(ticks(ET1) as u32))
    );

    // And nothing is read twice: the generation has not moved since.
    b.take();
    let mut again = Vec::new();
    p.cycle(&mut b, &mut again);
    assert!(
        b.reads_of(RunRecord::NAME).is_empty(),
        "a record was re-read with no generation change"
    );
    assert!(runs(&again).is_empty());
}

#[test]
fn the_pulse_rides_along_with_the_run_it_belongs_to() {
    // The map schedules the pulse block on generation change, and no digest bit
    // carries its generation — but the pulse is what advances the run generation
    // in the first place, so the digest's change is its change signal too.
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);
    b.inner.run();

    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);

    let margin = out
        .iter()
        .find_map(|e| match e {
            Event::Pulse {
                address,
                observation,
            } if *address == START_L1 => Some(observation.launch_margin()),
            _ => None,
        })
        .expect("the margin source must have reported its pulse view");
    assert!(
        margin.is_some(),
        "both pulses were seen, so D20's first term is available"
    );
}

#[test]
fn first_contact_adopts_generations_instead_of_inventing_news() {
    // A master that starts after a run must not announce it as one. The result is
    // not lost — it is latched, and `refetch` is how it is recovered.
    let mut b = bus(&clean_pair());
    b.inner.run();

    let mut p = poller();
    let mut out = Vec::new();
    for _ in 0..3 {
        p.cycle(&mut b, &mut out);
    }
    assert!(
        runs(&out).is_empty(),
        "arriving is not a change: {:?}",
        runs(&out)
    );
    assert!(out
        .iter()
        .any(|e| matches!(e, Event::Identified { address, .. } if *address == FINISH)));

    out.clear();
    p.refetch(FINISH);
    p.cycle(&mut b, &mut out);
    assert_eq!(runs(&out).len(), 2, "both lanes on request");
}

// ---------------------------------------------------------------------------
// The quiet window
// ---------------------------------------------------------------------------

#[test]
fn the_quiet_window_puts_nothing_at_all_on_the_wire() {
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    p.set_phase(Phase::Quiet);
    let mut out = Vec::new();
    for _ in 0..5 {
        let stats = p.cycle(&mut b, &mut out);
        assert_eq!(stats, Default::default());
    }
    b.inner.run();
    for _ in 0..5 {
        p.cycle(&mut b, &mut out);
    }
    assert!(
        b.take().is_empty(),
        "the master transmitted during the launch"
    );
    assert!(out.is_empty());

    // Lifting it loses nothing: everything latched.
    p.set_phase(Phase::Live);
    p.cycle(&mut b, &mut out);
    assert!(
        !runs(&out).is_empty(),
        "the results were still there afterwards, which is the whole point of D25"
    );
}

#[test]
fn the_rotations_wait_until_the_records_are_in() {
    // §4: telemetry must not compete with results for the bus in the seconds
    // after a run.
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);
    p.set_phase(Phase::Quiet);
    b.inner.run();
    p.set_phase(Phase::Live);

    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);
    assert!(
        b.reads_of(Telemetry::NAME).is_empty(),
        "telemetry rode along with the collect cycle"
    );
    assert!(!runs(&out).is_empty());

    b.take();
    p.cycle(&mut b, &mut out);
    assert_eq!(
        b.reads_of(Telemetry::NAME).len(),
        1,
        "and resumes once nothing is outstanding"
    );
}

#[test]
fn telemetry_rotates_one_device_per_cycle() {
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..ADDRESSES.len() {
        b.take();
        p.cycle(&mut b, &mut out);
        let t = b.reads_of(Telemetry::NAME);
        assert_eq!(t.len(), 1, "exactly one device per cycle");
        seen.insert(t[0].address);
    }
    assert_eq!(
        seen.len(),
        ADDRESSES.len(),
        "one full sweep covers every device"
    );
}

// ---------------------------------------------------------------------------
// Things going wrong
// ---------------------------------------------------------------------------

#[test]
fn a_reboot_is_caught_by_the_digest_not_by_the_slow_rotation() {
    // The prompt evidence is a generation returning to NEVER, which rides in the
    // four registers read every cycle. `boot_count` confirms it later.
    let text = clean_pair().replace(
        "[[car]]",
        "[[fault]]\nkind = \"reboot\"\naddress = 6\nat_s = 12.0\n\n[[car]]",
    );
    let mut b = bus(&text);
    let mut p = poller();
    settle(&mut p, &mut b);

    // Let the run land so the finish node is holding generations, then reboot it.
    b.inner.advance_to(ticks(11.0));
    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);
    assert!(!runs(&out).is_empty(), "the run was recorded first");

    out.clear();
    b.inner.run();
    p.cycle(&mut b, &mut out);

    assert!(
        out.iter().any(|e| matches!(
            e,
            Event::Reset {
                address: 6,
                evidence: ResetEvidence::GenerationCleared
            }
        )),
        "a restart must invalidate what the master holds: {out:?}"
    );
    assert!(
        out.iter()
            .any(|e| matches!(e, Event::Identified { address: 6, .. })),
        "and identity is re-read, in case it came back on different firmware"
    );
    assert_eq!(
        p.device(FINISH).unwrap().digest.unwrap().run_gen(Lane::L1),
        Generation::NEVER
    );
}

#[test]
fn a_silent_node_is_announced_once_and_its_return_once() {
    let text = clean_pair().replace(
        "[[car]]",
        "[[fault]]\nkind = \"silent\"\naddress = 4\nfrom_s = 5.0\n\n[[car]]",
    );
    let mut b = bus(&text);
    let mut p = poller();
    settle(&mut p, &mut b);

    let mut out = Vec::new();
    b.inner.advance_to(ticks(6.0));
    for _ in 0..6 {
        p.cycle(&mut b, &mut out);
    }
    let silences = out
        .iter()
        .filter(|e| matches!(e, Event::Silent { address: 4, .. }))
        .count();
    assert_eq!(silences, 1, "a dead node is news once, not every cycle");

    // One lost frame must not do it — the transport already retried inside that
    // one call.
    assert!(
        beam402_poller::Config::default().misses_before_silent >= 2,
        "a single frame lost to ignition noise must not blank the panel"
    );
}

#[test]
fn a_node_on_an_unknown_contract_is_refused_not_guessed() {
    let mut b = bus(&clean_pair());
    b.fake_version = Some((SIXTY, 99));
    let mut p = poller();
    let mut out = Vec::new();
    for _ in 0..2 {
        p.cycle(&mut b, &mut out);
    }

    assert!(out.iter().any(|e| matches!(
        e,
        Event::Unsupported {
            address: 3,
            protocol_version: 99
        }
    )));
    assert!(!p.device(SIXTY).unwrap().usable);
    assert!(
        !p.device(SIXTY).unwrap().silent,
        "unusable is not absent — it stays on the panel"
    );

    // And it is never read for timing, no matter what its generations do.
    b.inner.run();
    b.take();
    out.clear();
    p.cycle(&mut b, &mut out);
    assert!(b
        .reads_of(RunRecord::NAME)
        .iter()
        .all(|t| t.address != SIXTY));
    assert!(runs(&out).iter().all(|(a, _, _)| *a != SIXTY));
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[test]
fn arming_the_tree_goes_out_over_the_seam_and_comes_back_confirmed() {
    // The master arms, the tree runs it (§5). This is the whole path: a write
    // through `Bus`, a sequence machine that reacts to it, and a confirmation
    // read — no simulator side door.
    let text = clean_pair().replace("arm_at_s = 3.0", "arm_at_s = 600.0");
    let mut b = bus(&text);
    let mut p = poller();
    settle(&mut p, &mut b);
    b.inner.advance_to(ticks(2.0));

    let seq = p
        .send(TREE, Opcode::TreeArm, 0, 700)
        .expect("nothing queued");
    assert_eq!(p.send(TREE, Opcode::TreeAbort, 0, 0), None, "one at a time");

    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);

    assert!(
        out.iter().any(|e| matches!(
            e,
            Event::Commanded {
                address: 10,
                opcode: Opcode::TreeArm,
                status: CommandStatus::Accepted
            }
        )),
        "confirmed by a read of command_seq_echo, not by the write returning: {out:?}"
    );
    assert_eq!(seq, 1);

    // The tree is now sequencing, and the poller is watching the block that says
    // so — nothing in the digest would have told it.
    out.clear();
    for _ in 0..4 {
        b.inner.advance_by_s(0.6);
        p.cycle(&mut b, &mut out);
    }
    let greens = out
        .iter()
        .filter_map(|e| match e {
            Event::Tree { tree, .. } => Some(tree.reaction_time(Lane::L1)),
            _ => None,
        })
        .count();
    assert!(greens > 0, "the tree's own block was read while it ran");
}

#[test]
fn a_command_the_device_refuses_is_reported_as_refused() {
    let mut b = bus(&clean_pair());
    let mut p = poller();
    settle(&mut p, &mut b);

    // A timing node is not a tree, and D24 says so by class, not by silence.
    p.send(SIXTY, Opcode::TreeArm, 0, 500).unwrap();
    let mut out = Vec::new();
    p.cycle(&mut b, &mut out);
    assert!(
        out.iter().any(|e| matches!(
            e,
            Event::Commanded {
                address: 3,
                status: CommandStatus::Rejected,
                ..
            }
        )),
        "{out:?}"
    );
}

#[test]
fn a_command_into_silence_is_declared_lost_and_never_repeated() {
    let text = clean_pair().replace(
        "[[car]]",
        "[[fault]]\nkind = \"silent\"\naddress = 10\nfrom_s = 0.0\n\n[[car]]",
    );
    let mut b = bus(&text);
    let mut p = poller();
    b.inner.advance_to(ticks(0.5));
    settle(&mut p, &mut b);

    p.send(TREE, Opcode::TreeArm, 0, 500).unwrap();
    let mut out = Vec::new();
    for _ in 0..12 {
        p.cycle(&mut b, &mut out);
    }
    assert!(
        out.iter().any(|e| matches!(
            e,
            Event::CommandLost {
                address: 10,
                opcode: Opcode::TreeArm
            }
        )),
        "{out:?}"
    );
    let writes: Vec<_> = b.log.iter().filter(|t| t.write).collect();
    assert_eq!(
        writes.len(),
        1,
        "a write that may already have run is not repeated behind the operator's back"
    );
}
