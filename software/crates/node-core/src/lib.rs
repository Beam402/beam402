#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

//! # Beam402 — the node's register layer
//!
//! `(capture events, config) → register image`, with no I/O and no clock reads.
//! This is [`software.md`] §3's seam and §7's tier 1: on the device the events
//! come from MCPWM, on a host they are constructed, and the register image is a
//! pure function of both either way.
//!
//! **Status: nothing here has run against hardware.** No firmware exists. What
//! this crate is today is the simulator's node model — and, under **D27**'s
//! shared crate, the same code the firmware will compile. It is written to that
//! standard rather than as a test double, because it is the reference the silicon
//! gets compared against.
//!
//! ## The two clocks, both local
//!
//! **D20** puts each lane's beams on their own capture timer, zeroed by that
//! lane's start pulse, and observes both pulses on one *common* timer. So this
//! crate takes ticks on two different bases and never mixes them:
//!
//! - [`Event::Edge`] carries ticks **from that lane's pulse** — a split, directly.
//! - [`Event::Pulse`] carries ticks on the **common** timer, which is the only
//!   reason `launch_margin_ticks` means anything (**D04** stays intact because
//!   both of its terms come off one clock).
//!
//! ## What it refuses to do
//!
//! No ET, no split, no speed, no lane meaning. The node does not know what it
//! measured (**D24**), so neither does this crate. Ticks go out as ticks.
//!
//! [`software.md`]: https://github.com/perfilev-dev/beam402/blob/main/docs/software.md

use beam402_protocol::blocks::{
    Access, Block, Command, CommandStatus, DeviceClass, Digest, Identity, LogPage, LogRecord,
    Opcode, PulseObservation, RunRecord, Status, Telemetry, Tree,
};
use beam402_protocol::flags::{EdgeFlags, FaultFlags, PulseFlags, RunFlags, StatusFlags};
use beam402_protocol::map::REGISTER_MAP;
use beam402_protocol::words::{Generation, Millis, Ticks};
use beam402_protocol::{InputCapture, Lane};

/// Widest block in the map, so a read can be assembled on the stack.
const MAX_BLOCK: usize = 64;
/// Edges buffered in RAM during a run. Flushed to flash between rounds by
/// firmware — never during a run, because flash writes here run with interrupts
/// disabled and would stall the path being measured.
pub const LOG_CAPACITY: usize = 256;

/// Modbus exception codes this layer can raise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exception {
    /// 02 — the range covers an address this device does not implement.
    IllegalDataAddress = 2,
    /// 03 — a legal address, an impossible value or count.
    IllegalDataValue = 3,
}

/// What the 5 ms start pulse has to look like to be believed.
///
/// **The numbers are placeholders until the bench speaks.** `architecture.md` §11
/// #5 is about ignition noise on 400 m of cable, and the tolerance that actually
/// separates a pulse from a spike is a measurement, not a constant somebody
/// picked. The *shape* is what matters here: an accepted band, and a marginal
/// band inside it that warns before runs start being thrown away.
#[derive(Clone, Copy, Debug)]
pub struct PulseSpec {
    pub nominal_us: u16,
    pub tolerance_us: u16,
}

impl Default for PulseSpec {
    fn default() -> Self {
        PulseSpec {
            nominal_us: 5_000,
            tolerance_us: 500,
        }
    }
}

impl PulseSpec {
    pub fn accepts(&self, width_us: u16) -> bool {
        self.deviation(width_us) <= self.tolerance_us
    }

    /// Within 20 % of the rejection threshold — `pulse_flags.width_marginal_*`.
    /// This is the early warning: a width trending toward the edge is visible to
    /// the operator a round before it costs anybody a run.
    pub fn is_marginal(&self, width_us: u16) -> bool {
        let d = self.deviation(width_us);
        self.accepts(width_us) && d * 5 >= self.tolerance_us * 4
    }

    fn deviation(&self, width_us: u16) -> u16 {
        width_us.abs_diff(self.nominal_us)
    }
}

/// Everything a node knows about itself. All of it is either strapped (the DIP
/// address) or built in — none of it is configured over the bus, which is
/// **D08**.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub dip_address: u8,
    pub device_class: DeviceClass,
    pub mac: u64,
    pub firmware_version: u16,
    /// Bitmap of populated inputs.
    pub input_present: u16,
    pub capture_channels: u16,
    pub tick_hz: u32,
    pub pulse: PulseSpec,
}

impl Config {
    pub fn timing_node(dip_address: u8, mac: u64, input_present: u16) -> Self {
        Config {
            dip_address,
            device_class: DeviceClass::TimingNode,
            mac,
            firmware_version: 0x0001,
            input_present,
            capture_channels: 6,
            tick_hz: 80_000_000,
            pulse: PulseSpec::default(),
        }
    }

    pub fn tree(dip_address: u8, mac: u64) -> Self {
        Config {
            device_class: DeviceClass::TreeModule,
            input_present: 0,
            ..Config::timing_node(dip_address, mac, 0)
        }
    }

    fn populated(&self) -> impl Iterator<Item = u8> + '_ {
        (0..RunRecord::INPUTS as u8).filter(move |i| self.input_present & (1u16 << i) != 0)
    }
}

/// Which edge of a beam. Under **D17** the line is active while the beam is
/// intact, so a `Break` is the line going inactive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EdgeKind {
    Break,
    Make,
}

