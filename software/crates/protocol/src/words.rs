//! Word packing, and the two newtypes that carry an invariant rather than a unit.
//!
//! Both Modbus stacks hand over 16-bit words, not bytes: `tokio-modbus` returns
//! `Vec<u16>` on the master side, and the ESP-IDF slave works on a `uint16_t`
//! register area. Byte order *inside* a register is therefore the stack's job on
//! both sides, and the only ordering rule that belongs to this crate is the one
//! spanning registers: `protocol.md` §2, high register first.
//!
//! It lives in exactly these six functions. A `u32` assembled anywhere else is a
//! bug that reproduces only above 65,535 ticks.

/// `protocol.md` §2: a `u32` at address *A* has its high 16 bits at *A*.
pub const fn u32_from_words(hi: u16, lo: u16) -> u32 {
    ((hi as u32) << 16) | lo as u32
}

pub const fn u32_to_words(v: u32) -> [u16; 2] {
    [(v >> 16) as u16, v as u16]
}

/// Signed values are two's complement (`protocol.md` §2).
pub const fn i32_from_words(hi: u16, lo: u16) -> i32 {
    u32_from_words(hi, lo) as i32
}

pub const fn i32_to_words(v: i32) -> [u16; 2] {
    u32_to_words(v as u32)
}

/// The factory MAC, high word first. Inventory and the crystal-correction key
/// (**D08**, **D13**) — never an address.
pub const fn u48_from_words(hi: u16, mid: u16, lo: u16) -> u64 {
    ((hi as u64) << 32) | ((mid as u64) << 16) | lo as u64
}

pub const fn u48_to_words(v: u64) -> [u16; 3] {
    [(v >> 32) as u16, (v >> 16) as u16, v as u16]
}

/// Capture-clock counts — 80 MHz, 12.5 ns, wrapping at ~53.7 s (**D20**).
///
/// There is deliberately no conversion to seconds here. Nodes report ticks;
/// converting, applying the per-board crystal correction and dividing distances
/// is the master's job, always (`protocol.md` §2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct Ticks(pub u32);

/// A signed difference of two instants **on one timer**. Never the difference of
/// two nodes' clocks — that is the thing **D04** exists to prevent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct TickDelta(pub i32);

/// Milliseconds on the raw log's coarse clock.
///
/// A separate type from [`Ticks`] on purpose. **D20** puts the edge log on a
/// deliberately coarser clock — dispute evidence does not need 12.5 ns — and the
/// two must never be added, compared or swapped. The type system is a cheaper
/// guard than remembering which register is which.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default, Hash)]
pub struct Millis(pub u32);

/// A run generation, with **D25**'s semantics attached.
///
/// Deliberately not `Ord`: the counter wraps 65535 → 1 and the master must
/// compare it for *inequality*, never for greater-than. Making the ordering
/// unavailable is cheaper than remembering the rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct Generation(u16);

impl Generation {
    /// No run since boot. A rebooted node must never appear to hold a valid split.
    pub const NEVER: Generation = Generation(0);

    pub const fn from_raw(raw: u16) -> Self {
        Generation(raw)
    }

    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn is_never(self) -> bool {
        self.0 == 0
    }

    /// The only comparison **D25** permits.
    pub const fn changed_from(self, prev: Generation) -> bool {
        self.0 != prev.0
    }

    /// The node's own increment: on wrap goes 65535 → 1, **skipping 0**, so a wrap
    /// can never be mistaken for a reboot. Here rather than in the node because
    /// the simulator and the firmware must agree on it exactly.
    pub const fn next(self) -> Generation {
        if self.0 == u16::MAX {
            Generation(1)
        } else {
            Generation(self.0 + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_is_high_register_first() {
        // The bug this ordering causes shows up only above 65,535 ticks, so it is
        // pinned with a value that has distinct halves.
        assert_eq!(u32_from_words(0x0001, 0x86A0), 100_000);
        assert_eq!(u32_to_words(100_000), [0x0001, 0x86A0]);
    }

    #[test]
    fn word_packing_round_trips() {
        for v in [0u32, 1, 0xFFFF, 0x1_0000, 0xDEAD_BEEF, u32::MAX] {
            let [hi, lo] = u32_to_words(v);
            assert_eq!(u32_from_words(hi, lo), v);
        }
        for v in [0i32, -1, i32::MIN, i32::MAX, -100_000] {
            let [hi, lo] = i32_to_words(v);
            assert_eq!(i32_from_words(hi, lo), v);
        }
    }

    #[test]
    fn negative_margin_survives_the_wire() {
        // launch_margin_ticks is signed: lane 2 leaving first is a negative number,
        // and it decides who won.
        let [hi, lo] = i32_to_words(-240_000);
        assert_eq!(i32_from_words(hi, lo), -240_000);
    }

    #[test]
    fn mac_is_high_word_first() {
        let mac = u48_from_words(0x7CDF, 0xA100, 0x1122);
        assert_eq!(mac, 0x7CDF_A100_1122);
        assert_eq!(u48_to_words(mac), [0x7CDF, 0xA100, 0x1122]);
    }

    #[test]
    fn generation_wrap_skips_zero() {
        assert_eq!(Generation::from_raw(65535).next(), Generation::from_raw(1));
        assert_eq!(Generation::from_raw(1).next(), Generation::from_raw(2));
        // ...so a wrap is never read as a reboot.
        assert!(!Generation::from_raw(65535).next().is_never());
    }

    #[test]
    fn generation_only_compares_for_inequality() {
        let prev = Generation::from_raw(65535);
        let now = prev.next();
        assert!(now.changed_from(prev));
        assert!(!now.changed_from(now));
        // A greater-than comparison would call this "went backwards"; it is a wrap.
        // Generation is not Ord, so that comparison cannot be written at all.
    }
}
