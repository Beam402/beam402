//! The register blocks as types.
//!
//! Each block decodes from, and encodes to, a slice of Modbus holding registers.
//! Both directions live here because both halves need both: the master decodes
//! what it read, and the node's register layer encodes what it captured
//! (`software.md` §3 — "the register image is a pure function of (events,
//! config)"). One definition, so the two can never disagree about layout.
//!
//! What the accessors add on top of the layout is the part worth reading: an
//! instant that was never observed is `None` and not `Ticks(0)`, a beam that is
//! intact reads as a *set* bit (**D17**), and a run that started timing before its
//! pulse was disowned reports both facts (**D16**).

use crate::flags::{EdgeFlags, FaultFlags, LampFlags, PulseFlags, RunFlags, StatusFlags};
use crate::words::{
    i32_from_words, i32_to_words, u32_from_words, u32_to_words, u48_from_words, u48_to_words,
    Generation, Millis, TickDelta, Ticks,
};

/// Which lane a per-lane block belongs to. Lane identity lives in the master's
/// mapping file and in the register layout — never in node flash (**D08**).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Lane {
    L1,
    L2,
}

impl Lane {
    pub const ALL: [Lane; 2] = [Lane::L1, Lane::L2];

    /// 0 or 1 — the stride multiplier, not a track-facing number.
    pub const fn ord(self) -> u16 {
        match self {
            Lane::L1 => 0,
            Lane::L2 => 1,
        }
    }

    /// 1 or 2 — what a human and the mapping file call it.
    pub const fn number(self) -> u8 {
        match self {
            Lane::L1 => 1,
            Lane::L2 => 2,
        }
    }

    /// Parse the mapping file's `lane = 1` / `lane = 2`. `None` for anything
    /// else: the register map has exactly two lane records, so a third lane is a
    /// load error rather than something to widen at the edges.
    pub const fn from_number(n: u8) -> Option<Lane> {
        match n {
            1 => Some(Lane::L1),
            2 => Some(Lane::L2),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeviceClass {
    TimingNode,
    TreeModule,
    /// A class this build has no name for. Not an error here — the master decides
    /// what to do with it, and refusing to time on an unknown device is its call.
    Unknown(u16),
}

impl DeviceClass {
    pub const fn from_raw(raw: u16) -> Self {
        match raw {
            1 => DeviceClass::TimingNode,
            2 => DeviceClass::TreeModule,
            other => DeviceClass::Unknown(other),
        }
    }

    pub const fn raw(self) -> u16 {
        match self {
            DeviceClass::TimingNode => 1,
            DeviceClass::TreeModule => 2,
            DeviceClass::Unknown(v) => v,
        }
    }
}

/// What a node made of the last command it was written.
///
/// The taxonomy stays deliberately small — accepted, or not. A list of rejection
/// reasons is worth having once firmware has reasons to report, and inventing one
/// now would put a meaning on the wire that nothing has agreed to.
///
/// It lives here rather than in the firmware crate because the *master* is the
/// party that has to read it: a value space known to only one side of a wire
/// contract is not a contract.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CommandStatus {
    /// No command has been written since boot.
    #[default]
    None,
    Accepted,
    Rejected,
    Unknown(u16),
}

impl CommandStatus {
    pub const fn from_raw(raw: u16) -> Self {
        match raw {
            0 => CommandStatus::None,
            1 => CommandStatus::Accepted,
            2 => CommandStatus::Rejected,
            other => CommandStatus::Unknown(other),
        }
    }

    pub const fn raw(self) -> u16 {
        match self {
            CommandStatus::None => 0,
            CommandStatus::Accepted => 1,
            CommandStatus::Rejected => 2,
            CommandStatus::Unknown(v) => v,
        }
    }

    /// The node has answered for this command, one way or the other.
    pub const fn is_settled(self) -> bool {
        !matches!(self, CommandStatus::None)
    }
}

/// Where the sequence as a whole has got to.
///
/// One value for two lanes, which under a handicap start are deliberately not in
/// the same place: this is the *furthest along* of them, and the per-lane detail
/// lives in [`LampFlags`]. It exists as the cheap summary for an operator panel,
/// not as something to drive logic from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TreeState {
    #[default]
    Idle,
    /// Armed and waiting for both cars to stage.
    Armed,
    /// At least one lane's cascade has started.
    Sequencing,
    /// Both lanes have had their green.
    Green,
    Unknown(u16),
}

impl TreeState {
    pub const fn from_raw(raw: u16) -> Self {
        match raw {
            0 => TreeState::Idle,
            1 => TreeState::Armed,
            2 => TreeState::Sequencing,
            3 => TreeState::Green,
            other => TreeState::Unknown(other),
        }
    }