/// Everything that can reach the register layer. Constructed by a test or a
/// simulator; produced by MCPWM on silicon.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// A start pulse's **leading edge** on one lane: the capture timer is zeroed
    /// and a new run begins. `t` is on the common pulse-observation timer.
    ///
    /// **D16**: the run starts here. Its width is not known yet.
    Pulse { lane: Lane, t: Ticks },
    /// The width, measured when the pulse ends — 5 ms after the run started.
    /// A wrong width **invalidates a run that has already been timing**; waiting
    /// for it before starting would add 5 ms to every measurement.
    PulseWidth { lane: Lane, width_us: u16 },
    /// A beam edge. `t` is ticks from that lane's pulse, so it is a split already.
    Edge {
        lane: Lane,
        input: u8,
        t: Ticks,
        kind: EdgeKind,
        /// Coarse milliseconds for the raw log (**D20**) — a different clock, on
        /// purpose, and never mixed with `t`.
        log_t: Millis,
    },
    /// The capture hardware could not keep up, so the record cannot represent
    /// what happened. What actually triggers this on silicon is `software.md`
    /// §8 #6, which is why it arrives as an event rather than being inferred.
    CaptureOverrun { lane: Lane },
    /// A second of wall time passed, for `uptime_s`. Not in any timing path.
    Second,
}

/// A node, or a tree module — the difference is [`Config::device_class`] plus one
/// block, not a separate build (**D07**, **D24**).
#[derive(Clone, Debug)]
pub struct NodeCore {
    cfg: Config,

    run_gen: [Generation; 2],
    runs: [RunRecord; 2],
    /// Per lane: has this run's width been judged yet, and did it pass.
    width_judged: [bool; 2],
    pulse: PulseObservation,
    pulse_seen_t: [Option<Ticks>; 2],

    input_state: u16,
    /// The half of `status_flags` that latches until the next pulse. The rest is
    /// derived from live state on every read — see [`NodeCore::status_flags`].
    latched_status: u16,
    /// One bit in the digest covers two capture timers, so it means "either lane
    /// is mid-run". Tracked per lane and collapsed on read.
    lane_active: [bool; 2],
    self_test_armed: bool,
    self_test_ready: bool,
    alignment_until: Option<u32>,
    identify_until: Option<u32>,

    status: Status,
    telemetry: Telemetry,
    tree: Tree,

    log: [LogRecord; LOG_CAPACITY],
    log_len: usize,
    log_wrapped: bool,
    log_cursor: u32,

    last_command: Command,
}

impl NodeCore {
    pub fn new(cfg: Config) -> Self {
        // D17: every line reads active — beam intact — until something breaks it.
        // A node that booted with a cut cable shows zeros immediately, loudly.
        let input_state = cfg.input_present;
        NodeCore {
            cfg,
            run_gen: [Generation::NEVER; 2],
            runs: [RunRecord::default(); 2],
            width_judged: [false; 2],
            pulse: PulseObservation::default(),
            pulse_seen_t: [None; 2],
            input_state,
            latched_status: 0,
            lane_active: [false; 2],
            self_test_armed: false,
            self_test_ready: false,
            alignment_until: None,
            identify_until: None,
            status: Status {
                boot_count: 1,
                sensor_health: cfg.input_present,
                ..Status::default()
            },
            telemetry: Telemetry {
                battery_mv: 13_200,
                temp_interior: 250,
                temp_bracket: [250; 4],
            },
            tree: Tree::default(),
            log: [LogRecord::default(); LOG_CAPACITY],
            log_len: 0,
            log_wrapped: false,
            log_cursor: 0,
            last_command: Command {
                opcode: Opcode::Unknown(0),
                arg0: 0,
                arg1: 0,
                seq: 0,
            },
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn is_tree(&self) -> bool {
        self.cfg.device_class == DeviceClass::TreeModule
    }

    /// The tree's block, for a simulator's sequence machine to drive. A timing
    /// node has one too — it just never reads as anything but zero, which is
    /// **D24**: a meaningless register is data, not an error.
    pub fn tree_mut(&mut self) -> &mut Tree {
        &mut self.tree
    }

    pub fn telemetry_mut(&mut self) -> &mut Telemetry {
        &mut self.telemetry
    }

    pub fn faults_mut(&mut self) -> &mut FaultFlags {
        &mut self.status.faults
    }

    pub fn run_gen(&self, lane: Lane) -> Generation {
        self.run_gen[lane.ord() as usize]
    }

    pub fn run(&self, lane: Lane) -> &RunRecord {
        &self.runs[lane.ord() as usize]
    }

    // -- events ------------------------------------------------------------

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Pulse { lane, t } => self.on_pulse(lane, t),
            Event::PulseWidth { lane, width_us } => self.on_width(lane, width_us),
            Event::Edge {
                lane,
                input,
                t,
                kind,
                log_t,
            } => self.on_edge(lane, input, t, kind, log_t),
            Event::CaptureOverrun { lane } => {
                let r = &mut self.runs[lane.ord() as usize];
                r.flags = RunFlags::from_bits(r.flags.bits() | 1 << 3);
            }
            Event::Second => self.status.uptime_s = self.status.uptime_s.saturating_add(1),
        }
    }

