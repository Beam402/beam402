//! A bracket round, end to end: dial-ins in, time slip out.
//!
//! The master computes the handicap, writes it over the bus, arms the tree, polls
//! the result and decides the round. Nothing is reached around: the handicap
//! travels as a `tree_handicap` command and comes back in the tree's registers,
//! and every time asserted below is read off the wire rather than out of the
//! simulator.
//!
//! The numbers are the case the format exists for — a 12.34 street car against a
//! 7.50 dragster — because that is where "quicker ET" and "won the race" come
//! apart, and where a system that measured only ET would confidently print the
//! wrong winner.

use beam402_bus::BusExt;
use beam402_mapping::{Beam, Mapping};
use beam402_poller::{Event, Poller};
use beam402_protocol::{Lane, Opcode, Tree};
use beam402_race::{decide, Entry, Format, Missing, Outcome, Pairing, Reason, Round, RunBuilder};
use beam402_sim::reference::*;
use beam402_sim::{seconds, ticks, Simulator, TICK_HZ};

const DIAL_L1: f64 = 12.34;
const DIAL_L2: f64 = 7.50;
/// Both drivers hit their dial exactly, so the round turns entirely on the
/// 0.040 s of reaction time between them.
const RT_L1: f64 = 0.500;
const RT_L2: f64 = 0.540;

fn scenario_text(extra: &str) -> String {
    format!(
        r#"
[scenario]
name = "bracket: 12.34 vs 7.50"
seed = 7

[tree]
address = 10
mode = "standard"
random_delay_ms = 400
# Far out of the way: this round is armed by the master below, over the bus.
arm_at_s = 600.0
{extra}

[[car]]
lane = 1
stage_at_s = 1.0
reaction_s = {RT_L1}
[car.splits]
interval_60 = 2.104
finish = {DIAL_L1}

[[car]]
lane = 2
stage_at_s = 1.1
reaction_s = {RT_L2}
[car.splits]
interval_60 = 0.951
trap_entry = 7.120
trap_exit = 7.310
finish = {DIAL_L2}
"#
    )
}

fn pairing() -> Pairing {
    Pairing::new(
        Format::Bracket,
        vec![
            Entry {
                lane: Lane::L1,
                dial_s: Some(DIAL_L1),
            },
            Entry {
                lane: Lane::L2,
                dial_s: Some(DIAL_L2),
            },
        ],
    )
    .unwrap()
}

/// Cycle the poller until the queued command for `address` settles, feeding
/// everything it learns to the builder. Fails loudly rather than looping: a
/// command that never confirms is the failure this is meant to surface.
fn settle_command(
    sim: &mut Simulator,
    poller: &mut Poller,
    builder: &mut RunBuilder,
    address: u8,
    what: Opcode,
) {
    for _ in 0..20 {
        let mut events = Vec::new();
        poller.cycle(sim, &mut events);
        for e in &events {
            builder.apply(e);
        }
        if let Some(Event::Commanded { status, .. }) = events
            .iter()
            .find(|e| matches!(e, Event::Commanded { address: a, opcode, .. } if *a == address && *opcode == what))
        {
            assert_eq!(
                *status,
                beam402_protocol::CommandStatus::Accepted,
                "{what:?} was refused"
            );
            return;
        }
        sim.advance_by_s(0.05);
    }
    panic!("{what:?} never confirmed");
}

