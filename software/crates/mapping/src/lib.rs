//! # Beam402 — the venue mapping file
//!
//! **The single source of truth for what the numbers mean** (**D08**). One file
//! per venue, versioned in the club's own repository, never on a node.
//!
//! A node reports ticks against an input index. It does not know which lane it
//! serves or what a 60-foot split is — that is **D24**, and it is what keeps one
//! firmware good for every position. The meaning lives here, and only here.
//!
//! ## Loading is two steps, not one
//!
//! [`Mapping::parse`] fails on anything that makes the file uninterpretable: bad
//! TOML, an unknown key, an unknown beam meaning. Everything else is a
//! *validation* problem, collected rather than thrown, because a file with three
//! mistakes should report three.
//!
//! Validation itself splits, which [`protocol.md`] §5 does not spell out but the
//! master has to respect:
//!
//! ```text
//! check_static(&mapping)                 → decidable from the file alone
//! check_against_bus(&mapping, &facts)    → needs input_present, MACs, pulses seen
//! ```
//!
//! One rule can be decided by neither yet. Nothing in the register map publishes
//! which `(input, lane)` pairs a node can actually capture, so a lane typo cannot
//! be caught — it reads as "not seen this run", which is data, and the run
//! quietly loses a split. That is `software.md` §8 #7, and it reports as
//! **unchecked** rather than passing quietly: a rule that silently does not run
//! is worse than a rule nobody wrote.
//!
//! [`protocol.md`]: https://github.com/perfilev-dev/beam402/blob/main/docs/protocol.md

mod model;
mod validate;

