//! The map's `poll` column, restated as a checklist.
//!
//! `REGISTER_MAP` says when each block is due; [`Poller::cycle`](crate::Poller::cycle)
//! is where that happens. The two cannot be one expression — decoding needs a
//! concrete type — so what stands between them is this table plus the test below.
//!
//! Its whole job is to be a tripwire: adding a block to the map without deciding
//! what the poller does with it fails `the_poller_covers_the_map`, instead of
//! shipping a register that nothing ever reads. That is a smaller claim than "the
//! map drives the schedule", and it is the true one.

use beam402_protocol::blocks::Poll;
use beam402_protocol::{
    Block, Command, Digest, Identity, LogPage, PulseObservation, RunRecord, Status, Telemetry, Tree,
};

/// One block's place in the cycle.
#[derive(Clone, Copy, Debug)]
pub struct Scheduled {
    pub block: &'static str,
    /// The map's own value, held against it by the test below.
    pub poll: Poll,
    /// Where in the cycle it is read, in words — this is what the operator panel
    /// and a reviewer actually want to know.
    pub site: &'static str,
}

const fn s(block: &'static str, poll: Poll, site: &'static str) -> Scheduled {
    Scheduled { block, poll, site }
}

pub const SCHEDULE: &[Scheduled] = &[
    s(Digest::NAME, Poll::EveryCycle, "visit: unconditional"),
    s(
        Identity::NAME,
        Poll::Once,
        "visit: when none is held — first contact, and after a reset",
    ),
    s(
        Status::NAME,
        Poll::OnFaultOrSlowRotation,
        "visit: on fault_present, a reset, or an unconfirmed command; rotate: every few sweeps",
    ),
    s(Telemetry::NAME, Poll::RoundRobin, "rotate: one per cycle"),
    s(
        PulseObservation::NAME,
        Poll::OnGenerationChange,
        "visit: alongside a run record — the pulse is what moved the generation",
    ),
    s(
        RunRecord::NAME,
        Poll::OnGenerationChange,
        "visit: one transaction per lane whose digest generation moved",
    ),
    s(
        Tree::NAME,
        Poll::OnGenerationChange,
        "visit: every cycle while watched, because no digest bit carries sequence_gen",
    ),
    s(
        Command::NAME,
        Poll::Write,
        "send_pending: written, never polled",
    ),
    s(
        LogPage::NAME,
        Poll::OnRequest,
        "not in the loop at all — dispute evidence, fetched after a round",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_protocol::REGISTER_MAP;

    #[test]
    fn the_poller_covers_the_map() {
        for desc in REGISTER_MAP {
            let entry = SCHEDULE
                .iter()
                .find(|e| e.block == desc.name)
                .unwrap_or_else(|| {
                    panic!(
                        "block {:?} is in the register map and the poller does nothing with it",
                        desc.name
                    )
                });
            assert_eq!(
                entry.poll, desc.poll,
                "the poller treats {:?} as {:?}, the map says {:?}",
                desc.name, entry.poll, desc.poll
            );
            assert!(!entry.site.is_empty(), "{:?} needs a site", desc.name);
        }
        assert_eq!(
            SCHEDULE.len(),
            REGISTER_MAP.len(),
            "the poller lists a block the map does not have"
        );
    }
}
