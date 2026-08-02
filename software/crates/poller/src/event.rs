//! What the poller emits, and what a cycle cost.
//!
//! Events are the boundary between the impure half of race control and the pure
//! half (`software.md` §4). Everything below this line touches a serial port;
//! everything above it is a function of this stream plus the mapping file. So the
//! stream carries **what the wire said**, not what it meant: no ET, no split, no
//! lane role. Resolving `input_state` bit 2 into "lane 1's stage beam" is the
//! master's job, and it is done in the pure layer where it can be tested.

use beam402_bus::BusError;
use beam402_protocol::map::LINK;
use beam402_protocol::{
    CommandStatus, Digest, Identity, Lane, Opcode, PulseObservation, RunRecord, Status, Telemetry,
    Tree,
};

/// Why the poller believes a device restarted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResetEvidence {
    /// A run generation the master was holding came back as
    /// [`Generation::NEVER`](beam402_protocol::Generation::NEVER). Only a restart
    /// does that: **D25** wraps 65535 → 1 skipping zero precisely so that a wrap
    /// can never be read as one.
    ///
    /// This is the *prompt* evidence — it rides in the four-register digest, so it
    /// arrives on the next cycle rather than on the next slow rotation.
    GenerationCleared,
    /// `boot_count` moved. Slower to arrive, and definitive.
    BootCount { from: u16, to: u16 },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// First contact, or the first read after a restart.
    Identified {
        address: u8,
        identity: Identity,
    },
    /// The device answered a `protocol_version` this build does not implement.
    /// It stays on the panel as alive and is never read for timing: `protocol.md`
    /// §2 says a master refuses rather than guesses.
    Unsupported {
        address: u8,
        protocol_version: u16,
    },
    Silent {
        address: u8,
        error: BusError,
    },
    Returned {
        address: u8,
    },
    /// Everything the master held for this address is stale.
    Reset {
        address: u8,
        evidence: ResetEvidence,
    },
    /// The four-register digest, emitted only when it differs from the last one.
    /// Live beam state for the staging machine rides here.
    Digest {
        address: u8,
        digest: Digest,
    },
    /// A lane's latched record, read in one transaction because that lane's
    /// generation moved.
    Run {
        address: u8,
        lane: Lane,
        record: RunRecord,
    },
    /// This device's view of both start pulses (**D24**), read alongside the run
    /// it belongs to.
    Pulse {
        address: u8,
        observation: PulseObservation,
    },
    Tree {
        address: u8,
        tree: Tree,
    },
    Telemetry {
        address: u8,
        telemetry: Telemetry,
    },
    Status {
        address: u8,
        status: Status,
    },
    /// The node echoed the sequence number and said what it made of the command.
    Commanded {
        address: u8,
        opcode: Opcode,
        status: CommandStatus,
    },
    /// Written, never confirmed. The write may or may not have run — which is why
    /// this is reported rather than retried behind the operator's back.
    CommandLost {
        address: u8,
        opcode: Opcode,
    },
    /// One block failed to read while the device is otherwise answering. Not
    /// silence, and not fatal: the block is tried again next time it is due.
    ReadFailed {
        address: u8,
        block: &'static str,
        error: BusError,
    },
}

impl Event {
    pub const fn address(&self) -> u8 {
        match *self {
            Event::Identified { address, .. }
            | Event::Unsupported { address, .. }
            | Event::Silent { address, .. }
            | Event::Returned { address }
            | Event::Reset { address, .. }
            | Event::Digest { address, .. }
            | Event::Run { address, .. }
            | Event::Pulse { address, .. }
            | Event::Tree { address, .. }
            | Event::Telemetry { address, .. }
            | Event::Status { address, .. }
            | Event::Commanded { address, .. }
            | Event::CommandLost { address, .. }
            | Event::ReadFailed { address, .. } => address,
        }
    }
}

/// What one cycle cost the bus.
///
/// `software.md` §4 prices the poll loop in prose — "100 ms buys about 192
/// characters for the entire bus". This is that arithmetic as a value, so the
/// claim can be asserted against the poller that has to live inside it instead of
/// being re-derived by hand whenever the loop changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CycleStats {
    pub reads: u32,
    pub writes: u32,
    /// Registers moved, counted once — they travel in the response of a read and
    /// in the request of a write, never both.
    pub registers: u32,
    /// Reads that got no answer at all. Charged at the full response timeout,
    /// because that is what they actually cost the cycle.
    pub timeouts: u32,
}

impl CycleStats {
    /// Frame characters, both directions, excluding inter-frame silence.
    ///
    /// FC3 asks in 8 and answers in 5 + 2N; FC16 writes in 9 + 2N and answers
    /// in 8.
    pub const fn characters(&self) -> u32 {
        self.reads * 13 + self.writes * 17 + self.registers * 2
    }

    /// Bus time at `baud`, including the 3.5-character silence every transaction
    /// must be preceded by and the response timeout each unanswered read burns.
    ///
    /// The silence counts because on a half-duplex trunk it is bus time like any
    /// other, and at 19,200 bps it is not a rounding error: 3.5 characters is
    /// ~1.8 ms, which on a ten-device cycle is 18 ms of the budget.
    pub fn millis_at(&self, baud: u32) -> f64 {
        let chars = self.characters() as f64 + (self.reads + self.writes) as f64 * 3.5;
        chars * 10_000.0 / baud as f64 + self.timeout_millis()
    }

    /// At the trunk's own baud (**D05**).
    pub fn millis(&self) -> f64 {
        self.millis_at(LINK.baud)
    }

    /// What silence costs. A dead node is not free: the transport waits out the
    /// response timeout once per attempt, retries included.
    pub fn timeout_millis(&self) -> f64 {
        self.timeouts as f64 * LINK.response_timeout_ms as f64 * (LINK.retries as f64 + 1.0)
    }

    /// Accumulate, for a caller keeping a session total.
    pub fn merge(&mut self, other: CycleStats) {
        self.reads += other.reads;
        self.writes += other.writes;
        self.registers += other.registers;
        self.timeouts += other.timeouts;
    }
}
