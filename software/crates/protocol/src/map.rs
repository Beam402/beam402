//! The map, reflected.
//!
//! The typed blocks in [`blocks`](crate::blocks) are how code touches the
//! registers; this table is the same map in a form a program can walk. It exists
//! so `protocol.md` §3 and `docs/registers.toml` are *printed* rather than
//! maintained — `protocol.md` §0 asks for exactly one source, and with the map in
//! Rust that source is the crate.
//!
//! Reversing the arrow this way has one more payoff worth naming: the same walk
//! emits a C header. If **D22** does not reverse and the node stays on C, the
//! fallback costs an emitter, not a redesign.
//!
//! The tests below hold this table against the typed blocks. Neither is generated
//! from the other, so a mistake has to be made twice in the same way to survive.

use crate::blocks::{
    Access, Block, Command, Digest, Identity, LogPage, Poll, PulseObservation, RunRecord, Status,
    Telemetry, Tree,
};
use crate::flags::{
    BitDesc, EdgeFlags, FaultFlags, FlagWord, LampFlags, PulseFlags, RunFlags, StatusFlags,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegType {
    U16,
    I16,
    U32,
    I32,
    U48,
}

impl RegType {
    pub const fn words(self) -> u16 {
        match self {
            RegType::U16 | RegType::I16 => 1,
            RegType::U32 | RegType::I32 => 2,
            RegType::U48 => 3,
        }
    }

    pub const fn wire_name(self) -> &'static str {
        match self {
            RegType::U16 => "u16",
            RegType::I16 => "i16",
            RegType::U32 => "u32",
            RegType::I32 => "i32",
            RegType::U48 => "u48",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RegDesc {
    pub offset: u16,
    pub name: &'static str,
    pub ty: RegType,
    /// Repeat count for array registers (`temp_bracket`).
    pub count: u16,
    /// Name of the flag word this register carries, if any.
    pub flags: Option<&'static str>,
    /// Name of the enumeration whose values this register takes, if any.
    pub enumeration: Option<&'static str>,
    pub doc: &'static str,
}

impl RegDesc {
    const fn reg(offset: u16, name: &'static str, ty: RegType, doc: &'static str) -> Self {
        RegDesc {
            offset,
            name,
            ty,
            count: 1,
            flags: None,
            enumeration: None,
            doc,
        }
    }

    const fn flagged(offset: u16, name: &'static str, flags: &'static str) -> Self {
        RegDesc {
            offset,
            name,
            ty: RegType::U16,
            count: 1,
            flags: Some(flags),
            enumeration: None,
            doc: "",
        }
    }

    const fn enumerated(offset: u16, name: &'static str, enumeration: &'static str) -> Self {
        RegDesc {
            offset,
            name,
            ty: RegType::U16,
            count: 1,
            flags: None,
            enumeration: Some(enumeration),
            doc: "",
        }
    }

    const fn array(
        offset: u16,
        name: &'static str,
        ty: RegType,
        count: u16,
        doc: &'static str,
    ) -> Self {
        RegDesc {
            offset,
            name,
            ty,
            count,
            flags: None,
            enumeration: None,
            doc,
        }
    }

    /// Registers occupied by this entry.
    pub const fn words(&self) -> u16 {
        self.ty.words() * self.count
    }
}

/// A repeated sub-structure inside a block — the four input groups of a run record.
#[derive(Clone, Copy, Debug)]
pub struct GroupDesc {
    pub name: &'static str,
    pub count: u16,
    /// Offset of the first repetition within the block.
    pub offset: u16,
    /// Registers per repetition.
    pub size: u16,
    pub regs: &'static [RegDesc],
}

#[derive(Clone, Copy, Debug)]
pub struct BlockDesc {
    pub name: &'static str,
    /// One address per lane for per-lane blocks, otherwise one.
    pub addrs: &'static [u16],
    pub lanes: &'static [u8],
    pub stride: u16,
    pub len: u16,
    pub poll: Poll,
    pub access: Access,
    pub atomic: bool,
    pub device_class: Option<u16>,
    pub doc: &'static str,
    pub regs: &'static [RegDesc],
    pub groups: &'static [GroupDesc],
}

#[derive(Clone, Copy, Debug)]
pub struct FlagWordDesc {
    pub name: &'static str,
    pub bits: &'static [BitDesc],
}

pub const FLAG_WORDS: &[FlagWordDesc] = &[
    FlagWordDesc {
        name: <StatusFlags as FlagWord>::WORD_NAME,
        bits: <StatusFlags as FlagWord>::BITS,
    },
    FlagWordDesc {
        name: <FaultFlags as FlagWord>::WORD_NAME,
        bits: <FaultFlags as FlagWord>::BITS,
    },
    FlagWordDesc {
        name: <PulseFlags as FlagWord>::WORD_NAME,
        bits: <PulseFlags as FlagWord>::BITS,
    },
    FlagWordDesc {
        name: <RunFlags as FlagWord>::WORD_NAME,
        bits: <RunFlags as FlagWord>::BITS,
    },
    FlagWordDesc {
        name: <LampFlags as FlagWord>::WORD_NAME,
        bits: <LampFlags as FlagWord>::BITS,
    },
    FlagWordDesc {
        name: <EdgeFlags as FlagWord>::WORD_NAME,
        bits: <EdgeFlags as FlagWord>::BITS,
    },
];

/// Beam meanings are a closed set. An unknown value is a mapping-file **load
/// error**, not a warning — a typo must not silently drop a beam.
pub const BEAM_MEANINGS: &[&str] = &[
    "prestage",
    "stage",
    "guard",
    "interval_60",
    "interval_660",
    "trap_entry",
    "trap_exit",
    "finish",
];

/// A command opcode and what its arguments mean.
///
/// The argument meanings are part of the contract, not a convenience: `arg0` of
/// `self_test` is an interval in µs and `arg0` of `reboot` is a magic number, and
/// nothing in the register map itself distinguishes them.
#[derive(Clone, Copy, Debug)]
pub struct OpcodeDesc {
    pub name: &'static str,
    pub code: u16,
    pub args: &'static str,
}

const fn op(name: &'static str, code: u16, args: &'static str) -> OpcodeDesc {
    OpcodeDesc { name, code, args }
}

pub const OPCODES: &[OpcodeDesc] = &[
    op("identify", 1, "arg0 = seconds"),
    op("alignment_mode", 2, "arg0 = input mask, arg1 = seconds"),
    op(
        "self_test",
        3,
        "arg0 = interval in us; results carry run_flags.synthetic",
    ),
    op("clear_faults", 4, ""),
    op("clear_run", 5, "arg0 = lane mask"),
    op("log_seek", 6, "arg0/arg1 = record index, high word first"),
    op("reboot", 7, "arg0 = magic"),
    op(
        "tree_arm",
        16,
        "arg0 = mode, arg1 = random delay bound in ms (tree only)",
    ),
    op("tree_abort", 17, ""),
    op("tree_lamp_test", 18, ""),
    op(
        "tree_handicap",
        19,
        "arg0 = lane (1|2), arg1 = milliseconds that lane is held back; write before tree_arm",
    ),
];

/// Link layer, per **D05** and `protocol.md` §1.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub baud: u32,
    pub framing: &'static str,
    pub address_min: u8,
    pub address_max: u8,
    pub response_timeout_ms: u16,
    pub retries: u8,
    pub broadcast_used: bool,
}

pub const LINK: Link = Link {
    baud: 19_200,
    framing: "8N1",
    address_min: 1,
    address_max: 63,
    response_timeout_ms: 100,
    retries: 2,
    broadcast_used: false,
};

#[derive(Clone, Copy, Debug)]
pub struct Conventions {
    pub word_order: &'static str,
    pub signed: &'static str,
    pub tick_hz: u32,
    pub temperature_unit: &'static str,
    pub voltage_unit: &'static str,
    pub reserved_policy: &'static str,
}

pub const CONVENTIONS: Conventions = Conventions {
    word_order: "high-register-first",
    signed: "twos-complement",
    tick_hz: 80_000_000,
    temperature_unit: "0.1 C",
    voltage_unit: "mV",
    reserved_policy: "read as 0; masters MUST ignore, never validate",
};

pub const PROTOCOL_VERSION: u16 = 1;

const RUN_RECORD_GROUP: &[RegDesc] = &[
    RegDesc::reg(0, "edge_count", RegType::U16, ""),
    RegDesc::flagged(1, "edge_flags", "edge_flags"),
    RegDesc::reg(
        2,
        "t_break",
        RegType::U32,
        "First break edge, ticks from the pulse.",
    ),
    RegDesc::reg(
        4,
        "t_make",
        RegType::U32,
        "First make edge. Both edges cost one capture channel, not two.",
    ),
];

pub const REGISTER_MAP: &[BlockDesc] = &[
    BlockDesc {
        name: Digest::NAME,
        addrs: &[Digest::ADDR],
        lanes: &[],
        stride: 0,
        len: Digest::LEN,
        poll: Digest::POLL,
        access: Digest::ACCESS,
        atomic: Digest::ATOMIC,
        device_class: Digest::DEVICE_CLASS,
        doc: "Everything needed to decide whether anything happened.",
        regs: &[
            RegDesc::reg(
                0,
                "run_gen_l1",
                RegType::U16,
                "Lane 1 run generation. 0 = no run since boot; wrap goes 65535 -> 1.",
            ),
            RegDesc::reg(1, "run_gen_l2", RegType::U16, "Lane 2 run generation."),
            RegDesc::flagged(2, "status_flags", "status_flags"),
            RegDesc::reg(
                3,
                "input_state",
                RegType::U16,
                "Bit N = input N line active = beam INTACT (D17: PNP, Light ON).",
            ),
        ],
        groups: &[],
    },
    BlockDesc {
        name: Identity::NAME,
        addrs: &[Identity::ADDR],
        lanes: &[],
        stride: 0,
        len: Identity::LEN,
        poll: Identity::POLL,
        access: Identity::ACCESS,
        atomic: Identity::ATOMIC,
        device_class: Identity::DEVICE_CLASS,
        doc: "Static after boot.",
        regs: &[
            RegDesc::reg(0, "protocol_version", RegType::U16, ""),
            RegDesc::reg(1, "firmware_version", RegType::U16, "major << 8 | minor"),
            RegDesc::enumerated(2, "device_class", "device_class"),
            RegDesc::reg(3, "dip_address", RegType::U16, "As read from the switch at boot (D08)."),
            RegDesc::reg(
                4,
                "mac",
                RegType::U48,
                "Factory MAC. Serial number and crystal-correction key only, never an address.",
            ),
            RegDesc::reg(7, "input_present", RegType::U16, "Bitmap of populated inputs."),
            RegDesc::reg(8, "capture_channels", RegType::U16, ""),
            RegDesc::reg(9, "tick_hz", RegType::U32, ""),
            RegDesc::reg(11, "log_capacity_runs", RegType::U16, ""),
        ],
        groups: &[],
    },
    BlockDesc {
        name: Status::NAME,
        addrs: &[Status::ADDR],
        lanes: &[],
        stride: 0,
        len: Status::LEN,
        poll: Status::POLL,
        access: Status::ACCESS,
        atomic: Status::ATOMIC,
        device_class: Status::DEVICE_CLASS,
        doc: "",
        regs: &[
            RegDesc::reg(0, "uptime_s", RegType::U32, ""),
            RegDesc::reg(
                2,
                "boot_count",
                RegType::U16,
                "A change invalidates anything the master holds for this node.",
            ),
            RegDesc::flagged(3, "fault_flags", "fault_flags"),
            RegDesc::reg(4, "bus_frame_errors", RegType::U16, ""),
            RegDesc::reg(5, "bus_crc_errors", RegType::U16, ""),
            RegDesc::reg(6, "command_seq_echo", RegType::U16, ""),
            RegDesc::enumerated(7, "command_status", "command_status"),
            RegDesc::reg(
                8,
                "sensor_health",
                RegType::U16,
                "Receiver self-diagnosis bitmap; primary alignment instrument under D18.",
            ),
        ],
        groups: &[],
    },
    BlockDesc {
        name: Telemetry::NAME,
        addrs: &[Telemetry::ADDR],
        lanes: &[],
        stride: 0,
        len: Telemetry::LEN,
        poll: Telemetry::POLL,
        access: Telemetry::ACCESS,
        atomic: Telemetry::ATOMIC,
        device_class: Telemetry::DEVICE_CLASS,
        doc: "One device per cycle, round-robin.",
        regs: &[
            RegDesc::reg(0, "battery_mv", RegType::U16, ""),
            RegDesc::reg(1, "temp_interior", RegType::I16, ""),
            RegDesc::array(
                2,
                "temp_bracket",
                RegType::I16,
                4,
                "Sensor BODY temperature, not air — D19 depends on this.",
            ),
        ],
        groups: &[],
    },
    BlockDesc {
        name: PulseObservation::NAME,
        addrs: &[PulseObservation::ADDR],
        lanes: &[],
        stride: 0,
        len: PulseObservation::LEN,
        poll: PulseObservation::POLL,
        access: PulseObservation::ACCESS,
        atomic: PulseObservation::ATOMIC,
        device_class: PulseObservation::DEVICE_CLASS,
        doc: "Present on EVERY device (D24). Both lanes' pulses observed on one common timer, which is what makes their difference meaningful.",
        regs: &[
            RegDesc::flagged(0, "pulse_flags", "pulse_flags"),
            RegDesc::reg(1, "pulse_gen_l1", RegType::U16, ""),
            RegDesc::reg(2, "pulse_gen_l2", RegType::U16, ""),
            RegDesc::reg(
                3,
                "pulse_width_l1_us",
                RegType::U16,
                "Measured width. Trending toward the reject threshold is the early warning for architecture.md §11 #5.",
            ),
            RegDesc::reg(4, "pulse_width_l2_us", RegType::U16, ""),
            RegDesc::reg(
                5,
                "launch_margin_ticks",
                RegType::I32,
                "t(pulse2) - t(pulse1), one timer. The first term of D20's margin formula.",
            ),
            RegDesc::reg(7, "t_pulse_l1", RegType::U32, "Raw, for audit."),
            RegDesc::reg(9, "t_pulse_l2", RegType::U32, ""),
        ],
        groups: &[],
    },
    BlockDesc {
        name: RunRecord::NAME,
        addrs: &[
            RunRecord::addr(crate::blocks::Lane::L1),
            RunRecord::addr(crate::blocks::Lane::L2),
        ],
        lanes: &[1, 2],
        stride: RunRecord::STRIDE,
        len: RunRecord::LEN,
        poll: RunRecord::POLL,
        access: RunRecord::ACCESS,
        atomic: RunRecord::ATOMIC,
        device_class: RunRecord::DEVICE_CLASS,
        doc: "Snapshotted whole by the node. Splitting the read can pair a split from one run with a generation from the next.",
        regs: &[
            RegDesc::reg(0, "run_gen", RegType::U16, ""),
            RegDesc::flagged(1, "run_flags", "run_flags"),
            RegDesc::reg(2, "input_mask", RegType::U16, "Inputs that contributed."),
            RegDesc::reg(3, "_reserved", RegType::U16, ""),
        ],
        groups: &[GroupDesc {
            name: "input",
            count: RunRecord::INPUTS as u16,
            offset: RunRecord::GROUP_BASE,
            size: RunRecord::GROUP_SIZE,
            regs: RUN_RECORD_GROUP,
        }],
    },
    BlockDesc {
        name: Tree::NAME,
        addrs: &[Tree::ADDR],
        lanes: &[],
        stride: 0,
        len: Tree::LEN,
        poll: Tree::POLL,
        access: Tree::ACCESS,
        atomic: Tree::ATOMIC,
        device_class: Tree::DEVICE_CLASS,
        doc: "Tree module only. See software.md §5.",
        regs: &[
            RegDesc::enumerated(0, "tree_state", "tree_state"),
            RegDesc::reg(1, "tree_mode", RegType::U16, "0 = standard (500 ms), 1 = pro (400 ms)."),
            RegDesc::flagged(2, "lamp_flags", "lamp_flags"),
            RegDesc::reg(3, "sequence_gen", RegType::U16, ""),
            RegDesc::reg(4, "foul_flags", RegType::U16, ""),
            RegDesc::reg(
                5,
                "handicap_l1_ms",
                RegType::U16,
                "Milliseconds this lane's cascade is held back. Both zero = heads-up.",
            ),
            RegDesc::reg(6, "handicap_l2_ms", RegType::U16, ""),
            RegDesc::reg(
                7,
                "reaction_time_l1",
                RegType::I32,
                "Ticks from this lane's own green. Negative = red light. Measured on the tree's clock (D04 intact).",
            ),
            RegDesc::reg(9, "reaction_time_l2", RegType::I32, ""),
            RegDesc::reg(
                11,
                "t_green_l1",
                RegType::U32,
                "Captured from the lamp driver output, not taken when firmware writes the LED.",
            ),
            RegDesc::reg(13, "t_green_l2", RegType::U32, ""),
        ],
        groups: &[],
    },
    BlockDesc {
        name: Command::NAME,
        addrs: &[Command::ADDR],
        lanes: &[],
        stride: 0,
        len: Command::LEN,
        poll: Command::POLL,
        access: Command::ACCESS,
        atomic: Command::ATOMIC,
        device_class: Command::DEVICE_CLASS,
        doc: "FC6 / FC16. Confirmed by reading command_seq_echo, not by the write's acknowledgement.",
        regs: &[
            RegDesc::enumerated(0, "opcode", "opcode"),
            RegDesc::reg(1, "arg0", RegType::U16, ""),
            RegDesc::reg(2, "arg1", RegType::U16, ""),
            RegDesc::reg(
                3,
                "command_seq",
                RegType::U16,
                "Master increments. Retrying with an unchanged value is safe.",
            ),
        ],
        groups: &[],
    },
    BlockDesc {
        name: LogPage::NAME,
        addrs: &[LogPage::ADDR],
        lanes: &[],
        stride: 0,
        len: LogPage::LEN,
        poll: LogPage::POLL,
        access: LogPage::ACCESS,
        atomic: LogPage::ATOMIC,
        device_class: LogPage::DEVICE_CLASS,
        doc: "Dispute evidence on a coarse millisecond clock (D20). Pulled after a round, never in the live poll loop. The cursor is moved by log_seek only — a read-advancing cursor makes a retried read return different data.",
        regs: &[],
        groups: &[GroupDesc {
            name: "record",
            count: LogPage::RECORDS as u16,
            offset: 0,
            size: LogPage::RECORD_SIZE,
            regs: LOG_PAGE_RECORD,
        }],
    },
];

const LOG_PAGE_RECORD: &[RegDesc] = &[
    RegDesc::reg(
        0,
        "t_ms",
        RegType::U32,
        "Coarse milliseconds, not capture ticks (D20).",
    ),
    RegDesc::reg(2, "input", RegType::U16, ""),
    RegDesc::reg(3, "flags", RegType::U16, ""),
];

pub fn block(name: &str) -> Option<&'static BlockDesc> {
    REGISTER_MAP.iter().find(|b| b.name == name)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The table and the typed blocks are written separately on purpose. This is
    /// what makes that safe: a smeared offset has to be made twice, identically.
    #[test]
    fn table_agrees_with_the_typed_blocks() {
        fn same<B: Block>(d: &BlockDesc) {
            assert_eq!(d.name, B::NAME);
            assert_eq!(d.addrs[0], B::ADDR, "{} base address", B::NAME);
            assert_eq!(d.len, B::LEN, "{} length", B::NAME);
            assert_eq!(d.poll, B::POLL, "{} poll policy", B::NAME);
            assert_eq!(d.atomic, B::ATOMIC, "{} atomicity", B::NAME);
            assert_eq!(d.access, B::ACCESS, "{} access", B::NAME);
            assert_eq!(d.device_class, B::DEVICE_CLASS, "{} device class", B::NAME);
        }
        same::<Digest>(block("digest").unwrap());
        same::<Identity>(block("identity").unwrap());
        same::<Status>(block("status").unwrap());
        same::<Telemetry>(block("telemetry").unwrap());
        same::<PulseObservation>(block("pulse").unwrap());
        same::<RunRecord>(block("run_record").unwrap());
        same::<Tree>(block("tree").unwrap());
        same::<Command>(block("command").unwrap());
        same::<LogPage>(block("log_page").unwrap());
    }

    /// Every block reachable through [`Block`] is in the table. Written out by
    /// hand because a block that exists as a type and not as a row would render a
    /// `registers.toml` that quietly omits it — which is how `log_page` was
    /// missed the first time.
    #[test]
    fn the_table_covers_every_block() {
        let names: Vec<&str> = REGISTER_MAP.iter().map(|b| b.name).collect();
        for expected in [
            Digest::NAME,
            Identity::NAME,
            Status::NAME,
            Telemetry::NAME,
            PulseObservation::NAME,
            RunRecord::NAME,
            Tree::NAME,
            Command::NAME,
            LogPage::NAME,
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from REGISTER_MAP"
            );
        }
        assert_eq!(names.len(), 9, "a block was added without a test");
    }

    /// Every register in a block is accounted for, nothing overlaps, and the
    /// declared length is exactly the space used. An off-by-one in a u32 shows up
    /// here rather than as a number that is wrong only above 65,535 ticks.
    #[test]
    fn blocks_are_gapless_and_non_overlapping() {
        for b in REGISTER_MAP {
            let mut used = std::vec![false; b.len as usize];
            let mut claim = |from: u16, n: u16, what: &str| {
                for o in from..from + n {
                    assert!(
                        (o as usize) < used.len(),
                        "{}: {} runs past the block ({} >= {})",
                        b.name,
                        what,
                        o,
                        b.len
                    );
                    assert!(
                        !used[o as usize],
                        "{}: {} overlaps at offset {}",
                        b.name, what, o
                    );
                    used[o as usize] = true;
                }
            };
            for r in b.regs {
                claim(r.offset, r.words(), r.name);
            }
            for g in b.groups {
                for i in 0..g.count {
                    let base = g.offset + g.size * i;
                    let mut inner = 0;
                    for r in g.regs {
                        claim(base + r.offset, r.words(), r.name);
                        inner += r.words();
                    }
                    assert_eq!(inner, g.size, "{}: group {} size", b.name, g.name);
                }
            }
            let holes: Vec<usize> = used
                .iter()
                .enumerate()
                .filter(|(_, u)| !**u)
                .map(|(i, _)| i)
                .collect();
            assert!(holes.is_empty(), "{}: unmapped offsets {:?}", b.name, holes);
        }
    }

    #[test]
    fn blocks_do_not_collide_in_the_address_space() {
        let mut spans: Vec<(u16, u16, &str)> = Vec::new();
        for b in REGISTER_MAP {
            for a in b.addrs {
                spans.push((*a, *a + b.len, b.name));
            }
        }
        spans.sort();
        for pair in spans.windows(2) {
            let (_, end, first) = pair[0];
            let (start, _, second) = pair[1];
            assert!(end <= start, "{first} overlaps {second}");
        }
    }

    #[test]
    fn flag_bits_are_unique_within_a_word() {
        for w in FLAG_WORDS {
            let mut seen = 0u16;
            for b in w.bits {
                let mask = 1u16 << b.n;
                assert_eq!(seen & mask, 0, "{}: bit {} declared twice", w.name, b.n);
                seen |= mask;
            }
        }
    }

    #[test]
    fn opcodes_are_unique() {
        for (i, a) in OPCODES.iter().enumerate() {
            for b in &OPCODES[i + 1..] {
                assert_ne!(
                    a.code, b.code,
                    "{} and {} share opcode {}",
                    a.name, b.name, a.code
                );
            }
        }
    }

    /// Every opcode the map advertises decodes to a named variant, and back.
    #[test]
    fn opcodes_round_trip_through_the_typed_enum() {
        for o in OPCODES {
            let decoded = crate::blocks::Opcode::from_raw(o.code);
            assert!(
                !matches!(decoded, crate::blocks::Opcode::Unknown(_)),
                "{} ({}) has no variant",
                o.name,
                o.code
            );
            assert_eq!(decoded.raw(), o.code);
        }
    }
}
