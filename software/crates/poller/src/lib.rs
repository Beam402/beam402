#![forbid(unsafe_code)]

//! Poll for change, read on change.
//!
//! The poller is the last impure layer: it owns the bus, and it turns registers
//! into the event stream that `software.md` §4 makes race logic a pure function
//! of. It contains no race logic itself — no ET, no staging state machine, no
//! idea which beam is the finish. It knows addresses, generations and a schedule.
//!
//! ## The schedule, and the arithmetic behind it
//!
//! 19,200 bps 8N1 is 1,920 characters per second, so a 100 ms cycle buys about
//! 192 characters *for the entire bus*. A two-lane run record is 28 registers per
//! lane, ~40 ms for one device — seven devices of that is over half a second. So
//! the steady-state loop reads the four-register digest and nothing else, and
//! pulls a record only when that lane's generation moves (**D25**).
//!
//! This costs nothing, because records latch and there is nowhere to be late to.
//! A run lasts 10–20 s and the next pair stages for minutes; the unhurried moment
//! to read a result is exactly the moment after it exists. [`CycleStats`] prices
//! each cycle so that claim stays checkable rather than remembered.
//!
//! ## What it will not do
//!
//! - **It does not sleep.** The caller paces cycles. That keeps the poll loop a
//!   deterministic function of the bus it is handed, which is what lets a
//!   recorded session replay against the simulator and produce the same events.
//! - **It does not retry behind the operator's back.** Retries belong to the
//!   transport (`LINK.retries`); a command that goes unconfirmed is reported as
//!   lost, because a write that may or may not have run is not something to
//!   quietly repeat.
//! - **It does not interpret.** `input_state` bit 2 leaves here as bit 2.

use std::collections::BTreeMap;

use beam402_bus::{Bus, BusError, BusExt};
use beam402_protocol::map::PROTOCOL_VERSION;
use beam402_protocol::{
    Block, Command, CommandStatus, DeviceClass, Digest, Generation, Identity, Lane, Opcode,
    PulseObservation, RunRecord, Status, Telemetry, Tree,
};

mod event;
mod schedule;

pub use event::{CycleStats, Event, ResetEvidence};
pub use schedule::{Scheduled, SCHEDULE};

/// Whether the master is allowed to talk.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Live,
    /// Both cars staged. **Nothing leaves the master** until it is lifted.
    ///
    /// `architecture.md` §3's quiet window. The launch is the noisiest instant
    /// the system has — full ignition energy metres from the start node — and
    /// pulse width validation rejects a spike but not a sustained transmission
    /// burst. The bus is therefore silent across exactly the interval where noise
    /// would cost a run, which it can afford because everything latches.
    Quiet,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Consecutive failed cycles before a device is declared silent. The
    /// transport has already retried inside a single call, so this sits on top of
    /// `LINK.retries`: one frame lost to ignition noise must not blank a device
    /// off the operator panel.
    pub misses_before_silent: u32,
    /// Full rotations between status reads for any one device. Faults do not wait
    /// for this — `status_flags.fault_present` in the digest pulls the block
    /// immediately.
    pub status_every_sweeps: u64,
    /// Cycles a written command may go unconfirmed before it is declared lost.
    pub command_deadline_cycles: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            misses_before_silent: 2,
            status_every_sweeps: 4,
            command_deadline_cycles: 10,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    cmd: Command,
    sent: bool,
    age: u32,
}

#[derive(Clone, Debug, Default)]
struct Device {
    identity: Option<Identity>,
    /// False once the device answers a protocol version this build does not
    /// implement. It is still polled — an unusable node is not an absent one.
    usable: bool,
    digest: Option<Digest>,
    /// Generations already fetched. Compared for **inequality only**: `Generation`
    /// is deliberately not `Ord` (**D25**).
    run_gen: [Generation; 2],
    boot_count: Option<u16>,
    tree: Option<Tree>,
    misses: u32,
    silent: bool,
    /// Read the tree block every cycle — see [`Poller::send`].
    watch_tree: bool,
    want_status: bool,
    refetch: bool,
    pending: Option<Pending>,
    next_seq: u16,
}

