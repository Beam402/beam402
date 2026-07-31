//! The five flag words.
//!
//! Each is its own type, so `run_flags` cannot be handed to something expecting
//! `pulse_flags`. Bits not listed here are reserved: `protocol.md` §2 says a
//! master must **ignore** them, never validate them, which is what makes additive
//! protocol changes free. [`unknown_bits`](FlagWord::unknown_bits) exists for
//! diagnostics only — nothing may refuse to work because it is non-zero.

/// One bit's reflection entry, used by `render-map` to print §3's tables.
#[derive(Clone, Copy, Debug)]
pub struct BitDesc {
    pub n: u8,
    pub name: &'static str,
    pub doc: &'static str,
}

/// Shared surface of the five flag words.
pub trait FlagWord: Copy {
    const WORD_NAME: &'static str;
    const BITS: &'static [BitDesc];

    fn bits(self) -> u16;

    /// Bits set that this build has no name for. Diagnostics only.
    fn unknown_bits(self) -> u16 {
        let mut known = 0u16;
        for b in Self::BITS {
            known |= 1u16 << b.n;
        }
        self.bits() & !known
    }
}

macro_rules! flag_word {
    (
        $(#[$meta:meta])*
        $name:ident, $wire:literal {
            $( $n:literal => $flag:ident, $doc:literal );* $(;)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
        pub struct $name(u16);

        impl $name {
            pub const fn from_bits(bits: u16) -> Self { $name(bits) }
            pub const fn bits(self) -> u16 { self.0 }
            $(
                #[doc = concat!("Bit ", stringify!($n), ". ", $doc)]
                pub const fn $flag(self) -> bool { self.0 & (1u16 << $n) != 0 }
            )*
        }

        impl $crate::flags::FlagWord for $name {
            const WORD_NAME: &'static str = $wire;
            const BITS: &'static [BitDesc] = &[
                $( BitDesc { n: $n, name: stringify!($flag), doc: $doc } ),*
            ];
            fn bits(self) -> u16 { self.0 }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                use $crate::flags::FlagWord;
                f.write_str($wire)?;
                f.write_str("(")?;
                let mut first = true;
                for b in <$name as FlagWord>::BITS {
                    if self.0 & (1u16 << b.n) != 0 {
                        if !first { f.write_str("|")?; }
                        f.write_str(b.name)?;
                        first = false;
                    }
                }
                let unknown = FlagWord::unknown_bits(*self);
                if unknown != 0 {
                    if !first { f.write_str("|")?; }
                    write!(f, "reserved:{:#06x}", unknown)?;
                    first = false;
                }
                if first { f.write_str("-")?; }
                f.write_str(")")
            }
        }
    };
}

flag_word! {
    /// Digest word. Everything needed to decide whether anything happened.
    StatusFlags, "status_flags" {
        0 => run_active, "Capture timer synced, run in progress";
        1 => run_complete_l1, "";
        2 => run_complete_l2, "";
        3 => fault_present, "Read fault_flags";
        4 => pulse_invalid_l1, "Width validation failed this run";
        5 => pulse_invalid_l2, "";
        6 => self_test_ready, "";
        7 => log_wrapped, "";
        8 => battery_low, "";
        9 => temp_warning, "";
        10 => alignment_mode_active, "";
    }
}

flag_word! {
    /// Faults. Surfaced on the operator panel; `fault_present` in the digest is
    /// the cheap indication that this word is worth reading.
    FaultFlags, "fault_flags" {
        0 => dip_invalid, "Address 0 read from the switch";
        1 => sensor_health_lost, "A receiver's stability output dropped";
        2 => temp_sensor_missing, "";
        3 => battery_critical, "";
        4 => capture_config_failed, "";
        5 => log_flash_error, "";
        6 => self_test_failed, "";
        7 => unexpected_reset, "";
    }
}

flag_word! {
    /// Pulse observation, present on **every** device (**D24**).
    ///
    /// The `width_marginal_*` bits are the early warning for `architecture.md`
    /// §11 #5: ignition noise on 400 m of cable degrades the margin before it
    /// starts rejecting pulses outright.
    PulseFlags, "pulse_flags" {
        0 => seen_l1, "";
        1 => seen_l2, "";
        2 => width_valid_l1, "";
        3 => width_valid_l2, "";
        4 => margin_valid, "Both pulses seen this run on the same timer";
        5 => width_marginal_l1, "Within 20% of the rejection threshold";
        6 => width_marginal_l2, "";
    }
}

flag_word! {
    /// Per-lane run record status.
    ///
    /// `valid` and `invalidated` are **D16**'s pair: the counter starts on the
    /// pulse's leading edge, and width validation completes 5 ms later. A run can
    /// therefore be timing and then be disowned.
    RunFlags, "run_flags" {
        0 => valid, "Counter started from a width-valid pulse";
        1 => invalidated, "Width proved wrong AFTER the counter started (D16)";
        2 => timer_wrapped, "Run exceeded 53.7 s";
        3 => overflow, "More edges than the record holds";
        4 => complete, "Every populated input reported a break";
        5 => synthetic, "Self-test injection, not a beam. Must never be read as a race";
    }
}

flag_word! {
    /// Per-input edge validity inside a run record.
    EdgeFlags, "edge_flags" {
        0 => break_valid, "";
        1 => make_valid, "";
        2 => multi_edge, "More than one break seen";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_match_the_documented_positions() {
        assert!(RunFlags::from_bits(0b10_0000).synthetic());
        assert!(StatusFlags::from_bits(1 << 10).alignment_mode_active());
        assert!(PulseFlags::from_bits(1 << 4).margin_valid());
        assert!(EdgeFlags::from_bits(0b10).make_valid());
        assert!(FaultFlags::from_bits(1 << 7).unexpected_reset());
    }

    #[test]
    fn reserved_bits_are_ignored_not_rejected() {
        // protocol.md §2: reserved read as 0 and a master MUST ignore them. A node
        // from a later firmware setting bit 15 must not disturb bit 0.
        let f = StatusFlags::from_bits(0b1000_0000_0000_0001);
        assert!(f.run_active());
        assert_eq!(f.unknown_bits(), 0b1000_0000_0000_0000);
    }

    #[test]
    fn a_run_can_be_valid_and_invalidated_at_once() {
        // D16's trap: this combination is not a contradiction, it is the whole
        // point — the counter started, then the width proved wrong.
        let f = RunFlags::from_bits(0b11);
        assert!(f.valid());
        assert!(f.invalidated());
    }

    #[test]
    fn debug_names_the_set_bits() {
        extern crate std;
        use std::format;
        assert_eq!(
            format!("{:?}", RunFlags::from_bits(0b1_0001)),
            "run_flags(valid|complete)"
        );
        assert_eq!(format!("{:?}", RunFlags::from_bits(0)), "run_flags(-)");
    }
}