    fn on_pulse(&mut self, lane: Lane, t: Ticks) {
        let i = lane.ord() as usize;

        // D25: the generation moves on every *change to the record*, wrapping
        // 65535 -> 1 so a wrap can never be read as a reboot. The sync is one
        // such change; see `on_edge` for the other, and why it has to be.
        self.run_gen[i] = self.run_gen[i].next();

        // The record is replaced whole. Latching means the *previous* numbers
        // stayed readable until exactly this moment (D25) — not that they linger.
        let mut flags = 1u16; // valid: the counter did start
        if self.self_test_armed {
            flags |= 1 << 5; // synthetic: never to be mistaken for a race
        }
        self.runs[i] = RunRecord {
            gen: self.run_gen[i],
            flags: RunFlags::from_bits(flags),
            input_mask: 0,
            inputs: [InputCapture::default(); RunRecord::INPUTS],
        };
        self.width_judged[i] = false;
        self.lane_active[i] = true;

        // The pulse observation lives on the common timer, which is what makes
        // the difference between the two pulses mean anything (D20).
        self.pulse_seen_t[i] = Some(t);
        let mut pf = self.pulse.flags.bits();
        pf |= 1 << i; // seen_lN
        pf &= !(1 << (4 + i)); // width_marginal_lN, until judged
        self.pulse.flags = PulseFlags::from_bits(pf);
        match lane {
            Lane::L1 => {
                self.pulse.gen_l1 = self.run_gen[i];
                self.pulse.t_pulse_l1 = t.0;
            }
            Lane::L2 => {
                self.pulse.gen_l2 = self.run_gen[i];
                self.pulse.t_pulse_l2 = t.0;
            }
        }
        self.recompute_margin();

        self.latched_status &= !(1 << (1 + i)); // run_complete_lN
        self.latched_status &= !(1 << (4 + i)); // pulse_invalid_lN

        self.self_test_armed = false;
    }

    fn on_width(&mut self, lane: Lane, width_us: u16) {
        let i = lane.ord() as usize;
        let ok = self.cfg.pulse.accepts(width_us);
        self.width_judged[i] = true;

        match lane {
            Lane::L1 => self.pulse.width_l1_us = width_us,
            Lane::L2 => self.pulse.width_l2_us = width_us,
        }

        let mut pf = self.pulse.flags.bits();
        if ok {
            pf |= 1 << (2 + i); // width_valid_lN
        } else {
            pf &= !(1 << (2 + i));
        }
        if self.cfg.pulse.is_marginal(width_us) {
            pf |= 1 << (5 + i); // width_marginal_lN
        } else {
            pf &= !(1 << (5 + i));
        }
        self.pulse.flags = PulseFlags::from_bits(pf);
        self.recompute_margin();

        if !ok {
            // D16: the run has already been timing for 5 ms. It is disowned, not
            // prevented — and `valid` stays set, because the counter did start.
            let r = &mut self.runs[i];
            r.flags = RunFlags::from_bits(r.flags.bits() | 1 << 1);
            self.latched_status |= 1 << (4 + i); // pulse_invalid_lN
        }
    }

    fn on_edge(&mut self, lane: Lane, input: u8, t: Ticks, kind: EdgeKind, log_t: Millis) {
        if input as usize >= RunRecord::INPUTS {
            return;
        }
        let bit = 1u16 << input;

        match kind {
            EdgeKind::Break => self.input_state &= !bit,
            EdgeKind::Make => self.input_state |= bit,
        }

        self.push_log(input, kind, log_t);

        let i = lane.ord() as usize;
        let slot = &mut self.runs[i].inputs[input as usize];
        let mut ef = slot.flags.bits();
        let (mut t_break, mut t_make) = slot.raw();
        let count = slot.edge_count.saturating_add(1);

        match kind {
            EdgeKind::Break => {
                if ef & 1 == 0 {
                    t_break = t.0;
                    ef |= 1; // break_valid
                } else {
                    ef |= 1 << 2; // multi_edge — more than one break seen
                }
            }
            EdgeKind::Make => {
                if ef & 2 == 0 {
                    t_make = t.0;
                    ef |= 2; // make_valid
                }
            }
        }
        *slot = InputCapture::new(count, EdgeFlags::from_bits(ef), t_break, t_make);
        self.runs[i].input_mask |= bit;

        // The record just changed, so the generation moves — **D25**'s "poll a
        // digest for change" is about the record's contents, not about which run
        // it belongs to.
        //
        // Advancing only on the sync would leave the master reading a record that
        // is correct and empty: the pulse arrives at the launch and the beams are
        // crossed seconds later, with nothing in the four-register digest to say
        // so. `run_complete` cannot serve, because on a node shared between lanes
        // it never sets (see below, and `software.md` §8 #7). This is the one
        // signal that works with the registers that exist, and it makes each read
        // self-checking as well: the record carries its own generation, so a
        // master can tell whether what it read is still current.
        //
        // **Not before the first pulse, and not after a reboot.** Generation zero
        // means "no run since boot", and it is the master's whole defence against
        // a capture taken while the timer was free-running (`protocol.md` §4). An
        // edge that lands there is recorded and left at zero, so it stays a number
        // rather than becoming a split.
        if !self.run_gen[i].is_never() {
            self.run_gen[i] = self.run_gen[i].next();
            self.runs[i].gen = self.run_gen[i];
        }

        // `complete` is every *populated* input having reported a break.
        //
        // Note what that means on a node shared between lanes, because it is not
        // what the flag's name suggests: input 0 serves lane 1 and input 1 serves
        // lane 2, so lane 1's record can never see input 1 break, and `complete`
        // never sets. The node cannot do better — it does not know which input
        // belongs to which lane (D24), which is the same missing fact as
        // software.md §8 #7. Implemented as specified, and the flag is unusable on
        // shared nodes until that gap closes.
        let all = self
            .cfg
            .populated()
            .all(|n| self.runs[i].inputs[n as usize].flags.break_valid());
        if all && self.cfg.input_present != 0 {
            let r = &mut self.runs[i];
            r.flags = RunFlags::from_bits(r.flags.bits() | 1 << 4);
            self.latched_status |= 1 << (1 + i); // run_complete_lN
            self.lane_active[i] = false;
        }
    }