impl Device {
    fn new() -> Self {
        Device {
            next_seq: 1,
            ..Device::default()
        }
    }

    /// Everything held for this address is stale. Identity goes too, so the next
    /// cycle re-reads it and a node that came back on different firmware is not
    /// mistaken for the one that left.
    fn invalidate(&mut self) {
        self.identity = None;
        self.usable = false;
        self.digest = None;
        self.run_gen = [Generation::NEVER; 2];
        self.tree = None;
        self.want_status = true;
    }
}

/// A device as the operator panel needs it.
#[derive(Clone, Copy, Debug)]
pub struct DeviceView {
    pub address: u8,
    pub identity: Option<Identity>,
    pub digest: Option<Digest>,
    pub silent: bool,
    /// Read together with `identity`: `None` and not usable is "not identified
    /// yet", `Some` and not usable is "refused — unknown protocol version".
    pub usable: bool,
}

pub struct Poller {
    order: Vec<u8>,
    devices: BTreeMap<u8, Device>,
    phase: Phase,
    rotation: usize,
    sweeps: u64,
    cfg: Config,
}

impl Poller {
    pub fn new(addresses: impl IntoIterator<Item = u8>) -> Self {
        Poller::with_config(addresses, Config::default())
    }

    pub fn with_config(addresses: impl IntoIterator<Item = u8>, cfg: Config) -> Self {
        let mut order = Vec::new();
        let mut devices = BTreeMap::new();
        for a in addresses {
            if devices.insert(a, Device::new()).is_none() {
                order.push(a);
            }
        }
        Poller {
            order,
            devices,
            phase: Phase::Live,
            rotation: 0,
            sweeps: 0,
            cfg,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Enter or leave the quiet window. Leaving it makes the next cycle a fat
    /// one — every device whose generation moved while the master was silent
    /// reports in it, and the rotations stand aside for it. That is the intended
    /// shape, not a hiccup.
    pub fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
    }

    pub fn addresses(&self) -> &[u8] {
        &self.order
    }

    pub fn device(&self, address: u8) -> Option<DeviceView> {
        self.devices.get(&address).map(|d| DeviceView {
            address,
            identity: d.identity,
            digest: d.digest,
            silent: d.silent,
            usable: d.usable,
        })
    }

    /// Re-read this device's records on the next cycle even though nothing
    /// changed — the operator's "read it again", and the way a run latched before
    /// the master started is recovered. First contact deliberately *adopts* the
    /// generations it finds instead of reporting them as runs: arriving is not a
    /// change.
    pub fn refetch(&mut self, address: u8) {
        if let Some(d) = self.devices.get_mut(&address) {
            d.refetch = true;
        }
    }

    /// Queue a command. It goes out at the top of the next live cycle and is
    /// confirmed by a later read of `command_seq_echo`, never by the write
    /// returning (`protocol.md` §2).
    ///
    /// `None` means a command is already outstanding for that address: the node
    /// has one command register, so there is nowhere to put a second.
    pub fn send(&mut self, address: u8, opcode: Opcode, arg0: u16, arg1: u16) -> Option<u16> {
        let dev = self.devices.get_mut(&address)?;
        if dev.pending.is_some() {
            return None;
        }
        let seq = dev.next_seq;
        // Zero is "nothing has been written since boot", so it is skipped on wrap
        // exactly as a generation is.
        dev.next_seq = if seq == u16::MAX { 1 } else { seq + 1 };
        dev.pending = Some(Pending {
            cmd: Command {
                opcode,
                arg0,
                arg1,
                seq,
            },
            sent: false,
            age: 0,
        });
        match opcode {
            // The tree block carries a `sequence_gen`, but the digest does not,
            // so no cheap read tells the master the tree moved (`software.md`
            // §8 #9). Until that is settled the block is watched outright from
            // the arm onward: 13 registers on one device, ~9 ms, only during a
            // round.
            Opcode::TreeArm => dev.watch_tree = true,
            Opcode::TreeAbort => dev.watch_tree = false,
            _ => {}
        }
        Some(seq)
    }

    /// Stop reading the tree block every cycle. The round is over; the caller
    /// knows that and the poller does not.
    pub fn release_tree(&mut self, address: u8) {
        if let Some(d) = self.devices.get_mut(&address) {
            d.watch_tree = false;
        }
    }

    /// One pass over the bus. Returns what it cost; appends what it learned.
    pub fn cycle<B: Bus + ?Sized>(&mut self, bus: &mut B, out: &mut Vec<Event>) -> CycleStats {
        let mut st = CycleStats::default();
        if self.phase == Phase::Quiet {
            return st;
        }

        // Commands first. An arm that waits its turn in the rotation is an arm
        // that arrives late.
        for i in 0..self.order.len() {
            self.send_pending(bus, self.order[i], &mut st);
        }

        let mut fetched = 0u32;
        for i in 0..self.order.len() {
            let address = self.order[i];
            fetched += self.visit(bus, address, out, &mut st);
            // Outside `visit`, because a device that has gone silent still owes an
            // answer for a command written to it — otherwise the one case where a
            // command is most likely to be lost is the one case that never
            // reports it.
            self.confirm(address, out);
        }

        // §4's "until every node has reported": a cycle that pulled results does
        // not also pull telemetry. Records and the rotation never compete for the
        // bus in the seconds after a run.
        if fetched == 0 && !self.order.is_empty() {
            let address = self.order[self.rotation % self.order.len()];
            self.rotate(bus, address, out, &mut st);
            self.rotation += 1;
            if self.rotation % self.order.len() == 0 {
                self.sweeps += 1;
            }
        }
        st
    }

    fn send_pending<B: Bus + ?Sized>(&mut self, bus: &mut B, address: u8, st: &mut CycleStats) {
        let Some(dev) = self.devices.get_mut(&address) else {
            return;
        };
        let Some(pending) = dev.pending.as_mut() else {
            return;
        };
        if pending.sent {
            return;
        }
        let cmd = pending.cmd;
        pending.sent = true;
        dev.want_status = true;
        st.writes += 1;
        st.registers += Command::LEN as u32;
        if bus.command(address, cmd).is_err() {
            st.timeouts += 1;
            // Left pending: the deadline in `confirm` decides whether it is lost,
            // and it is not re-sent, because a write that may already have run is
            // not something to repeat quietly.
        }
    }

    /// Everything one device owes the master this cycle. Returns how many blocks
    /// were pulled because something changed.
    fn visit<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        address: u8,
        out: &mut Vec<Event>,
        st: &mut CycleStats,
    ) -> u32 {
        let digest = match read::<B, Digest>(bus, address, st) {
            Ok(d) => d,
            Err(e) => {
                self.miss(address, e, out);
                return 0;
            }
        };
        self.hit(address, out);

        let dev = self.devices.get_mut(&address).expect("visited own address");
        let first_contact = dev.digest.is_none() && dev.identity.is_none();

        // A generation that was moving and is now NEVER is a restart, and it is
        // the only evidence that rides in the four-register digest.
        let mut reset = false;
        for lane in Lane::ALL {
            let held = dev.run_gen[lane.ord() as usize];
            if digest.run_gen(lane) == Generation::NEVER && held != Generation::NEVER {
                reset = true;
            }
        }
        if reset {
            dev.invalidate();
            out.push(Event::Reset {
                address,
                evidence: ResetEvidence::GenerationCleared,
            });
        }

        if dev.identity.is_none() {
            match read::<B, Identity>(bus, address, st) {
                Ok(identity) => {
                    let dev = self.devices.get_mut(&address).expect("visited own address");
                    dev.identity = Some(identity);
                    dev.usable = identity.protocol_version == PROTOCOL_VERSION;
                    if dev.usable {
                        out.push(Event::Identified { address, identity });
                    } else {
                        out.push(Event::Unsupported {
                            address,
                            protocol_version: identity.protocol_version,
                        });
                    }
                }
                Err(error) => out.push(Event::ReadFailed {
                    address,
                    block: Identity::NAME,
                    error,
                }),
            }
        }

        let dev = self.devices.get_mut(&address).expect("visited own address");
        if dev.digest != Some(digest) {
            dev.digest = Some(digest);
            out.push(Event::Digest { address, digest });
        }
        if digest.status.fault_present() {
            dev.want_status = true;
        }
        if !dev.usable {
            // Alive, on the panel, and never read for timing.
            return 0;
        }

        let refetch = std::mem::take(&mut dev.refetch);
        let mut lanes = Vec::new();
        for lane in Lane::ALL {
            let held = dev.run_gen[lane.ord() as usize];
            let moved = digest.run_gen(lane).changed_from(held);
            dev.run_gen[lane.ord() as usize] = digest.run_gen(lane);
            // First contact adopts what it finds — and so does a device coming
            // back from silence, because `invalidate` cleared both. The master
            // did not miss a run; it was not there, and what happened across an
            // outage of unknown length is not attributable to it. The result is
            // not lost: it is latched, and `refetch` is how it is recovered.
            if (moved && !first_contact && digest.run_gen(lane) != Generation::NEVER) || refetch {
                lanes.push(lane);
            }
        }

        let mut fetched = 0;
        for lane in lanes {
            match read_run(bus, address, lane, st) {
                Ok(record) => {
                    fetched += 1;
                    out.push(Event::Run {
                        address,
                        lane,
                        record,
                    });
                }
                Err(error) => out.push(Event::ReadFailed {
                    address,
                    block: RunRecord::NAME,
                    error,
                }),
            }
        }

        // One pulse read serves both lanes. It is due exactly when a record is:
        // the pulse *is* what advances the run generation, so the digest's change
        // is the pulse block's change signal as well.
        if fetched > 0 {
            match read::<B, PulseObservation>(bus, address, st) {
                Ok(observation) => out.push(Event::Pulse {
                    address,
                    observation,
                }),
                Err(error) => out.push(Event::ReadFailed {
                    address,
                    block: PulseObservation::NAME,
                    error,
                }),
            }
        }

        let dev = self.devices.get_mut(&address).expect("visited own address");
        let is_tree = dev.identity.map(|i| i.device_class) == Some(DeviceClass::TreeModule);
        if dev.watch_tree && is_tree {
            match read::<B, Tree>(bus, address, st) {
                Ok(tree) => {
                    let dev = self.devices.get_mut(&address).expect("visited own address");
                    if dev.tree != Some(tree) {
                        dev.tree = Some(tree);
                        out.push(Event::Tree { address, tree });
                    }
                }
                Err(error) => out.push(Event::ReadFailed {
                    address,
                    block: Tree::NAME,
                    error,
                }),
            }
        }

        if self.devices[&address].want_status {
            self.read_status(bus, address, out, st);
        }
        fetched
    }