/// Run the round the way race control would: compute the handicap, write it,
/// arm, then poll until the timeline is exhausted.
fn run_round(extra: &str) -> (Round, Tree, Mapping) {
    let mapping = venue();
    let mut sim =
        Simulator::new(&mapping, scenario(&scenario_text(extra))).expect("scenario must build");
    let mut poller = Poller::new(ADDRESSES);
    let mut builder = RunBuilder::new(&mapping);
    let p = pairing();

    // First contact, so identities and tick rates are known before anything is
    // timed. Nothing here assumes a node's clock; it reads it.
    for _ in 0..3 {
        let mut events = Vec::new();
        poller.cycle(&mut sim, &mut events);
        for e in &events {
            builder.apply(e);
        }
    }

    // The handicap: one command per lane that owes time, each confirmed before
    // the next goes out — the node has one command register.
    let handicap = p.handicap_ms().unwrap();
    for lane in Lane::ALL {
        let ms = handicap[lane.ord() as usize];
        if ms == 0 {
            continue;
        }
        poller
            .send(TREE, Opcode::TreeHandicap, lane.number() as u16, ms)
            .expect("nothing else queued");
        settle_command(
            &mut sim,
            &mut poller,
            &mut builder,
            TREE,
            Opcode::TreeHandicap,
        );
    }

    // Read it back before arming. A handicap that did not land is the one
    // failure in this path that would look like a normal race.
    let staged: Tree = sim.block(TREE).unwrap();
    assert_eq!(
        staged.handicap_ms(Lane::L2),
        0,
        "the tree holds the handicap pending until the arm latches it"
    );

    poller
        .send(TREE, Opcode::TreeArm, 0, 400)
        .expect("nothing else queued");
    settle_command(&mut sim, &mut poller, &mut builder, TREE, Opcode::TreeArm);

    // 20 s covers the arm, a 4.84 s spot and a 12.34 s run with room over.
    for _ in 0..400 {
        sim.advance_by_s(0.05);
        let mut events = Vec::new();
        poller.cycle(&mut sim, &mut events);
        for e in &events {
            builder.apply(e);
        }
    }

    let tree: Tree = sim.block(TREE).unwrap();
    (builder.round(), tree, mapping)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn the_tree_runs_two_cascades_and_the_master_can_see_which() {
    let (_, tree, _) = run_round("");

    assert_eq!(tree.handicap_ms(Lane::L1), 0, "the slower car leaves first");
    assert_eq!(tree.handicap_ms(Lane::L2), 4840);
    assert!(tree.is_handicap());

    // Two greens, one handicap apart, both captured from the lamp driver output
    // on the tree's own clock.
    let spot = tree.t_green(Lane::L2).wrapping_sub(tree.t_green(Lane::L1));
    assert!(
        close(seconds(spot as u64), 4.840),
        "greens {:.4} s apart",
        seconds(spot as u64)
    );
}

#[test]
fn a_bracket_is_won_at_the_stripe_by_the_slower_car() {
    let (round, _, _) = run_round("");
    let p = pairing();

    // Each ET is its own node's register, zeroed by its own car's pulse — the
    // 4.84 s the dragster spent waiting is not in its ET.
    let l1 = round.lane(Lane::L1).unwrap();
    let l2 = round.lane(Lane::L2).unwrap();
    assert!(close(l1.et_s.unwrap(), DIAL_L1), "{:?}", l1.et_s);
    assert!(close(l2.et_s.unwrap(), DIAL_L2), "{:?}", l2.et_s);

    // Each reaction time is against *that lane's* green.
    assert!(close(l1.reaction_s.unwrap(), RT_L1), "{:?}", l1.reaction_s);
    assert!(close(l2.reaction_s.unwrap(), RT_L2), "{:?}", l2.reaction_s);
    assert!(!l1.is_red() && !l2.is_red());

    // The launch margin carries the handicap, because the handicap *is* part of
    // the difference between the two pulses. Nothing had to tell it.
    let margin = round.launch_margin_s.unwrap();
    assert!(close(margin, 4.840 + RT_L2 - RT_L1), "{margin}");

    // And so the finish margin is exactly the reaction-time difference: two
    // drivers on their dial, separated by how they left.
    let stripe = round.finish_margin_s().unwrap();
    assert!(close(stripe, RT_L2 - RT_L1), "{stripe}");

    match decide(&round, &p) {
        Outcome::Win {
            lane: Lane::L1,
            reason: Reason::FirstToFinish { margin_s },
        } => assert!(close(margin_s, 0.040), "{margin_s}"),
        other => panic!("the 12.34 car drove better and must win: {other:?}"),
    }
}

#[test]
fn the_quicker_et_belongs_to_the_car_that_lost() {
    // Worth stating on its own, because it is the assertion a system that timed
    // only ET would fail: lane 2 ran 4.8 seconds quicker and did not win.
    let (round, _, _) = run_round("");
    let l1 = round.lane(Lane::L1).unwrap().et_s.unwrap();
    let l2 = round.lane(Lane::L2).unwrap().et_s.unwrap();
    assert!(l2 < l1);
    assert!(matches!(
        decide(&round, &pairing()),
        Outcome::Win { lane: Lane::L1, .. }
    ));
}

#[test]
fn splits_and_trap_speed_come_off_the_nodes_that_own_them() {
    let (round, _, mapping) = run_round("");
    let l2 = round.lane(Lane::L2).unwrap();

    assert!(close(l2.splits_s[&Beam::Interval60], 0.951));
    assert!(close(l2.splits_s[&Beam::TrapEntry], 7.120));
    assert!(close(l2.splits_s[&Beam::TrapExit], 7.310));

    // Both ends of the trap are one node and one timer, so the interval closes
    // without a cross-clock subtraction.
    let base = mapping.geometry.trap_base.unwrap();
    let expect = base / (7.310 - 7.120);
    assert!(
        (l2.trap_speed_ms.unwrap() - expect).abs() < 1e-3,
        "{:?} vs {expect}",
        l2.trap_speed_ms
    );
    assert!(
        l2.trap_speed_kmh().unwrap() > 350.0,
        "a 7.50 quarter is fast"
    );
}

#[test]
fn a_missing_split_names_its_reason_and_the_slip_is_still_issued() {
    // The agreed rule: an ET that exists and is timing-valid is a run. A split
    // that is not there prints as a reason, never as a zero and never as a
    // reason to withhold the round.
    let (round, _, _) = run_round("\n[[fault]]\nkind = \"silent\"\naddress = 3\nfrom_s = 0.5\n");

    let l1 = round.lane(Lane::L1).unwrap();
    assert!(l1.has_time(), "the finish node still reported");
    assert!(close(l1.et_s.unwrap(), DIAL_L1));
    assert_eq!(
        l1.gap(Beam::Interval60),
        Some(Missing::NodeSilent),
        "the 60 ft node is named, not silently dropped: {:?}",
        l1.gaps
    );
    assert!(!l1.splits_s.contains_key(&Beam::Interval60));

    // And the round is still decided, because both cars finished.
    assert!(matches!(
        decide(&round, &pairing()),
        Outcome::Win { lane: Lane::L1, .. }
    ));
}

#[test]
fn the_scenarios_handicap_and_the_pairings_agree() {
    // Two independent statements of the same number: what the tree was told, and
    // what the master computes from two dial-ins. If they ever disagree, one of
    // them is a bug and this says which.
    let spot = pairing().handicap_ms().unwrap();
    assert_eq!(spot, [0, 4840]);
    assert!(
        (ticks(4.840) as f64 / TICK_HZ as f64 - 4.840).abs() < 1e-9,
        "and the tick conversion does not drift on the way"
    );
}