    fn recompute_margin(&mut self) {
        let mut pf = self.pulse.flags.bits();
        match (self.pulse_seen_t[0], self.pulse_seen_t[1]) {
            (Some(a), Some(b)) => {
                pf |= 1 << 4; // margin_valid
                self.pulse.flags = PulseFlags::from_bits(pf);
                // Both terms are on the common timer. This is the one subtraction
                // the node is allowed to do (D20).
                self.pulse = self.pulse.with_margin(b.0 as i32 - a.0 as i32);
            }
            _ => {
                pf &= !(1 << 4);
                self.pulse.flags = PulseFlags::from_bits(pf);
            }
        }
    }

    fn push_log(&mut self, input: u8, kind: EdgeKind, t: Millis) {
        let rec = LogRecord {
            t_ms: t,
            input: input as u16,
            flags: match kind {
                EdgeKind::Break => 0,
                EdgeKind::Make => 1,
            },
        };
        if self.log_len == LOG_CAPACITY {
            self.log_wrapped = true;
            self.log.rotate_left(1);
            self.log[LOG_CAPACITY - 1] = rec;
        } else {
            self.log[self.log_len] = rec;
            self.log_len += 1;
        }
    }

    // -- commands ----------------------------------------------------------

    fn execute(&mut self, cmd: Command) {
        self.last_command = cmd;
        let accepted = match cmd.opcode {
            Opcode::Identify => {
                self.identify_until = Some(self.status.uptime_s + cmd.arg0 as u32);
                true
            }
            Opcode::AlignmentMode => {
                self.alignment_until = Some(self.status.uptime_s + cmd.arg1 as u32);
                true
            }
            Opcode::SelfTest => {
                // The next run carries `synthetic`, so a self-test result can
                // never be read as a race.
                self.self_test_armed = true;
                self.self_test_ready = true;
                true
            }
            Opcode::ClearFaults => {
                self.status.faults = FaultFlags::default();
                true
            }
            Opcode::ClearRun => {
                for lane in Lane::ALL {
                    if cmd.arg0 & (1 << lane.ord()) != 0 {
                        let i = lane.ord() as usize;
                        self.runs[i] = RunRecord::default();
                        self.run_gen[i] = Generation::NEVER;
                        self.pulse_seen_t[i] = None;
                    }
                }
                self.recompute_margin();
                true
            }
            Opcode::LogSeek => {
                self.log_cursor = ((cmd.arg0 as u32) << 16) | cmd.arg1 as u32;
                true
            }
            Opcode::Reboot => {
                if cmd.arg0 == 0 {
                    false
                } else {
                    self.reboot();
                    true
                }
            }
            // The tree's own commands are the sequence machine's business; the
            // register layer only records that they were accepted for a device of
            // the right class.
            Opcode::TreeArm | Opcode::TreeAbort | Opcode::TreeLampTest => self.is_tree(),
            // The one tree command with an argument that can be wrong on its
            // face. Rejecting it here means a handicap written to lane 3 is
            // refused rather than silently dropped, which matters: the failure
            // it prevents is a race started heads-up that both drivers were told
            // was a handicap.
            Opcode::TreeHandicap => self.is_tree() && Lane::from_number(cmd.arg0 as u8).is_some(),
            Opcode::Unknown(_) => false,
        };
        self.status.command_seq_echo = cmd.seq;
        self.status.command_status = if accepted {
            CommandStatus::Accepted
        } else {
            CommandStatus::Rejected
        };
    }

    /// Everything volatile goes; `boot_count` moves so the master invalidates
    /// whatever it was holding, and generations return to "no run since boot".
    pub fn reboot(&mut self) {
        let boot_count = self.status.boot_count.saturating_add(1);
        let faults = FaultFlags::from_bits(self.status.faults.bits() | 1 << 7); // unexpected_reset
        let cfg = self.cfg;
        *self = NodeCore::new(cfg);
        self.status.boot_count = boot_count;
        self.status.faults = faults;
    }

    /// The command a master last wrote, for a simulator's sequence machine.
    pub fn last_command(&self) -> Command {
        self.last_command
    }

    // -- the register image ------------------------------------------------

    /// The digest's flag word: part latched, part derived from live state.
    pub fn status_flags(&self) -> StatusFlags {
        let mut bits = self.latched_status & LATCHED_STATUS_MASK;
        if self.lane_active.iter().any(|a| *a) {
            bits |= 1 << 0; // run_active
        }
        if self.status.faults.bits() != 0 {
            bits |= 1 << 3; // fault_present
        }
        if self.self_test_ready {
            bits |= 1 << 6;
        }
        if self.log_wrapped {
            bits |= 1 << 7;
        }
        if self.telemetry.battery_mv < 11_000 {
            bits |= 1 << 8; // battery_low
        }
        if self.telemetry.temp_bracket.iter().any(|t| *t > 550) {
            bits |= 1 << 9; // temp_warning
        }
        if self.alignment_until.is_some() {
            bits |= 1 << 10;
        }
        StatusFlags::from_bits(bits)
    }

