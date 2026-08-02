#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

//! # Beam402 — the wire contract
//!
//! The register map between the race control PC (the only bus master) and every
//! device on the trunk. `docs/protocol.md` is the prose; this crate is the map.
//!
//! **Status: nothing here has run against hardware.** No firmware exists. The
//! layout is a proposal that both halves will be held to, and the most expensive
//! thing in the project to change later.
//!
//! ## What this crate is for
//!
//! `protocol.md` §0 asks for exactly one source for the map, because a map
//! maintained by hand in a document and transcribed into two codebases drifts,
//! the drift is silent, and what it produces is a *valid number read from the
//! wrong register*. This crate is that source: `docs/registers.toml` and §3's
//! tables are printed from [`map::REGISTER_MAP`] by the `render-map` binary.
//!
//! It is `no_std` and dependency-free so both halves can share it verbatim —
//! race control on stable Rust, node firmware on the xtensa toolchain fork. That
//! sharing rests on an assumption, and the assumption is stated rather than
//! assumed: **D22** stands at *revisit*, and a Rust node is admissible only once
//! it reproduces the **T3** number on the same rig. If it does not, the fallback
//! is a C-header emitter walking the same table — an output, not a redesign.
//!
//! ## What it refuses to do
//!
//! No ticks are converted to seconds, no crystal correction is applied, no
//! distance is divided. Nodes report ticks; interpreting them is the master's
//! job, always (`protocol.md` §2). And nothing here knows what an ET is: the node
//! has no role (**D24**), so neither does its register map.
//!
//! ## The invariants that ride along
//!
//! Layout is the boring half. These are the ones that produce a plausible wrong
//! number if they are forgotten, so they are attached to the types:
//!
//! - [`Generation`] is not `Ord`. It wraps 65535 → 1 skipping 0, so **D25** says
//!   compare it for inequality and never for greater-than.
//! - [`InputCapture::break_at`] returns `Option`, because "not seen this run" is
//!   data, and `Ticks(0)` is a legal instant.
//! - [`Digest::beam_intact`] reads a *set* bit as intact — under **D17** a zero
//!   is a broken beam or a cut cable, and both faults are loud on purpose.
//! - [`RunRecord::is_timing_valid`] needs `valid && !invalidated`, which is
//!   **D16**: the counter starts on the pulse's leading edge and the width proves
//!   itself 5 ms later.
//! - Word order lives in [`words`] and nowhere else.

pub mod blocks;
pub mod flags;
pub mod map;
pub mod words;

