//! The bus, behind an interface (**D26**).
//!
//! One trait, two implementations: a node simulator now, Modbus RTU over serial
//! when there is something to plug in. Everything above this line — the poller,
//! the race logic, the results — never learns which one it is talking to, and a
//! recorded session replays against either.
//!
//! ## Why the seam is at *words*, not bytes and not structs
//!
//! [`Bus`] moves `u16` registers, because that is exactly what both real stacks
//! hand over: `tokio-modbus` returns `Vec<u16>` and the ESP-IDF slave works on a
//! `uint16_t` area. Byte order inside a register, framing and CRC live below this
//! line and are the transport's business.
//!
//! Putting the seam there buys the one property `protocol.md` §2 demands and a
//! struct-level seam would quietly lose: **one call is one transaction.** A run
//! record is 28 words in a single [`Bus::read`], so it cannot be assembled from
//! two reads that straddle a new run.
//!
//! [`BusExt`] then decodes into the typed blocks, which is what callers actually
//! use. Two levels, and the boundary between them is the transaction.
//!
//! ## Why it is synchronous
//!
//! The trunk is half-duplex with a single master, and only the polled node
//! transmits — collisions are impossible by discipline (**D05**). There is
//! therefore nothing to await concurrently: a second request cannot be in flight.
//! An async trait would buy no parallelism and cost `dyn` compatibility, so the
//! poll loop blocks on its own thread and hands results to the web side by
//! channel.

use beam402_protocol::blocks::{Block, Command, DecodeError, RunRecord};
use beam402_protocol::Lane;

/// Widest block in the map, so a transaction fits on the stack.
const MAX_BLOCK: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusError {
    /// No answer within the response timeout, after the configured retries. The
    /// node is marked silent and surfaces on the operator panel — free liveness
    /// monitoring, every poll cycle.
    Timeout,
    /// The device answered with a Modbus exception. 02 means the range covers
    /// something it does not implement.
    Exception(u8),
    /// The device answered with the wrong number of registers.
    ShortFrame { asked: u16, got: usize },
    /// The registers arrived but the block layer rejected them.
    Decode(DecodeError),
    /// The transport itself failed — a closed port, a vanished simulator.
    Transport,
}

impl core::fmt::Display for BusError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BusError::Timeout => write!(f, "no answer"),
            BusError::Exception(c) => write!(f, "modbus exception {c}"),
            BusError::ShortFrame { asked, got } => {
                write!(f, "asked for {asked} registers, got {got}")
            }
            BusError::Decode(e) => write!(f, "decode: {e}"),
            BusError::Transport => write!(f, "transport failure"),
        }
    }
}

impl From<DecodeError> for BusError {
    fn from(e: DecodeError) -> Self {
        BusError::Decode(e)
    }
}

/// One Modbus transaction, in each direction.
///
/// Retries, the response timeout and the inter-frame silence are the
/// implementation's business — a caller sees either registers or [`BusError`].
pub trait Bus {
    /// FC3. `out.len()` is the register count.
    fn read(&mut self, address: u8, reg: u16, out: &mut [u16]) -> Result<(), BusError>;

    /// FC6 / FC16. Retrying with an unchanged `command_seq` is safe, which is
    /// what makes a lost acknowledgement harmless.
    fn write(&mut self, address: u8, reg: u16, values: &[u16]) -> Result<(), BusError>;
}

/// The typed surface. Blanket-implemented, so every [`Bus`] gets it.
pub trait BusExt: Bus {
    /// Read a whole block in one transaction and decode it.
    fn block<B: Block>(&mut self, address: u8) -> Result<B, BusError> {
        let len = B::LEN as usize;
        debug_assert!(len <= MAX_BLOCK, "{} is wider than a transaction", B::NAME);
        let mut buf = [0u16; MAX_BLOCK];
        self.read(address, B::ADDR, &mut buf[..len])?;
        Ok(B::decode(&buf[..len])?)
    }

    /// A lane's run record — the one read `protocol.md` §2 insists must not be
    /// split, which here it structurally cannot be.
    fn run_record(&mut self, address: u8, lane: Lane) -> Result<RunRecord, BusError> {
        let len = RunRecord::LEN as usize;
        let mut buf = [0u16; MAX_BLOCK];
        self.read(address, RunRecord::addr(lane), &mut buf[..len])?;
        Ok(RunRecord::decode(&buf[..len])?)
    }

    /// Write a command. It is confirmed by a later read of `command_seq_echo`,
    /// not by this call returning.
    fn command(&mut self, address: u8, cmd: Command) -> Result<(), BusError> {
        let mut buf = [0u16; Command::LEN as usize];
        cmd.encode(&mut buf)?;
        self.write(address, Command::ADDR, &buf)
    }
}

impl<T: Bus + ?Sized> BusExt for T {}