    pub fn digest(&self) -> Digest {
        Digest {
            run_gen_l1: self.run_gen[0],
            run_gen_l2: self.run_gen[1],
            status: self.status_flags(),
            input_state: self.input_state,
        }
    }

    pub fn identity(&self) -> Identity {
        Identity {
            protocol_version: beam402_protocol::map::PROTOCOL_VERSION,
            firmware_version: self.cfg.firmware_version,
            device_class: self.cfg.device_class,
            dip_address: self.cfg.dip_address as u16,
            mac: self.cfg.mac,
            input_present: self.cfg.input_present,
            capture_channels: self.cfg.capture_channels,
            tick_hz: self.cfg.tick_hz,
            log_capacity_runs: LOG_CAPACITY as u16,
        }
    }

    pub fn pulse_observation(&self) -> PulseObservation {
        self.pulse
    }

    /// The log page at the current cursor. **Reading does not advance it** — a
    /// read-advancing cursor makes a retried read return different data, which is
    /// exactly what a noisy bus produces.
    pub fn log_page(&self) -> LogPage {
        let mut page = LogPage::default();
        for (n, slot) in page.records.iter_mut().enumerate() {
            let idx = self.log_cursor as usize + n;
            if idx < self.log_len {
                *slot = self.log[idx];
            }
        }
        page
    }

    /// Answer an FC3 read.
    ///
    /// Address decoding walks [`REGISTER_MAP`] rather than a hand-written table,
    /// so a block that exists in the contract is readable here by construction —
    /// and the gaps between blocks raise exception 02, as `protocol.md` §2 says
    /// unimplemented addresses must.
    pub fn read(&self, addr: u16, count: u16, out: &mut [u16]) -> Result<(), Exception> {
        if count == 0 || out.len() != count as usize {
            return Err(Exception::IllegalDataValue);
        }
        let end = addr
            .checked_add(count)
            .ok_or(Exception::IllegalDataAddress)?;
        let mut scratch = [0u16; MAX_BLOCK];
        let mut covered = 0u16;

        for desc in REGISTER_MAP {
            if let Some(class) = desc.device_class {
                if self.cfg.device_class.raw() != class {
                    continue; // not implemented on this device: 0x00C0 on a node
                }
            }
            for base in desc.addrs {
                let lo = addr.max(*base);
                let hi = end.min(base + desc.len);
                if lo >= hi {
                    continue;
                }
                let lane = if desc.addrs.len() > 1 && *base == desc.addrs[1] {
                    Lane::L2
                } else {
                    Lane::L1
                };
                self.encode(desc.name, lane, &mut scratch[..desc.len as usize])?;
                for a in lo..hi {
                    out[(a - addr) as usize] = scratch[(a - base) as usize];
                }
                covered += hi - lo;
            }
        }

        if covered == count {
            Ok(())
        } else {
            Err(Exception::IllegalDataAddress)
        }
    }

    /// Answer an FC6 / FC16 write. Only the command block is writable.
    ///
    /// `true` means the command *ran*; `false` means the sequence number was
    /// already echoed and this was an idempotent retry. Both are successful
    /// writes — the distinction exists because a caller standing in for the
    /// hardware (the simulator's tree) must not re-arm on a repeated frame.
    pub fn write(&mut self, addr: u16, values: &[u16]) -> Result<bool, Exception> {
        if values.is_empty() {
            return Err(Exception::IllegalDataValue);
        }
        let end = addr as usize + values.len();
        if addr != Command::ADDR || end != (Command::ADDR + Command::LEN) as usize {
            // A partial command write would execute half an instruction. The
            // sequence number is the last register on purpose: the whole block
            // arrives in one transaction or not at all.
            return Err(Exception::IllegalDataAddress);
        }
        let cmd = Command::decode(values).map_err(|_| Exception::IllegalDataValue)?;
        // Retrying a write with an unchanged sequence number is safe, so a repeat
        // must not run the command twice.
        if cmd.seq == self.status.command_seq_echo && self.status.command_status.is_settled() {
            return Ok(false);
        }
        self.execute(cmd);
        Ok(true)
    }

    fn encode(&self, name: &str, lane: Lane, out: &mut [u16]) -> Result<(), Exception> {
        let r = match name {
            "digest" => self.digest().encode(out),
            "identity" => self.identity().encode(out),
            "status" => self.status.encode(out),
            "telemetry" => self.telemetry.encode(out),
            "pulse" => self.pulse.encode(out),
            "run_record" => self.runs[lane.ord() as usize].encode(out),
            "tree" => self.tree.encode(out),
            "command" => self.last_command.encode(out),
            "log_page" => self.log_page().encode(out),
            _ => return Err(Exception::IllegalDataAddress),
        };
        r.map_err(|_| Exception::IllegalDataValue)
    }
}

/// Bits of `status_flags` that latch until the next pulse, as opposed to being
/// derived from live state on every read.
const LATCHED_STATUS_MASK: u16 = (1 << 1) | (1 << 2) | (1 << 4) | (1 << 5);