    pub const fn raw(self) -> u16 {
        match self {
            TreeState::Idle => 0,
            TreeState::Armed => 1,
            TreeState::Sequencing => 2,
            TreeState::Green => 3,
            TreeState::Unknown(v) => v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeMode {
    /// 500 ms cascade.
    Standard,
    /// 400 ms cascade.
    Pro,
    Unknown(u16),
}

impl TreeMode {
    pub const fn from_raw(raw: u16) -> Self {
        match raw {
            0 => TreeMode::Standard,
            1 => TreeMode::Pro,
            other => TreeMode::Unknown(other),
        }
    }

    pub const fn raw(self) -> u16 {
        match self {
            TreeMode::Standard => 0,
            TreeMode::Pro => 1,
            TreeMode::Unknown(v) => v,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opcode {
    Identify,
    AlignmentMode,
    SelfTest,
    ClearFaults,
    ClearRun,
    LogSeek,
    Reboot,
    TreeArm,
    TreeAbort,
    TreeLampTest,
    TreeHandicap,
    TreeStaging,
    Unknown(u16),
}

impl Opcode {
    pub const fn from_raw(raw: u16) -> Self {
        match raw {
            1 => Opcode::Identify,
            2 => Opcode::AlignmentMode,
            3 => Opcode::SelfTest,
            4 => Opcode::ClearFaults,
            5 => Opcode::ClearRun,
            6 => Opcode::LogSeek,
            7 => Opcode::Reboot,
            16 => Opcode::TreeArm,
            17 => Opcode::TreeAbort,
            18 => Opcode::TreeLampTest,
            19 => Opcode::TreeHandicap,
            20 => Opcode::TreeStaging,
            other => Opcode::Unknown(other),
        }
    }

    pub const fn raw(self) -> u16 {
        match self {
            Opcode::Identify => 1,
            Opcode::AlignmentMode => 2,
            Opcode::SelfTest => 3,
            Opcode::ClearFaults => 4,
            Opcode::ClearRun => 5,
            Opcode::LogSeek => 6,
            Opcode::Reboot => 7,
            Opcode::TreeArm => 16,
            Opcode::TreeAbort => 17,
            Opcode::TreeLampTest => 18,
            Opcode::TreeHandicap => 19,
            Opcode::TreeStaging => 20,
            Opcode::Unknown(v) => v,
        }
    }
}

/// When the master reads a block. Policy, not layout — it is here because
/// **D25** makes it part of the contract rather than a tuning choice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Poll {
    /// The four-register digest, every cycle.
    EveryCycle,
    /// Identity: read once, cached until `boot_count` moves.
    Once,
    /// Only when that lane's generation changes.
    OnGenerationChange,
    /// One device per cycle.
    RoundRobin,
    OnFaultOrSlowRotation,
    /// After a round, on request. Never in the live poll loop.
    OnRequest,
    /// Written by the master, not polled.
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeError {
    /// The slice handed in is not the block's length. For an atomic block this is
    /// the guard against a record assembled from two transactions.
    WrongLength { expected: u16, got: usize },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::WrongLength { expected, got } => {
                write!(f, "expected {expected} registers, got {got}")
            }
        }
    }
}

/// A contiguous block of holding registers.
///
/// `POLL` and `ATOMIC` ride along as associated constants so the poll scheduler
/// can be generic over blocks and the compiler can hold it to "an atomic block is
/// read in one transaction" — a rule that is otherwise a sentence in a document.
pub trait Block: Sized {
    const NAME: &'static str;
    /// For per-lane blocks this is lane 1's base; use the block's own `addr()`.
    const ADDR: u16;
    const LEN: u16;
    const POLL: Poll;
    const ACCESS: Access;
    /// `protocol.md` §2: the node snapshots the block whole, and splitting the
    /// read can pair a split from one run with a generation from the next.
    const ATOMIC: bool;
    /// `Some` if the block exists only on one device class.
    const DEVICE_CLASS: Option<u16>;

    fn decode(w: &[u16]) -> Result<Self, DecodeError>;
    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError>;
}

const fn check(len: u16, got: usize) -> Result<(), DecodeError> {
    if got == len as usize {
        Ok(())
    } else {
        Err(DecodeError::WrongLength { expected: len, got })
    }
}

// ---------------------------------------------------------------------------
// 0x0000 — Digest
// ---------------------------------------------------------------------------

/// Four registers, a 13-character exchange: everything the master needs to decide
/// whether anything happened (**D25**).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Digest {
    pub run_gen_l1: Generation,
    pub run_gen_l2: Generation,
    pub status: StatusFlags,
    /// Raw bitmap. Read it through [`beam_intact`](Digest::beam_intact) — the
    /// polarity is the opposite of the intuitive one.
    pub input_state: u16,
}

impl Digest {
    pub const fn run_gen(&self, lane: Lane) -> Generation {
        match lane {
            Lane::L1 => self.run_gen_l1,
            Lane::L2 => self.run_gen_l2,
        }
    }

    pub const fn run_complete(&self, lane: Lane) -> bool {
        match lane {
            Lane::L1 => self.status.run_complete_l1(),
            Lane::L2 => self.status.run_complete_l2(),
        }
    }

    pub const fn pulse_invalid(&self, lane: Lane) -> bool {
        match lane {
            Lane::L1 => self.status.pulse_invalid_l1(),
            Lane::L2 => self.status.pulse_invalid_l2(),
        }
    }

    /// **D17**: bit *N* set = line active = beam **intact**. Under PNP / Light ON
    /// a zero means a broken beam *or* a cut cable, and both faults are loud —
    /// which is the entire point of that decision.
    pub const fn beam_intact(&self, input: u8) -> bool {
        self.input_state & (1u16 << input) != 0
    }

    pub const fn beam_broken(&self, input: u8) -> bool {
        !self.beam_intact(input)
    }
}

impl Block for Digest {
    const NAME: &'static str = "digest";
    const ADDR: u16 = 0x0000;
    const LEN: u16 = 4;
    const POLL: Poll = Poll::EveryCycle;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Digest {
            run_gen_l1: Generation::from_raw(w[0]),
            run_gen_l2: Generation::from_raw(w[1]),
            status: StatusFlags::from_bits(w[2]),
            input_state: w[3],
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.run_gen_l1.raw();
        w[1] = self.run_gen_l2.raw();
        w[2] = self.status.bits();
        w[3] = self.input_state;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0010 — Identity
// ---------------------------------------------------------------------------

/// Static after boot. `protocol_version` gates everything else: a master that
/// reads a version it does not know refuses to use the node for timing and says
/// so — it does not guess (`protocol.md` §2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Identity {
    pub protocol_version: u16,
    /// major << 8 | minor
    pub firmware_version: u16,
    pub device_class: DeviceClass,
    /// As read from the DIP switch at boot — the node's only configuration (**D08**).
    pub dip_address: u16,
    /// Factory MAC. Serial number and crystal-correction key, never an address.
    pub mac: u64,
    pub input_present: u16,
    pub capture_channels: u16,
    pub tick_hz: u32,
    pub log_capacity_runs: u16,
}

impl Identity {
    pub const fn input_populated(&self, input: u8) -> bool {
        self.input_present & (1u16 << input) != 0
    }

    /// `fault_flags.dip_invalid` covers this on the node side; the master checks
    /// it too, because address 0 read from a switch is a fault, not a broadcast.
    pub const fn dip_valid(&self) -> bool {
        self.dip_address >= 1 && self.dip_address <= 63
    }
}

impl Block for Identity {
    const NAME: &'static str = "identity";
    const ADDR: u16 = 0x0010;
    const LEN: u16 = 12;
    const POLL: Poll = Poll::Once;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Identity {
            protocol_version: w[0],
            firmware_version: w[1],
            device_class: DeviceClass::from_raw(w[2]),
            dip_address: w[3],
            mac: u48_from_words(w[4], w[5], w[6]),
            input_present: w[7],
            capture_channels: w[8],
            tick_hz: u32_from_words(w[9], w[10]),
            log_capacity_runs: w[11],
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.protocol_version;
        w[1] = self.firmware_version;
        w[2] = self.device_class.raw();
        w[3] = self.dip_address;
        let mac = u48_to_words(self.mac);
        w[4] = mac[0];
        w[5] = mac[1];
        w[6] = mac[2];
        w[7] = self.input_present;
        w[8] = self.capture_channels;
        let hz = u32_to_words(self.tick_hz);
        w[9] = hz[0];
        w[10] = hz[1];
        w[11] = self.log_capacity_runs;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0020 — Status and counters
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Status {
    pub uptime_s: u32,
    /// A change invalidates anything the master holds for this node.
    pub boot_count: u16,
    pub faults: FaultFlags,
    pub bus_frame_errors: u16,
    pub bus_crc_errors: u16,
    pub command_seq_echo: u16,
    pub command_status: CommandStatus,
    /// Receiver self-diagnosis bitmap — the primary alignment instrument under **D18**.
    pub sensor_health: u16,
}

impl Block for Status {
    const NAME: &'static str = "status";
    const ADDR: u16 = 0x0020;
    const LEN: u16 = 9;
    const POLL: Poll = Poll::OnFaultOrSlowRotation;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Status {
            uptime_s: u32_from_words(w[0], w[1]),
            boot_count: w[2],
            faults: FaultFlags::from_bits(w[3]),
            bus_frame_errors: w[4],
            bus_crc_errors: w[5],
            command_seq_echo: w[6],
            command_status: CommandStatus::from_raw(w[7]),
            sensor_health: w[8],
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        let up = u32_to_words(self.uptime_s);
        w[0] = up[0];
        w[1] = up[1];
        w[2] = self.boot_count;
        w[3] = self.faults.bits();
        w[4] = self.bus_frame_errors;
        w[5] = self.bus_crc_errors;
        w[6] = self.command_seq_echo;
        w[7] = self.command_status.raw();
        w[8] = self.sensor_health;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0030 — Telemetry
// ---------------------------------------------------------------------------

/// Bracket temperature is mandatory, not decorative: **D19** needs the temperature
/// of the sensor *body*, because without it you cannot distinguish a hot day from
/// a lying sensor.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Telemetry {
    pub battery_mv: u16,
    /// 0.1 °C, signed.
    pub temp_interior: i16,
    pub temp_bracket: [i16; 4],
}

impl Block for Telemetry {
    const NAME: &'static str = "telemetry";
    const ADDR: u16 = 0x0030;
    const LEN: u16 = 6;
    const POLL: Poll = Poll::RoundRobin;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Telemetry {
            battery_mv: w[0],
            temp_interior: w[1] as i16,
            temp_bracket: [w[2] as i16, w[3] as i16, w[4] as i16, w[5] as i16],
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.battery_mv;
        w[1] = self.temp_interior as u16;
        for (i, t) in self.temp_bracket.iter().enumerate() {
            w[2 + i] = *t as u16;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0040 — Pulse observation
// ---------------------------------------------------------------------------

/// Present on **every** device (**D24**). Both lanes' pulses are observed on one
/// common timer, which is what makes their difference meaningful.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PulseObservation {
    pub flags: PulseFlags,
    pub gen_l1: Generation,
    pub gen_l2: Generation,
    pub width_l1_us: u16,
    pub width_l2_us: u16,
    launch_margin_ticks: i32,
    pub t_pulse_l1: u32,
    pub t_pulse_l2: u32,
}

impl PulseObservation {
    pub const fn seen(&self, lane: Lane) -> bool {
        match lane {
            Lane::L1 => self.flags.seen_l1(),
            Lane::L2 => self.flags.seen_l2(),
        }
    }

    pub const fn width_valid(&self, lane: Lane) -> bool {
        match lane {
            Lane::L1 => self.flags.width_valid_l1(),
            Lane::L2 => self.flags.width_valid_l2(),
        }
    }

    /// Trending toward the rejection threshold is the operator's warning a round
    /// before it costs anybody a run (`architecture.md` §11 #5).
    pub const fn width_marginal(&self, lane: Lane) -> bool {
        match lane {
            Lane::L1 => self.flags.width_marginal_l1(),
            Lane::L2 => self.flags.width_marginal_l2(),
        }
    }

    pub const fn width_us(&self, lane: Lane) -> u16 {
        match lane {
            Lane::L1 => self.width_l1_us,
            Lane::L2 => self.width_l2_us,
        }
    }

    /// `t(pulse₂) − t(pulse₁)`, the first term of **D20**'s margin formula.
    ///
    /// `None` unless both pulses were seen on the same timer this run — the raw
    /// register would read 0, which is a perfectly plausible dead heat.
    pub const fn launch_margin(&self) -> Option<TickDelta> {
        if self.flags.margin_valid() {
            Some(TickDelta(self.launch_margin_ticks))
        } else {
            None
        }
    }

    /// The raw register, for the audit log. Prefer [`launch_margin`](Self::launch_margin).
    pub const fn launch_margin_raw(&self) -> i32 {
        self.launch_margin_ticks
    }

    pub const fn with_margin(mut self, ticks: i32) -> Self {
        self.launch_margin_ticks = ticks;
        self
    }
}

impl Block for PulseObservation {
    const NAME: &'static str = "pulse";
    const ADDR: u16 = 0x0040;
    const LEN: u16 = 11;
    const POLL: Poll = Poll::OnGenerationChange;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(PulseObservation {
            flags: PulseFlags::from_bits(w[0]),
            gen_l1: Generation::from_raw(w[1]),
            gen_l2: Generation::from_raw(w[2]),
            width_l1_us: w[3],
            width_l2_us: w[4],
            launch_margin_ticks: i32_from_words(w[5], w[6]),
            t_pulse_l1: u32_from_words(w[7], w[8]),
            t_pulse_l2: u32_from_words(w[9], w[10]),
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.flags.bits();
        w[1] = self.gen_l1.raw();
        w[2] = self.gen_l2.raw();
        w[3] = self.width_l1_us;
        w[4] = self.width_l2_us;
        let m = i32_to_words(self.launch_margin_ticks);
        w[5] = m[0];
        w[6] = m[1];
        let p1 = u32_to_words(self.t_pulse_l1);
        w[7] = p1[0];
        w[8] = p1[1];
        let p2 = u32_to_words(self.t_pulse_l2);
        w[9] = p2[0];
        w[10] = p2[1];
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0050 / 0x0080 — Run records
// ---------------------------------------------------------------------------

/// One input's capture within a run record.
///
/// Both edges of every beam, always: §2 starts ET when the tire *exits* the stage
/// beam and stops it when the tire *breaks* the finish beam, and **T2** needs make
/// and break as separate numbers to measure the asymmetry between them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InputCapture {
    pub edge_count: u16,
    pub flags: EdgeFlags,
    t_break: u32,
    t_make: u32,
}

impl InputCapture {
    pub const fn new(edge_count: u16, flags: EdgeFlags, t_break: u32, t_make: u32) -> Self {
        InputCapture {
            edge_count,
            flags,
            t_break,
            t_make,
        }
    }

    /// First break, ticks from the pulse. `None` is "not seen this run", which is
    /// data and not an error — a finish node with no stage beam simply never
    /// observes one (`software.md` §2). It is deliberately not `Ticks(0)`, because
    /// zero is a legal instant.
    pub const fn break_at(&self) -> Option<Ticks> {
        if self.flags.break_valid() {
            Some(Ticks(self.t_break))
        } else {
            None
        }
    }

    pub const fn make_at(&self) -> Option<Ticks> {
        if self.flags.make_valid() {
            Some(Ticks(self.t_make))
        } else {
            None
        }
    }

    /// Raw registers for the audit log, valid bit or not.
    pub const fn raw(&self) -> (u32, u32) {
        (self.t_break, self.t_make)
    }
}

/// A lane's latched run record. 28 registers, read in **one** transaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RunRecord {
    pub gen: Generation,
    pub flags: RunFlags,
    pub input_mask: u16,
    pub inputs: [InputCapture; RunRecord::INPUTS],
}

impl RunRecord {
    pub const INPUTS: usize = 4;
    /// Registers per input group.
    pub const GROUP_SIZE: u16 = 6;
    /// Offset of the first input group within the block.
    pub const GROUP_BASE: u16 = 4;
    pub const STRIDE: u16 = 0x30;

    pub const fn addr(lane: Lane) -> u16 {
        Self::ADDR + Self::STRIDE * lane.ord()
    }

    /// **D16**: the counter starts on the pulse's leading edge and width
    /// validation completes 5 ms later, so a run can be timing and then be
    /// disowned. Both facts are reported; only their conjunction is a usable run.
    pub const fn is_timing_valid(&self) -> bool {
        self.flags.valid() && !self.flags.invalidated()
    }

    /// A self-test injection must never be mistaken for a race.
    pub const fn is_race(&self) -> bool {
        !self.flags.synthetic()
    }

    pub const fn contributed(&self, input: u8) -> bool {
        self.input_mask & (1u16 << input) != 0
    }
}

impl Block for RunRecord {
    const NAME: &'static str = "run_record";
    const ADDR: u16 = 0x0050;
    const LEN: u16 = 28;
    const POLL: Poll = Poll::OnGenerationChange;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = true;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        let mut inputs = [InputCapture::default(); Self::INPUTS];
        let mut i = 0;
        while i < Self::INPUTS {
            let b = (Self::GROUP_BASE + Self::GROUP_SIZE * i as u16) as usize;
            inputs[i] = InputCapture {
                edge_count: w[b],
                flags: EdgeFlags::from_bits(w[b + 1]),
                t_break: u32_from_words(w[b + 2], w[b + 3]),
                t_make: u32_from_words(w[b + 4], w[b + 5]),
            };
            i += 1;
        }
        Ok(RunRecord {
            gen: Generation::from_raw(w[0]),
            flags: RunFlags::from_bits(w[1]),
            input_mask: w[2],
            // w[3] reserved — read as 0, ignored, never validated.
            inputs,
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.gen.raw();
        w[1] = self.flags.bits();
        w[2] = self.input_mask;
        w[3] = 0;
        for (i, cap) in self.inputs.iter().enumerate() {
            let b = (Self::GROUP_BASE + Self::GROUP_SIZE * i as u16) as usize;
            w[b] = cap.edge_count;
            w[b + 1] = cap.flags.bits();
            let br = u32_to_words(cap.t_break);
            w[b + 2] = br[0];
            w[b + 3] = br[1];
            let mk = u32_to_words(cap.t_make);
            w[b + 4] = mk[0];
            w[b + 5] = mk[1];
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x00C0 — Tree module
// ---------------------------------------------------------------------------

/// The tree owns the instant the green lit and, under **D24**, also observes the
/// launch pulse — so both terms of the reaction time sit on one clock and **D04**
/// is not violated to produce a number handed to a driver.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tree {
    pub state: TreeState,
    pub mode: TreeMode,
    pub lamps: LampFlags,
    pub sequence_gen: Generation,
    pub foul_flags: u16,
    /// Milliseconds this lane's cascade is held back — the handicap the *armed*
    /// sequence will use, echoed so the master can verify it before a car stages
    /// rather than discovering it from the result. Zero on the lane that leaves
    /// first, and on both lanes in a heads-up race.
    handicap_ms: [u16; 2],
    reaction_time_l1: i32,
    reaction_time_l2: i32,
    /// Captured from the lamp driver output, not taken when firmware writes the LED.
    pub t_green_l1: u32,
    pub t_green_l2: u32,
}

impl Tree {
    pub const fn reaction_time(&self, lane: Lane) -> TickDelta {
        match lane {
            Lane::L1 => TickDelta(self.reaction_time_l1),
            Lane::L2 => TickDelta(self.reaction_time_l2),
        }
    }

    /// A red light is not a special case; it is a negative reaction time.
    pub const fn is_red(&self, lane: Lane) -> bool {
        self.reaction_time(lane).0 < 0
    }

    pub const fn t_green(&self, lane: Lane) -> u32 {
        match lane {
            Lane::L1 => self.t_green_l1,
            Lane::L2 => self.t_green_l2,
        }
    }

    pub const fn with_reaction_times(mut self, l1: i32, l2: i32) -> Self {
        self.reaction_time_l1 = l1;
        self.reaction_time_l2 = l2;
        self
    }

    /// How long this lane's cascade is held back. Both zero is a heads-up start.
    pub const fn handicap_ms(&self, lane: Lane) -> u16 {
        self.handicap_ms[lane.ord() as usize]
    }

    pub const fn with_handicap(mut self, l1_ms: u16, l2_ms: u16) -> Self {
        self.handicap_ms = [l1_ms, l2_ms];
        self
    }

    /// A handicap start is running. Worth its own name because it changes what
    /// the lamps mean: the two lanes are not in the same place, so a shared
    /// amber column would be a lie.
    pub const fn is_handicap(&self) -> bool {
        self.handicap_ms[0] != 0 || self.handicap_ms[1] != 0
    }
}

impl Default for Tree {
    fn default() -> Self {
        Tree {
            state: TreeState::Idle,
            mode: TreeMode::Standard,
            lamps: LampFlags::from_bits(0),
            sequence_gen: Generation::NEVER,
            foul_flags: 0,
            handicap_ms: [0; 2],
            reaction_time_l1: 0,
            reaction_time_l2: 0,
            t_green_l1: 0,
            t_green_l2: 0,
        }
    }
}

impl Block for Tree {
    const NAME: &'static str = "tree";
    const ADDR: u16 = 0x00C0;
    const LEN: u16 = 15;
    const POLL: Poll = Poll::OnGenerationChange;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = Some(2);

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Tree {
            state: TreeState::from_raw(w[0]),
            mode: TreeMode::from_raw(w[1]),
            lamps: LampFlags::from_bits(w[2]),
            sequence_gen: Generation::from_raw(w[3]),
            foul_flags: w[4],
            handicap_ms: [w[5], w[6]],
            reaction_time_l1: i32_from_words(w[7], w[8]),
            reaction_time_l2: i32_from_words(w[9], w[10]),
            t_green_l1: u32_from_words(w[11], w[12]),
            t_green_l2: u32_from_words(w[13], w[14]),
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.state.raw();
        w[1] = self.mode.raw();
        w[2] = self.lamps.bits();
        w[3] = self.sequence_gen.raw();
        w[4] = self.foul_flags;
        w[5] = self.handicap_ms[0];
        w[6] = self.handicap_ms[1];
        let r1 = i32_to_words(self.reaction_time_l1);
        w[7] = r1[0];
        w[8] = r1[1];
        let r2 = i32_to_words(self.reaction_time_l2);
        w[9] = r2[0];
        w[10] = r2[1];
        let g1 = u32_to_words(self.t_green_l1);
        w[11] = g1[0];
        w[12] = g1[1];
        let g2 = u32_to_words(self.t_green_l2);
        w[13] = g2[0];
        w[14] = g2[1];
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0100 — Commands
// ---------------------------------------------------------------------------

/// Written with FC6 / FC16. The node echoes `command_seq` at `command_seq_echo`
/// and the result at `command_status`, so a command is confirmed by a subsequent
/// read rather than by the write's acknowledgement — which is what makes retrying
/// a write with an unchanged sequence number safe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Command {
    pub opcode: Opcode,
    pub arg0: u16,
    pub arg1: u16,
    pub seq: u16,
}

impl Block for Command {
    const NAME: &'static str = "command";
    const ADDR: u16 = 0x0100;
    const LEN: u16 = 4;
    const POLL: Poll = Poll::Write;
    const ACCESS: Access = Access::Write;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        Ok(Command {
            opcode: Opcode::from_raw(w[0]),
            arg0: w[1],
            arg1: w[2],
            seq: w[3],
        })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        w[0] = self.opcode.raw();
        w[1] = self.arg0;
        w[2] = self.arg1;
        w[3] = self.seq;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 0x0200 — Raw log page
// ---------------------------------------------------------------------------

/// One logged edge. Dispute evidence, not a timing source.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LogRecord {
    /// Coarse milliseconds (**D20**), not capture ticks — hence [`Millis`].
    pub t_ms: Millis,
    pub input: u16,
    pub flags: u16,
}

/// A page of the raw edge log, 16 records of 4 registers.
///
/// The cursor is set by the `log_seek` command and is **not advanced by
/// reading**: a read-advancing cursor makes a retried read return different data,
/// which is exactly what a noisy bus produces. Idempotent reads, explicit seeks.
///
/// Pulled after a round. It never appears in the live poll loop, and the node
/// never writes flash during a run — flash operations here run with interrupts
/// disabled and would stall the path being measured.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LogPage {
    pub records: [LogRecord; LogPage::RECORDS],
}

impl LogPage {
    pub const RECORDS: usize = 16;
    pub const RECORD_SIZE: u16 = 4;
}

impl Default for LogPage {
    fn default() -> Self {
        LogPage {
            records: [LogRecord::default(); LogPage::RECORDS],
        }
    }
}

impl Block for LogPage {
    const NAME: &'static str = "log_page";
    const ADDR: u16 = 0x0200;
    const LEN: u16 = 64;
    const POLL: Poll = Poll::OnRequest;
    const ACCESS: Access = Access::Read;
    const ATOMIC: bool = false;
    const DEVICE_CLASS: Option<u16> = None;

    fn decode(w: &[u16]) -> Result<Self, DecodeError> {
        check(Self::LEN, w.len())?;
        let mut records = [LogRecord::default(); Self::RECORDS];
        let mut i = 0;
        while i < Self::RECORDS {
            let b = i * Self::RECORD_SIZE as usize;
            records[i] = LogRecord {
                t_ms: Millis(u32_from_words(w[b], w[b + 1])),
                input: w[b + 2],
                flags: w[b + 3],
            };
            i += 1;
        }
        Ok(LogPage { records })
    }

    fn encode(&self, w: &mut [u16]) -> Result<(), DecodeError> {
        check(Self::LEN, w.len())?;
        for (i, r) in self.records.iter().enumerate() {
            let b = i * Self::RECORD_SIZE as usize;
            let t = u32_to_words(r.t_ms.0);
            w[b] = t[0];
            w[b + 1] = t[1];
            w[b + 2] = r.input;
            w[b + 3] = r.flags;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Layout assertions — a smeared offset is a build error, not a field surprise.
// ---------------------------------------------------------------------------

const _: () = assert!(RunRecord::LEN == 28);
const _: () = assert!(
    RunRecord::GROUP_BASE + RunRecord::GROUP_SIZE * RunRecord::INPUTS as u16 == RunRecord::LEN
);
const _: () = assert!(RunRecord::addr(Lane::L1) == 0x0050);
const _: () = assert!(RunRecord::addr(Lane::L2) == 0x0080);
// Blocks must not run into each other.
const _: () = assert!(Digest::ADDR + Digest::LEN <= Identity::ADDR);
const _: () = assert!(Identity::ADDR + Identity::LEN <= Status::ADDR);
const _: () = assert!(Status::ADDR + Status::LEN <= Telemetry::ADDR);
const _: () = assert!(Telemetry::ADDR + Telemetry::LEN <= PulseObservation::ADDR);
const _: () = assert!(PulseObservation::ADDR + PulseObservation::LEN <= RunRecord::addr(Lane::L1));
const _: () = assert!(RunRecord::addr(Lane::L1) + RunRecord::LEN <= RunRecord::addr(Lane::L2));
const _: () = assert!(RunRecord::addr(Lane::L2) + RunRecord::LEN <= Tree::ADDR);
const _: () = assert!(Tree::ADDR + Tree::LEN <= Command::ADDR);
const _: () = assert!(Command::ADDR + Command::LEN <= LogPage::ADDR);
const _: () = assert!(LogPage::RECORDS as u16 * LogPage::RECORD_SIZE == LogPage::LEN);