pub use blocks::{
    Access, Block, Command, CommandStatus, DecodeError, DeviceClass, Digest, Identity,
    InputCapture, Lane, LogPage, LogRecord, Opcode, Poll, PulseObservation, RunRecord, Status,
    Telemetry, Tree, TreeMode,
};
pub use flags::{BitDesc, EdgeFlags, FaultFlags, FlagWord, PulseFlags, RunFlags, StatusFlags};
pub use map::{BlockDesc, RegDesc, RegType, REGISTER_MAP};
pub use words::{Generation, Millis, TickDelta, Ticks};

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60 ft split of 1.234567 s at 80 MHz, pinned as registers.
    ///
    /// The value is deliberately above 65,535 ticks: word order is the one bug in
    /// this layout that reproduces only there, so every test that could catch it
    /// must be able to.
    const SPLIT_TICKS: u32 = 98_765_360;
    const SPLIT_HI: u16 = 0x05E3;
    const SPLIT_LO: u16 = 0x0A30;

    fn a_60ft_record() -> RunRecord {
        let mut r = RunRecord {
            gen: Generation::from_raw(7),
            flags: RunFlags::from_bits(0b1_0001), // valid | complete
            input_mask: 0b0001,
            inputs: [InputCapture::default(); RunRecord::INPUTS],
        };
        r.inputs[0] = InputCapture::new(
            2,
            EdgeFlags::from_bits(0b11), // break_valid | make_valid
            SPLIT_TICKS,
            SPLIT_TICKS + 1_650_000, // tire chord, ~20.6 ms
        );
        r
    }

    #[test]
    fn run_record_lands_on_the_documented_offsets() {
        let mut w = [0u16; RunRecord::LEN as usize];
        a_60ft_record().encode(&mut w).unwrap();

        assert_eq!(w[0], 7, "run_gen at +0x00");
        assert_eq!(w[1], 0b1_0001, "run_flags at +0x01");
        assert_eq!(w[2], 0b0001, "input_mask at +0x02");
        assert_eq!(w[3], 0, "reserved reads as 0");
        // Input 0's group starts at +0x04: edge_count, edge_flags, t_break, t_make.
        assert_eq!(w[4], 2);
        assert_eq!(w[5], 0b11);
        assert_eq!(w[6], SPLIT_HI, "t_break high register first");
        assert_eq!(w[7], SPLIT_LO);
    }

    #[test]
    fn run_record_round_trips() {
        let rec = a_60ft_record();
        let mut w = [0u16; RunRecord::LEN as usize];
        rec.encode(&mut w).unwrap();
        assert_eq!(RunRecord::decode(&w).unwrap(), rec);
        assert_eq!(rec.inputs[0].break_at(), Some(Ticks(SPLIT_TICKS)));
    }

    #[test]
    fn an_unobserved_input_is_none_not_zero() {
        // software.md §2: a register that is meaningless at a position reads "not
        // seen this run", which is data, not an error. Input 3 is unpopulated here.
        let rec = a_60ft_record();
        assert_eq!(rec.inputs[3].break_at(), None);
        assert_eq!(rec.inputs[3].make_at(), None);
        assert!(!rec.contributed(3));
        // ...and the raw register is still available for the audit log.
        assert_eq!(rec.inputs[3].raw(), (0, 0));
    }

    #[test]
    fn a_short_read_is_refused() {
        // protocol.md §2: a lane's run record must be read in a single FC3
        // transaction. Handing over 27 registers is the failure this guards.
        let short = [0u16; 27];
        assert_eq!(
            RunRecord::decode(&short),
            Err(DecodeError::WrongLength {
                expected: 28,
                got: 27
            })
        );
        assert!(map::block("run_record").unwrap().atomic);
    }

    #[test]
    fn a_disowned_run_reports_both_facts() {
        // D16: the width proved wrong after the counter had already started.
        let mut rec = a_60ft_record();
        rec.flags = RunFlags::from_bits(0b11); // valid | invalidated
        assert!(rec.flags.valid());
        assert!(!rec.is_timing_valid());
    }

    #[test]
    fn a_self_test_result_is_not_a_race() {
        let mut rec = a_60ft_record();
        rec.flags = RunFlags::from_bits(0b10_0001); // valid | synthetic
        assert!(rec.is_timing_valid());
        assert!(!rec.is_race());
    }

    #[test]
    fn digest_reads_a_broken_beam_as_a_zero_bit() {
        // D17, PNP / Light ON. Inputs 0 and 2 intact, input 1 broken or cut.
        let d = Digest::decode(&[7, 0, 0, 0b0101]).unwrap();
        assert!(d.beam_intact(0));
        assert!(d.beam_broken(1));
        assert!(d.beam_intact(2));
        // An unwired input reads as broken too — loud, which is the point.
        assert!(d.beam_broken(3));
    }

    #[test]
    fn lane_two_record_sits_one_stride_up() {
        assert_eq!(RunRecord::addr(Lane::L1), 0x0050);
        assert_eq!(RunRecord::addr(Lane::L2), 0x0080);
        assert_eq!(Lane::L2.number(), 2);
    }

    #[test]
    fn margin_is_none_until_both_pulses_are_seen() {
        let mut w = [0u16; PulseObservation::LEN as usize];
        // seen_l1 | seen_l2 | width_valid_l1 | width_valid_l2, but NOT margin_valid.
        let mut p = PulseObservation::default();
        p.flags = PulseFlags::from_bits(0b1111);
        p.with_margin(-240_000).encode(&mut w).unwrap();

        let p = PulseObservation::decode(&w).unwrap();
        assert_eq!(p.launch_margin(), None, "0 would be a plausible dead heat");
        assert_eq!(p.launch_margin_raw(), -240_000);

        // With margin_valid the same registers mean something.
        let mut p = PulseObservation::default();
        p.flags = PulseFlags::from_bits(0b1_1111);
        let p = p.with_margin(-240_000);
        assert_eq!(p.launch_margin(), Some(TickDelta(-240_000)));
    }

    #[test]
    fn a_red_light_is_a_negative_reaction_time() {
        let mut w = [0u16; Tree::LEN as usize];
        Tree::default()
            .with_reaction_times(-40_000, 36_000)
            .encode(&mut w)
            .unwrap();
        let t = Tree::decode(&w).unwrap();
        assert!(t.is_red(Lane::L1));
        assert!(!t.is_red(Lane::L2));
        assert_eq!(t.reaction_time(Lane::L1), TickDelta(-40_000));
    }

    #[test]
    fn identity_round_trips_the_mac_and_tick_rate() {
        let id = Identity {
            protocol_version: 1,
            firmware_version: 0x0102,
            device_class: DeviceClass::TimingNode,
            dip_address: 4,
            mac: 0x7CDF_A100_1122,
            input_present: 0b0011,
            capture_channels: 6,
            tick_hz: 80_000_000,
            log_capacity_runs: 256,
        };
        let mut w = [0u16; Identity::LEN as usize];
        id.encode(&mut w).unwrap();
        let back = Identity::decode(&w).unwrap();
        assert_eq!(back, id);
        assert_eq!(back.tick_hz, map::CONVENTIONS.tick_hz);
        assert!(back.dip_valid());
        assert!(back.input_populated(1));
        assert!(!back.input_populated(2));
    }

    #[test]
    fn dip_address_zero_is_a_fault_not_a_broadcast() {
        let mut w = [0u16; Identity::LEN as usize];
        w[0] = 1;
        w[2] = 1;
        w[3] = 0;
        assert!(!Identity::decode(&w).unwrap().dip_valid());
        const { assert!(!map::LINK.broadcast_used) };
    }

    #[test]
    fn an_unknown_device_class_decodes_rather_than_failing() {
        // The master decides what to do with it; the map does not get to refuse.
        assert_eq!(DeviceClass::from_raw(9), DeviceClass::Unknown(9));
        assert_eq!(DeviceClass::from_raw(9).raw(), 9);
        assert_eq!(Opcode::from_raw(99), Opcode::Unknown(99));
    }
}