// The write path only accepts the command block, so nothing else needs Access.
const _: () = assert!(matches!(Command::ACCESS, Access::Write));

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_protocol::map::PROTOCOL_VERSION;
    use beam402_protocol::TickDelta;

    /// A shared 60 ft / finish node: inputs 0 and 1 populated, one per lane.
    fn node() -> NodeCore {
        NodeCore::new(Config::timing_node(3, 0x7CDF_A100_1122, 0b0011))
    }

    /// Ticks are 12.5 ns, so these are real-looking splits well above 65,535 —
    /// the range where a word-order mistake would show.
    const T_60FT: u32 = 96_000_000; // 1.2 s
    const CHORD: u32 = 1_650_000; // ~20.6 ms of tire

    fn pulse(n: &mut NodeCore, lane: Lane, t: u32) {
        n.apply(Event::Pulse { lane, t: Ticks(t) });
    }

    fn edge(n: &mut NodeCore, lane: Lane, input: u8, t: u32, kind: EdgeKind) {
        n.apply(Event::Edge {
            lane,
            input,
            t: Ticks(t),
            kind,
            log_t: Millis(t / 80_000),
        });
    }

    fn clean_pass(n: &mut NodeCore, lane: Lane, input: u8, t: u32) {
        edge(n, lane, input, t, EdgeKind::Break);
        edge(n, lane, input, t + CHORD, EdgeKind::Make);
    }

    // -- a run, end to end -------------------------------------------------

    #[test]
    fn a_clean_run_lands_where_the_master_will_look() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 1_000);
        n.apply(Event::PulseWidth {
            lane: Lane::L1,
            width_us: 5_000,
        });
        clean_pass(&mut n, Lane::L1, 0, T_60FT);

        let r = *n.run(Lane::L1);
        // The generation counts *changes to the record*, not runs: the sync, the
        // break and the make are three of them. What a master compares it to is
        // the digest, never a number it worked out for itself.
        assert_eq!(r.gen, n.digest().run_gen(Lane::L1));
        assert!(!r.gen.is_never());
        assert!(r.is_timing_valid());
        assert!(r.is_race());
        assert_eq!(r.inputs[0].break_at(), Some(Ticks(T_60FT)));
        assert_eq!(r.inputs[0].make_at(), Some(Ticks(T_60FT + CHORD)));
        assert_eq!(r.inputs[0].edge_count, 2);
        assert!(r.contributed(0));
    }

    #[test]
    fn an_input_that_never_broke_reads_as_not_seen() {
        // software.md §2: meaningless at this position is data, not an error.
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        let r = *n.run(Lane::L1);
        assert_eq!(r.inputs[1].break_at(), None);
        assert!(!r.contributed(1));
        // ...and the run is not complete, because input 1 is populated.
        assert!(!r.flags.complete());
    }

    #[test]
    fn complete_means_every_populated_input_broke() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        assert!(!n.run(Lane::L1).flags.complete());
        clean_pass(&mut n, Lane::L1, 1, T_60FT + 4_000);
        assert!(n.run(Lane::L1).flags.complete());
        assert!(n.digest().status.run_complete_l1());
        assert!(!n.digest().status.run_active());
    }

    #[test]
    fn a_second_break_on_one_input_sets_multi_edge_and_keeps_the_first() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        edge(&mut n, Lane::L1, 0, T_60FT + 9_000_000, EdgeKind::Break);
        let cap = n.run(Lane::L1).inputs[0];
        assert!(cap.flags.multi_edge());
        assert_eq!(
            cap.break_at(),
            Some(Ticks(T_60FT)),
            "the first break stands"
        );
        assert_eq!(cap.edge_count, 3);
    }

    // -- D16 ---------------------------------------------------------------

    #[test]
    fn a_bad_width_disowns_a_run_that_had_already_started() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        // The width arrives 5 ms after the counter started, and it is wrong.
        n.apply(Event::PulseWidth {
            lane: Lane::L1,
            width_us: 900,
        });

        let r = *n.run(Lane::L1);
        assert!(r.flags.valid(), "the counter did start");
        assert!(r.flags.invalidated(), "and the pulse was then disowned");
        assert!(!r.is_timing_valid());
        assert!(n.digest().status.pulse_invalid_l1());
        // The split is still readable — the master decides what to do with it.
        assert_eq!(r.inputs[0].break_at(), Some(Ticks(T_60FT)));
    }

    #[test]
    fn a_width_near_the_threshold_warns_before_it_rejects() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        n.apply(Event::PulseWidth {
            lane: Lane::L1,
            width_us: 5_450, // inside the band, in its last 20 %
        });
        let p = n.pulse_observation();
        assert!(p.width_valid(Lane::L1));
        assert!(p.width_marginal(Lane::L1));
        assert!(!n.run(Lane::L1).flags.invalidated());
        assert_eq!(p.width_us(Lane::L1), 5_450);
    }

    // -- D20 / D25 ---------------------------------------------------------

    #[test]
    fn the_margin_needs_both_pulses_and_carries_a_sign() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 250_000);
        assert_eq!(
            n.pulse_observation().launch_margin(),
            None,
            "one pulse is not a margin, and 0 would read as a dead heat"
        );

        // Lane 2 left 240,000 ticks (3 ms) earlier on the common timer, so the
        // margin is negative — and crossing order, not ET, decides the race.
        pulse(&mut n, Lane::L2, 10_000);
        let p = n.pulse_observation();
        assert!(p.seen(Lane::L1) && p.seen(Lane::L2));
        assert_eq!(p.launch_margin(), Some(TickDelta(-240_000)));
    }

    #[test]
    fn generations_advance_per_lane_and_independently() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        pulse(&mut n, Lane::L1, 1);
        pulse(&mut n, Lane::L2, 2);
        assert_eq!(n.run_gen(Lane::L1), Generation::from_raw(2));
        assert_eq!(n.run_gen(Lane::L2), Generation::from_raw(1));
    }

    #[test]
    fn a_result_latches_until_the_next_pulse_replaces_it() {
        // D25: a poll arriving seconds late reads exactly the same numbers.
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        let first = *n.run(Lane::L1);
        for _ in 0..1_000 {
            n.apply(Event::Second);
        }
        assert_eq!(*n.run(Lane::L1), first, "nothing decays with time");

        let before = n.run(Lane::L1).gen;
        pulse(&mut n, Lane::L1, 5_000);
        assert_eq!(n.run(Lane::L1).inputs[0].break_at(), None);
        assert!(n.run(Lane::L1).gen.changed_from(before));
    }

    #[test]
    fn the_generation_moves_when_a_beam_lands_not_only_when_the_run_starts() {
        // The regression this exists for cost a whole round: advancing only on
        // the sync leaves the master reading a record that is valid, current and
        // empty. The pulse arrives at the launch; the finish beam is crossed ten
        // seconds later; nothing in the four registers polled every cycle says
        // so, and `run_complete` cannot say it either on a node shared between
        // lanes (§8 #7). This bit of the digest is the only signal there is.
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        let at_launch = n.digest().run_gen(Lane::L1);
        assert_eq!(
            n.run(Lane::L1).input_mask,
            0,
            "nothing has been crossed yet"
        );

        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        assert!(
            n.digest().run_gen(Lane::L1).changed_from(at_launch),
            "a master polling the digest must be told the record filled in"
        );
        // And the record carries the same generation, so a read is self-checking:
        // if it comes back older than the digest, more has landed since.
        assert_eq!(n.run(Lane::L1).gen, n.digest().run_gen(Lane::L1));
    }

    // -- D17 ---------------------------------------------------------------

    #[test]
    fn input_state_reads_a_broken_beam_as_a_zero_bit() {
        let mut n = node();
        assert_eq!(n.digest().input_state, 0b0011, "boots with beams intact");
        assert!(n.digest().beam_intact(0));

        pulse(&mut n, Lane::L1, 0);
        edge(&mut n, Lane::L1, 0, T_60FT, EdgeKind::Break);
        assert!(n.digest().beam_broken(0));
        edge(&mut n, Lane::L1, 0, T_60FT + CHORD, EdgeKind::Make);
        assert!(n.digest().beam_intact(0));
        // An unpopulated input reads broken, which is loud on purpose.
        assert!(n.digest().beam_broken(2));
    }

    // -- address decoding, driven by the map -------------------------------

    #[test]
    fn the_digest_reads_at_the_documented_address() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        let mut w = [0u16; 4];
        n.read(Digest::ADDR, 4, &mut w).unwrap();
        assert_eq!(w[0], 1, "run_gen_l1");
        assert_eq!(w[3], 0b0011, "input_state");
        assert_eq!(Digest::decode(&w).unwrap(), n.digest());
    }

    #[test]
    fn a_gap_between_blocks_is_exception_02() {
        // protocol.md §2: unimplemented addresses return illegal data address.
        let n = node();
        let mut w = [0u16; 2];
        assert_eq!(
            n.read(0x0004, 2, &mut w),
            Err(Exception::IllegalDataAddress)
        );
        // ...and so does a range that starts inside a block and runs off its end.
        let mut w = [0u16; 8];
        assert_eq!(
            n.read(Digest::ADDR, 8, &mut w),
            Err(Exception::IllegalDataAddress)
        );
    }

    #[test]
    fn the_tree_block_exists_only_on_a_tree() {
        let mut w = [0u16; Tree::LEN as usize];
        assert_eq!(
            node().read(Tree::ADDR, Tree::LEN, &mut w),
            Err(Exception::IllegalDataAddress),
            "a timing node does not implement 0x00C0"
        );

        let mut tree = NodeCore::new(Config::tree(10, 0xAABB_CCDD_EEFF));
        *tree.tree_mut() = Tree::default().with_reaction_times(-40_000, 36_000);
        tree.read(Tree::ADDR, Tree::LEN, &mut w).unwrap();
        let t = Tree::decode(&w).unwrap();
        assert!(t.is_red(Lane::L1));
        assert!(!t.is_red(Lane::L2));
    }

    #[test]
    fn each_lane_record_reads_at_its_own_stride() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        pulse(&mut n, Lane::L2, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        clean_pass(&mut n, Lane::L2, 1, T_60FT + 500_000);

        let mut w = [0u16; RunRecord::LEN as usize];
        n.read(RunRecord::addr(Lane::L1), RunRecord::LEN, &mut w)
            .unwrap();
        assert_eq!(
            RunRecord::decode(&w).unwrap().inputs[0].break_at(),
            Some(Ticks(T_60FT))
        );

        n.read(RunRecord::addr(Lane::L2), RunRecord::LEN, &mut w)
            .unwrap();
        assert_eq!(
            RunRecord::decode(&w).unwrap().inputs[1].break_at(),
            Some(Ticks(T_60FT + 500_000))
        );
    }

    #[test]
    fn identity_reports_what_the_mapping_validator_checks_against() {
        let n = node();
        let mut w = [0u16; Identity::LEN as usize];
        n.read(Identity::ADDR, Identity::LEN, &mut w).unwrap();
        let id = Identity::decode(&w).unwrap();
        assert_eq!(id.protocol_version, PROTOCOL_VERSION);
        assert_eq!(id.dip_address, 3);
        assert_eq!(id.mac, 0x7CDF_A100_1122);
        assert_eq!(id.input_present, 0b0011);
        assert!(id.dip_valid());
    }

    // -- commands ----------------------------------------------------------

    fn command(n: &mut NodeCore, opcode: Opcode, arg0: u16, arg1: u16, seq: u16) {
        let cmd = Command {
            opcode,
            arg0,
            arg1,
            seq,
        };
        let mut w = [0u16; Command::LEN as usize];
        cmd.encode(&mut w).unwrap();
        n.write(Command::ADDR, &w).unwrap();
    }

    #[test]
    fn a_command_is_confirmed_by_the_echo_not_the_write() {
        let mut n = node();
        command(&mut n, Opcode::ClearFaults, 0, 0, 7);
        let mut w = [0u16; Status::LEN as usize];
        n.read(Status::ADDR, Status::LEN, &mut w).unwrap();
        let s = Status::decode(&w).unwrap();
        assert_eq!(s.command_seq_echo, 7);
        assert_eq!(s.command_status, CommandStatus::Accepted);
    }

    #[test]
    fn retrying_a_write_with_the_same_sequence_does_not_run_it_twice() {
        // The bus is noisy and writes get retried; that must be safe.
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        command(&mut n, Opcode::ClearRun, 0b01, 0, 3);
        assert_eq!(n.run_gen(Lane::L1), Generation::NEVER);

        pulse(&mut n, Lane::L1, 9_000);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        command(&mut n, Opcode::ClearRun, 0b01, 0, 3); // the retry
        assert_eq!(
            n.run(Lane::L1).inputs[0].break_at(),
            Some(Ticks(T_60FT)),
            "a retried clear must not eat the new run"
        );
    }

    #[test]
    fn a_self_test_result_carries_synthetic_and_only_once() {
        let mut n = node();
        command(&mut n, Opcode::SelfTest, 1_000, 0, 1);
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        assert!(!n.run(Lane::L1).is_race());

        pulse(&mut n, Lane::L1, 1);
        assert!(n.run(Lane::L1).is_race(), "the next run is a race again");
    }

    #[test]
    fn a_reboot_invalidates_what_the_master_was_holding() {
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        clean_pass(&mut n, Lane::L1, 0, T_60FT);
        let before = n.digest();
        n.reboot();

        let mut w = [0u16; Status::LEN as usize];
        n.read(Status::ADDR, Status::LEN, &mut w).unwrap();
        let s = Status::decode(&w).unwrap();
        assert_eq!(s.boot_count, 2);
        assert!(s.faults.unexpected_reset());
        assert!(before.run_gen_l1.changed_from(n.digest().run_gen_l1));
        assert!(n.digest().run_gen_l1.is_never());
    }

    #[test]
    fn a_partial_command_write_is_refused() {
        // The sequence number is the last register on purpose: half a command is
        // not a command.
        let mut n = node();
        assert_eq!(
            n.write(Command::ADDR, &[Opcode::ClearFaults.raw(), 0]),
            Err(Exception::IllegalDataAddress)
        );
        assert_eq!(
            n.write(Digest::ADDR, &[0, 0, 0, 0]),
            Err(Exception::IllegalDataAddress),
            "nothing but the command block is writable"
        );
    }

    #[test]
    fn an_unknown_opcode_is_rejected_rather_than_ignored() {
        let mut n = node();
        command(&mut n, Opcode::Unknown(99), 0, 0, 4);
        let mut w = [0u16; Status::LEN as usize];
        n.read(Status::ADDR, Status::LEN, &mut w).unwrap();
        assert_eq!(
            Status::decode(&w).unwrap().command_status,
            CommandStatus::Rejected
        );
    }

    #[test]
    fn a_tree_command_is_rejected_on_a_timing_node() {
        let mut n = node();
        command(&mut n, Opcode::TreeArm, 0, 700, 5);
        let mut w = [0u16; Status::LEN as usize];
        n.read(Status::ADDR, Status::LEN, &mut w).unwrap();
        assert_eq!(
            Status::decode(&w).unwrap().command_status,
            CommandStatus::Rejected
        );
    }

    // -- the raw log -------------------------------------------------------

    #[test]
    fn reading_the_log_twice_returns_the_same_page() {
        // A read-advancing cursor makes a retried read return different data,
        // which is exactly what a noisy bus produces.
        let mut n = node();
        pulse(&mut n, Lane::L1, 0);
        for i in 0..20u32 {
            edge(&mut n, Lane::L1, 0, T_60FT + i * 1_000, EdgeKind::Break);
        }
        let mut a = [0u16; LogPage::LEN as usize];
        let mut b = [0u16; LogPage::LEN as usize];
        n.read(LogPage::ADDR, LogPage::LEN, &mut a).unwrap();
        n.read(LogPage::ADDR, LogPage::LEN, &mut b).unwrap();
        assert_eq!(a, b);

        let first = LogPage::decode(&a).unwrap();
        assert_eq!(first.records[0].t_ms, Millis(T_60FT / 80_000));

        command(&mut n, Opcode::LogSeek, 0, 16, 11);
        n.read(LogPage::ADDR, LogPage::LEN, &mut b).unwrap();
        assert_ne!(a, b, "an explicit seek is what moves the cursor");
    }
}