    /// The round-robin half of the schedule: telemetry for one device per cycle,
    /// status for the same device every few sweeps.
    fn rotate<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        address: u8,
        out: &mut Vec<Event>,
        st: &mut CycleStats,
    ) {
        let Some(dev) = self.devices.get(&address) else {
            return;
        };
        if dev.silent || !dev.usable {
            return;
        }
        match read::<B, Telemetry>(bus, address, st) {
            Ok(telemetry) => out.push(Event::Telemetry { address, telemetry }),
            Err(error) => out.push(Event::ReadFailed {
                address,
                block: Telemetry::NAME,
                error,
            }),
        }
        if self.sweeps % self.cfg.status_every_sweeps == 0 {
            self.read_status(bus, address, out, st);
        }
    }

    fn read_status<B: Bus + ?Sized>(
        &mut self,
        bus: &mut B,
        address: u8,
        out: &mut Vec<Event>,
        st: &mut CycleStats,
    ) {
        let status = match read::<B, Status>(bus, address, st) {
            Ok(s) => s,
            Err(error) => {
                out.push(Event::ReadFailed {
                    address,
                    block: Status::NAME,
                    error,
                });
                return;
            }
        };
        let dev = self.devices.get_mut(&address).expect("known address");
        dev.want_status = false;
        match dev.boot_count {
            Some(held) if held != status.boot_count => {
                dev.invalidate();
                dev.boot_count = Some(status.boot_count);
                out.push(Event::Reset {
                    address,
                    evidence: ResetEvidence::BootCount {
                        from: held,
                        to: status.boot_count,
                    },
                });
            }
            _ => dev.boot_count = Some(status.boot_count),
        }
        // Held so `confirm` can settle a command against it without a second read.
        if let Some(p) = dev.pending.as_ref() {
            if p.sent && status.command_seq_echo == p.cmd.seq && status.command_status.is_settled()
            {
                let opcode = p.cmd.opcode;
                let result = status.command_status;
                dev.pending = None;
                if result == CommandStatus::Rejected {
                    dev.watch_tree = false;
                }
                out.push(Event::Commanded {
                    address,
                    opcode,
                    status: result,
                });
            }
        }
        out.push(Event::Status { address, status });
    }

    /// Age an unconfirmed command and give up loudly rather than silently.
    fn confirm(&mut self, address: u8, out: &mut Vec<Event>) {
        let dev = self.devices.get_mut(&address).expect("known address");
        let Some(p) = dev.pending.as_mut() else {
            return;
        };
        p.age += 1;
        if p.age < self.cfg.command_deadline_cycles {
            dev.want_status = true;
            return;
        }
        let opcode = p.cmd.opcode;
        dev.pending = None;
        dev.watch_tree = false;
        out.push(Event::CommandLost { address, opcode });
    }

    fn miss(&mut self, address: u8, error: BusError, out: &mut Vec<Event>) {
        let dev = self.devices.get_mut(&address).expect("known address");
        dev.misses += 1;
        if !dev.silent && dev.misses >= self.cfg.misses_before_silent {
            dev.silent = true;
            out.push(Event::Silent { address, error });
        }
    }

    fn hit(&mut self, address: u8, out: &mut Vec<Event>) {
        let dev = self.devices.get_mut(&address).expect("known address");
        dev.misses = 0;
        if dev.silent {
            dev.silent = false;
            // Whatever it was holding is unverifiable across an outage of unknown
            // length, so it is re-established rather than resumed.
            dev.invalidate();
            out.push(Event::Returned { address });
        }
    }
}

fn read<B: Bus + ?Sized, T: Block>(
    bus: &mut B,
    address: u8,
    st: &mut CycleStats,
) -> Result<T, BusError> {
    st.reads += 1;
    st.registers += T::LEN as u32;
    bus.block::<T>(address).inspect_err(|e| {
        if *e == BusError::Timeout {
            st.timeouts += 1;
        }
    })
}

/// The one read `protocol.md` §2 insists must not be split. It is a separate
/// function only because [`BusExt::run_record`] is where that guarantee lives.
fn read_run<B: Bus + ?Sized>(
    bus: &mut B,
    address: u8,
    lane: Lane,
    st: &mut CycleStats,
) -> Result<RunRecord, BusError> {
    st.reads += 1;
    st.registers += RunRecord::LEN as u32;
    bus.run_record(address, lane).inspect_err(|e| {
        if *e == BusError::Timeout {
            st.timeouts += 1;
        }
    })
}