pub use model::{
    Beam, BeamSite, Geometry, InputMap, Mac, Mapping, Margin, Node, TemperatureCorrection, Venue,
};
pub use validate::{check_against_bus, check_static, DeviceFacts, Problem, Report, Rule, Severity};

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_protocol::{DeviceClass, Lane};

    /// A two-lane venue that passes every static rule. Tests state their
    /// deviation from this rather than restating a whole file.
    fn valid() -> String {
        venue(&[
            r#"address = 1
               label = "start-lane1"
               mac = "7c:df:a1:00:11:22"
               crystal_ppm = -12.4
               terminated = true
               [[node.input]]
               index = 0
               beam = "prestage"
               lane = 1
               [[node.input]]
               index = 1
               beam = "stage"
               lane = 1
               [[node.input]]
               index = 2
               beam = "guard"
               lane = 1"#,
            r#"address = 2
               label = "start-lane2"
               [[node.input]]
               index = 0
               beam = "prestage"
               lane = 2
               [[node.input]]
               index = 1
               beam = "stage"
               lane = 2
               [[node.input]]
               index = 2
               beam = "guard"
               lane = 2"#,
            r#"address = 4
               label = "trap"
               [[node.input]]
               index = 0
               beam = "trap_entry"
               lane = 1
               [[node.input]]
               index = 1
               beam = "trap_exit"
               lane = 1
               [[node.input]]
               index = 2
               beam = "trap_entry"
               lane = 2
               [[node.input]]
               index = 3
               beam = "trap_exit"
               lane = 2"#,
            r#"address = 6
               label = "finish"
               terminated = true
               [[node.input]]
               index = 0
               beam = "finish"
               lane = 1
               [[node.input]]
               index = 1
               beam = "finish"
               lane = 2"#,
        ])
    }

    fn venue(nodes: &[&str]) -> String {
        let mut s = String::from(
            r#"
[venue]
name = "Test Strip"
lanes = 2

[geometry]
sixty_foot = 18.288
finish = 402.336
trap_base = 20.115

[margin]
source_address = 1
"#,
        );
        for n in nodes {
            s.push_str("\n[[node]]\n");
            for line in n.lines() {
                s.push_str(line.trim());
                s.push('\n');
            }
        }
        s
    }

    fn parse(text: &str) -> Mapping {
        Mapping::parse(text).expect("fixture should parse")
    }

    // -- parsing -----------------------------------------------------------

    #[test]
    fn the_reference_venue_passes_every_static_rule() {
        let m = parse(&valid());
        let r = check_static(&m);
        let errors: Vec<String> = r.errors().map(|p| p.to_string()).collect();
        assert!(errors.is_empty(), "{errors:#?}");
        assert!(r.may_start_a_round());
    }

    #[test]
    fn an_unknown_beam_meaning_is_a_load_error() {
        // §5: a typo must not silently drop a beam.
        let text = valid().replace(r#"beam = "interval_60""#, r#"beam = "sixty_foot""#);
        let text = text.replace(r#"beam = "finish""#, r#"beam = "finsih""#);
        let err = Mapping::parse(&text).expect_err("a misspelled beam must not load");
        assert!(err.to_string().contains("finsih"), "{err}");
    }

    #[test]
    fn an_unknown_key_is_a_load_error() {
        // A misspelled key is the same failure as a misspelled value.
        let text = valid().replace("crystal_ppm = -12.4", "crystal_pmm = -12.4");
        assert!(Mapping::parse(&text).is_err());
    }

    #[test]
    fn a_mac_round_trips_through_its_text_form() {
        let m = parse(&valid());
        assert_eq!(
            m.node(1).unwrap().mac.unwrap().to_string(),
            "7c:df:a1:00:11:22"
        );
        assert_eq!(m.node(1).unwrap().mac.unwrap().0, 0x7CDF_A100_1122);
    }

    #[test]
    fn a_malformed_mac_is_a_load_error() {
        let text = valid().replace("7c:df:a1:00:11:22", "7c:df:a1:00:11");
        assert!(Mapping::parse(&text).is_err());
        let text = valid().replace("7c:df:a1:00:11:22", "7c:df:a1:00:11:zz");
        assert!(Mapping::parse(&text).is_err());
    }

    #[test]
    fn the_beam_vocabulary_matches_the_register_spec() {
        // registers.toml carries the closed set too. If these ever disagree, one
        // of them silently accepts a beam the other rejects.
        let from_protocol = beam402_protocol::map::BEAM_MEANINGS;
        let here: Vec<&str> = Beam::ALL.iter().map(|b| b.wire_name()).collect();
        assert_eq!(here, from_protocol);
    }

    // -- static rules ------------------------------------------------------

    fn fails(text: &str, rule: Rule) {
        let m = parse(text);
        let r = check_static(&m);
        let hit: Vec<&Problem> = r.errors().filter(|p| p.rule == rule).collect::<Vec<_>>();
        assert!(
            !hit.is_empty(),
            "expected {} to fail; got {:#?}",
            rule.label(),
            r.errors().map(|p| p.to_string()).collect::<Vec<_>>()
        );
        assert!(!r.may_start_a_round());
    }

    #[test]
    fn duplicate_meaning_within_a_lane_is_rejected() {
        let text = valid().replace(
            r#"index = 1
beam = "stage"
lane = 2"#,
            r#"index = 1
beam = "stage"
lane = 1"#,
        );
        fails(&text, Rule::NoDuplicateMeaning);
    }

    #[test]
    fn a_lane_without_stage_or_finish_is_rejected() {
        let text = valid().replace(
            r#"index = 1
beam = "stage"
lane = 1"#,
            r#"index = 1
beam = "interval_60"
lane = 1"#,
        );
        fails(&text, Rule::MinimumSystem);
    }

    #[test]
    fn a_trap_without_a_measured_base_is_rejected() {
        let text = valid().replace("trap_base = 20.115\n", "");
        fails(&text, Rule::TrapBase);
    }

    #[test]
    fn a_trap_split_across_two_nodes_is_rejected() {
        // The interval has to close inside one timer, or D20's zero differs
        // between its two ends and the speed is quietly wrong.
        let text = valid().replace(
            r#"index = 1
beam = "trap_exit"
lane = 1"#,
            r#"index = 3
beam = "interval_60"
lane = 1"#,
        );
        let text = text.replace(
            r#"index = 0
beam = "finish"
lane = 1"#,
            r#"index = 0
beam = "finish"
lane = 1
[[node.input]]
index = 2
beam = "trap_exit"
lane = 1"#,
        );
        fails(&text, Rule::TrapOneTimer);
    }

    #[test]
    fn a_bus_with_the_wrong_number_of_ends_is_rejected() {
        let text = valid().replace("terminated = true\n", "");
        fails(&text, Rule::Termination);

        let one_end = valid().replacen("terminated = true\n", "", 1);
        fails(&one_end, Rule::Termination);
    }

    #[test]
    fn guard_on_one_lane_only_is_rejected() {
        let text = valid().replace(
            r#"index = 2
beam = "guard"
lane = 2"#,
            r#"index = 2
beam = "interval_660"
lane = 2"#,
        );
        fails(&text, Rule::GuardSymmetry);
    }

    #[test]
    fn a_margin_source_that_is_not_a_node_is_rejected() {
        let text = valid().replace("source_address = 1", "source_address = 9");
        fails(&text, Rule::MarginSource);
    }

    #[test]
    fn an_address_outside_the_dip_range_is_rejected() {
        let text = valid().replace("address = 4", "address = 64");
        fails(&text, Rule::Structural);
    }

    #[test]
    fn a_third_lane_is_rejected() {
        let text = valid().replace(
            r#"index = 1
beam = "finish"
lane = 2"#,
            r#"index = 1
beam = "finish"
lane = 3"#,
        );
        fails(&text, Rule::Structural);
    }

    #[test]
    fn an_input_index_beyond_the_run_record_is_rejected() {
        let text = valid().replace(
            r#"index = 2
beam = "guard"
lane = 1"#,
            r#"index = 7
beam = "guard"
lane = 1"#,
        );
        fails(&text, Rule::Structural);
    }

    #[test]
    fn every_problem_is_reported_not_just_the_first() {
        let text = valid()
            .replace("terminated = true\n", "")
            .replace("trap_base = 20.115\n", "");
        let r = check_static(&parse(&text));
        assert!(r.has(Rule::Termination));
        assert!(r.has(Rule::TrapBase));
    }

    #[test]
    fn the_capture_reachability_rule_reports_as_unchecked() {
        // It must never look like a pass. software.md §8 #7.
        let r = check_static(&parse(&valid()));
        let un: Vec<&Problem> = r.unchecked().collect();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].rule, Rule::CaptureReachable);
        assert!(r.may_start_a_round(), "unchecked must not block a round");
    }

    // -- bus-dependent rules -----------------------------------------------

    fn facts(address: u8, mac: u64, input_present: u16) -> DeviceFacts {
        DeviceFacts {
            address,
            mac: Mac(mac),
            device_class: DeviceClass::TimingNode,
            protocol_version: 1,
            input_present,
            both_pulses_seen: true,
        }
    }

    fn full_bus() -> Vec<DeviceFacts> {
        vec![
            facts(1, 0x7CDF_A100_1122, 0b0111),
            facts(2, 0x7CDF_A100_3344, 0b0111),
            facts(4, 0x7CDF_A100_5566, 0b1111),
            facts(6, 0x7CDF_A100_7788, 0b0011),
        ]
    }

    #[test]
    fn the_reference_venue_passes_against_a_matching_bus() {
        let r = check_against_bus(&parse(&valid()), &full_bus());
        let errors: Vec<String> = r.errors().map(|p| p.to_string()).collect();
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn a_mapped_input_the_node_does_not_have_is_rejected() {
        let mut bus = full_bus();
        bus[0].input_present = 0b0011; // guard on input 2 is gone
        let r = check_against_bus(&parse(&valid()), &bus);
        assert!(r.has(Rule::InputExists));
        assert!(!r.may_start_a_round());
    }

    #[test]
    fn a_silent_node_blocks_rather_than_passing() {
        let bus: Vec<DeviceFacts> = full_bus().into_iter().filter(|f| f.address != 4).collect();
        let r = check_against_bus(&parse(&valid()), &bus);
        assert!(r.has(Rule::InputExists));
        assert!(!r.may_start_a_round());
    }

    #[test]
    fn a_margin_source_that_has_not_seen_both_pulses_is_rejected() {
        let mut bus = full_bus();
        bus[0].both_pulses_seen = false;
        let r = check_against_bus(&parse(&valid()), &bus);
        assert!(r.has(Rule::MarginSource));
    }

    #[test]
    fn a_crystal_correction_for_an_absent_mac_is_rejected() {
        let mut bus = full_bus();
        bus[0].mac = Mac(0xAAAA_BBBB_CCCC);
        let r = check_against_bus(&parse(&valid()), &bus);
        assert!(r.has(Rule::CrystalMacPresent));
    }

    #[test]
    fn a_swapped_node_warns_and_does_not_block() {
        // D08 exists so a field swap works without editing this file. The warning
        // is there so the swap gets recorded, not to stop the round.
        let mut bus = full_bus();
        bus[3].mac = Mac(0xDEAD_BEEF_0001);
        let text = valid().replace(
            r#"label = "finish""#,
            r#"label = "finish"
mac = "7c:df:a1:00:77:88""#,
        );
        let r = check_against_bus(&parse(&text), &bus);
        assert_eq!(r.warnings().count(), 1);
        assert!(r.may_start_a_round());
    }

    // -- lookups the master will use ---------------------------------------

    #[test]
    fn a_beam_resolves_to_an_address_and_input() {
        let m = parse(&valid());
        let stage = m.site(Lane::L2, Beam::Stage).unwrap();
        assert_eq!((stage.address, stage.input), (2, 1));
        let exit = m.site(Lane::L1, Beam::TrapExit).unwrap();
        assert_eq!((exit.address, exit.input), (4, 1));
        assert!(m.site(Lane::L1, Beam::Interval660).is_none());
    }

    #[test]
    fn prestage_and_guard_are_not_timed_beams() {
        // architecture.md §6: they need no capture channel at all.
        assert!(!Beam::Prestage.is_timed());
        assert!(!Beam::Guard.is_timed());
        assert!(Beam::Stage.is_timed());
        assert!(Beam::Finish.is_timed());
    }
}
