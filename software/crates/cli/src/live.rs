//! A round that is happening now, with people watching it.
//!
//! Two threads and one rule between them. The **bus thread** is the only thing
//! that touches the bus — **D05** allows exactly one master, and a lock is not a
//! master. Clients never poll a node, never write a register and never call the
//! race logic; they post an *intent*, the bus thread drains it on its next
//! cycle, and everything anybody reads is a snapshot that thread published.
//!
//! ## The control token
//!
//! **D30**: several people can act, and two of them arming is worse than
//! neither. Exactly one client holds control at a time, and every screen shows
//! who. This is not authentication — a club's LAN is not a threat model — it is
//! the discipline **D05** applies to the bus, where only the polled node
//! transmits: collisions prevented by construction rather than by etiquette.
//!
//! It **expires**, and that matters more than the claiming does. A token that
//! never expires is a token that strands an event when somebody closes a laptop
//! and drives home, so holding it requires coming back.
//!
//! ## The operator arms, not the machine
//!
//! [`Staging`] reaching `Ready` means the tree *may* be armed, never that it
//! was. Nothing goes out until somebody with control says so. That is the
//! difference between this and `beam402 sim`, and it is the whole reason the
//! loop had to be opened up rather than reused.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use beam402_bus::{Bus, Paced};
use beam402_mapping::Mapping;
use beam402_poller::{Phase as BusPhase, Poller};
use beam402_race::staging::{Action, Config, Phase, Staging};
use beam402_race::{decide, Outcome, Pairing, RunBuilder};

use crate::round::{self, STEP_MS};
use crate::slip;

/// How long control survives without being renewed.
const TOKEN_IDLE: Duration = Duration::from_secs(20);

/// What a client can ask the bus thread to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Arm,
    Abort,
    /// Clear the round and set up for the next pair.
    Next,
}

#[derive(Clone, Copy, Debug)]
struct Control {
    token: u64,
    seen: Instant,
}

impl Control {
    fn live(&self) -> bool {
        self.seen.elapsed() < TOKEN_IDLE
    }
}

/// Everything shared between the bus thread and the server.
///
/// The published state is a **string**, rendered by the bus thread outside the
/// lock. A handler holding a mutex while it formats JSON would block the bus for
/// as long as it took, and the bus is the thing with a deadline.
#[derive(Default)]
struct Shared {
    state: String,
    control: Option<Control>,
    intents: Vec<Intent>,
}

pub struct Live {
    shared: Arc<Mutex<Shared>>,
    next_token: AtomicU64,
}

impl Live {
    pub fn new() -> Arc<Live> {
        Arc::new(Live {
            shared: Arc::new(Mutex::new(Shared::default())),
            // Seeded from the clock so tokens do not repeat across a restart —
            // a client holding a stale one must not be handed control by a
            // number that came round again.
            next_token: AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(1)
                    | 1,
            ),
        })
    }

    /// The current state, as JSON. Cheap: one clone of a string the bus thread
    /// already rendered.
    pub fn state(&self) -> String {
        let s = self.shared.lock().expect("state lock");
        if s.state.is_empty() {
            "{\"phase\":\"starting\"}".to_string()
        } else {
            s.state.clone()
        }
    }

    /// Take control, or renew it. Returns the token, or `None` if somebody else
    /// holds it and has not gone quiet.
    pub fn claim(&self, existing: Option<u64>) -> Option<u64> {
        let mut s = self.shared.lock().expect("state lock");
        match s.control {
            Some(c) if c.live() && Some(c.token) != existing => None,
            Some(c) if Some(c.token) == existing => {
                s.control = Some(Control {
                    token: c.token,
                    seen: Instant::now(),
                });
                Some(c.token)
            }
            // Free, or the holder stopped coming back.
            _ => {
                let token = self.next_token.fetch_add(2, Ordering::Relaxed);
                s.control = Some(Control {
                    token,
                    seen: Instant::now(),
                });
                Some(token)
            }
        }
    }

    pub fn release(&self, token: u64) {
        let mut s = self.shared.lock().expect("state lock");
        if s.control.map(|c| c.token) == Some(token) {
            s.control = None;
        }
    }

    /// Queue an intent. Refused unless the caller holds live control.
    pub fn intend(&self, token: u64, intent: Intent) -> Result<(), &'static str> {
        let mut s = self.shared.lock().expect("state lock");
        match s.control {
            Some(c) if c.token == token && c.live() => {
                s.control = Some(Control {
                    token,
                    seen: Instant::now(),
                });
                s.intents.push(intent);
                Ok(())
            }
            Some(c) if c.live() => Err("another client holds control"),
            _ => Err("nobody holds control — claim it first"),
        }
    }

    fn take_intents(&self) -> (Vec<Intent>, Option<u64>) {
        let mut s = self.shared.lock().expect("state lock");
        let holder = s.control.filter(|c| c.live()).map(|c| c.token);
        (std::mem::take(&mut s.intents), holder)
    }

    fn publish(&self, state: String) {
        self.shared.lock().expect("state lock").state = state;
    }
}

/// The bus thread. Owns the bus, the poller, the staging machine and the round,
/// and never gives any of them out.
pub struct Runtime<'m, B> {
    bus: B,
    mapping: &'m Mapping,
    pairing: Pairing,
    addresses: Vec<u8>,
    tree: u8,
    poller: Poller,
    staging: Staging<'m>,
    builder: RunBuilder<'m>,
    armed: bool,
    cycles: u64,
    note: String,
}

impl<'m, B: Bus + Paced> Runtime<'m, B> {
    pub fn new(
        bus: B,
        mapping: &'m Mapping,
        pairing: Pairing,
        addresses: Vec<u8>,
        tree: u8,
        cfg: Config,
    ) -> Self {
        Runtime {
            bus,
            mapping,
            pairing,
            addresses: addresses.clone(),
            tree,
            poller: Poller::new(addresses),
            staging: Staging::with_config(mapping, cfg),
            builder: RunBuilder::new(mapping),
            armed: false,
            cycles: 0,
            note: String::new(),
        }
    }

    /// One pass: drain intents, poll, act, publish.
    ///
    /// Nothing here holds the shared lock while it talks to the bus. A client
    /// waiting on a mutex is an inconvenience; a bus cycle waiting on one is a
    /// staging lamp that does not move.
    pub fn step(&mut self, live: &Live) {
        let (intents, holder) = live.take_intents();

        let mut events = Vec::new();
        let stats = self.poller.cycle(&mut self.bus, &mut events);
        self.cycles += 1;

        let mut actions = Vec::new();
        for e in &events {
            self.builder.apply(e);
            actions.extend(self.staging.apply(e));
        }
        actions.extend(self.staging.tick(STEP_MS));

        for action in actions {
            match action {
                Action::ShowStaging(lamps) => {
                    self.poller.send(
                        self.tree,
                        beam402_protocol::Opcode::TreeStaging,
                        lamps.bits(),
                        0,
                    );
                }
                // The machine says the tree *may* be armed. Somebody with
                // control has to say that it is.
                Action::Arm => {}
                Action::Quiet => self.poller.set_phase(BusPhase::Quiet),
                Action::Collect => {
                    self.poller.set_phase(BusPhase::Live);
                    for a in &self.addresses {
                        self.poller.refetch(*a);
                    }
                }
                Action::Abandon => {
                    self.note = "abandoned: a car never reached the finish beam".into()
                }
            }
        }

        for intent in intents {
            match intent {
                Intent::Arm => self.do_arm(),
                Intent::Abort => self.do_abort(),
                Intent::Next => self.do_next(),
            }
        }

        let state = self.render(holder, stats.millis());
        live.publish(state);
        self.bus.advance_ms(STEP_MS);
    }

    fn do_arm(&mut self) {
        if !self.staging.is_ready() {
            self.note = "not ready to arm".into();
            return;
        }
        let handicap = match self.pairing.handicap_ms() {
            Ok(h) => h,
            Err(e) => {
                self.note = e.to_string();
                return;
            }
        };
        match round::arm(
            &mut self.poller,
            &mut self.bus,
            self.tree,
            handicap,
            &mut self.builder,
        ) {
            Ok(_) => {
                self.staging.armed(handicap);
                self.poller.set_phase(BusPhase::Quiet);
                self.armed = true;
                self.note.clear();
            }
            Err(e) => self.note = e,
        }
    }

    fn do_abort(&mut self) {
        self.poller
            .send(self.tree, beam402_protocol::Opcode::TreeAbort, 0, 0);
        self.poller.set_phase(BusPhase::Live);
        self.staging.reset();
        self.armed = false;
        self.note = "aborted".into();
    }

    fn do_next(&mut self) {
        self.builder.clear_round();
        self.staging.reset();
        self.poller.set_phase(BusPhase::Live);
        self.poller.release_tree(self.tree);
        for a in &self.addresses {
            self.poller.refetch(*a);
        }
        self.armed = false;
        self.note.clear();
    }

    fn render(&self, holder: Option<u64>, bus_ms: f64) -> String {
        let round = self.builder.round();
        let phase = match self.staging.phase() {
            Phase::Blocked(why) => format!("blocked: {why}"),
            other => format!("{other:?}").to_lowercase(),
        };
        let slip = slip::render(&round, &self.pairing, None, false);
        let verdict = match decide(&round, &self.pairing) {
            Outcome::Win { lane, .. } => format!("lane {}", lane.number()),
            Outcome::NoContest => String::new(),
        };

        let mut lanes = String::new();
        for (i, entry) in self.pairing.entries().iter().enumerate() {
            if i > 0 {
                lanes.push(',');
            }
            let lane = entry.lane;
            let run = round.lane(lane);
            let num = |v: Option<f64>| match v {
                Some(v) => format!("{v:.4}"),
                None => "null".into(),
            };
            lanes.push_str(&format!(
                "{{\"lane\":{},\"where\":\"{}\",\"dial\":{},\"reaction\":{},\"et\":{},\"kmh\":{}}}",
                lane.number(),
                format!("{:?}", self.staging.position(lane)).to_lowercase(),
                num(self.pairing.breakout_limit(lane)),
                num(run.and_then(|r| r.reaction_s)),
                num(run.and_then(|r| r.et_s)),
                num(run.and_then(|r| r.trap_speed_kmh())),
            ));
        }

        let mut nodes = String::new();
        for (i, a) in self.addresses.iter().enumerate() {
            if i > 0 {
                nodes.push(',');
            }
            let d = self.poller.device(*a);
            nodes.push_str(&format!(
                "{{\"a\":{a},\"silent\":{},\"known\":{}}}",
                d.map(|d| d.silent).unwrap_or(true),
                d.map(|d| d.identity.is_some()).unwrap_or(false),
            ));
        }

        // The board travels in the same snapshot as everything else, so every
        // page renders from one endpoint and none of them can disagree about
        // what the round currently is.
        let board = beam402_scoreboard::Board::REFERENCE;
        let show = match self.staging.phase() {
            Phase::Idle => beam402_scoreboard::Show::Idle,
            Phase::Complete => beam402_scoreboard::Show::Result,
            Phase::Staging | Phase::Ready | Phase::Blocked(_) => beam402_scoreboard::Show::Staging,
            _ => beam402_scoreboard::Show::Running,
        };
        let frame = beam402_scoreboard::render(
            board,
            show,
            &self.mapping.venue.name,
            &round,
            &self.pairing,
        );

        format!(
            "{{\"phase\":\"{phase}\",\"ready\":{},\"armed\":{},\"held\":{},\"holder\":{},\
\"cycles\":{},\"bus_ms\":{:.0},\"note\":\"{}\",\"winner\":\"{verdict}\",\
\"board\":{{\"w\":{},\"h\":{},\"bits\":\"{}\"}},\
\"lanes\":[{lanes}],\"nodes\":[{nodes}],\"slip\":\"{}\"}}",
            self.staging.is_ready(),
            self.armed,
            holder.is_some(),
            match holder {
                Some(t) => t.to_string(),
                None => "null".into(),
            },
            self.cycles,
            bus_ms,
            escape(&self.note),
            board.w,
            board.height(),
            frame.hex(),
            escape(&slip),
        )
    }
}

/// JSON string escaping, hand-written for the same reason as everywhere else in
/// this project: the payload is numbers and short strings this code produced.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Wall-clock pacing. A simulator's virtual time is advanced by `step`; this is
/// what makes a round take as long to watch as it takes to run.
pub fn pace() {
    std::thread::sleep(Duration::from_millis(STEP_MS));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_client_holds_control_and_the_others_are_told_so() {
        // D30's rule, and the failure it prevents: two people arming is worse
        // than neither.
        let live = Live::new();
        let a = live.claim(None).expect("free at the start");
        assert_eq!(live.claim(None), None, "a second client is refused");
        assert_eq!(live.claim(Some(a)), Some(a), "the holder renews instead");

        assert_eq!(live.intend(a, Intent::Arm), Ok(()));
        assert!(live.intend(a.wrapping_add(1), Intent::Arm).is_err());
    }

    #[test]
    fn releasing_hands_control_to_the_next_client() {
        let live = Live::new();
        let a = live.claim(None).unwrap();
        live.release(a);
        let b = live.claim(None).expect("free again");
        assert_ne!(a, b, "and the token is not reissued");
        assert!(
            live.intend(a, Intent::Arm).is_err(),
            "the old token stops working"
        );
    }

    #[test]
    fn an_intent_without_control_is_refused_rather_than_queued() {
        // Nothing reaches the bus on the say-so of a client that never claimed.
        let live = Live::new();
        assert!(live.intend(1, Intent::Arm).is_err());
        let (intents, holder) = live.take_intents();
        assert!(intents.is_empty());
        assert!(holder.is_none());
    }

    #[test]
    fn a_client_that_goes_away_does_not_strand_the_event() {
        // The reason the token expires at all. Somebody closes a laptop and
        // drives home; the event must not need a restart.
        let live = Live::new();
        let gone = live.claim(None).unwrap();
        {
            let mut s = live.shared.lock().unwrap();
            s.control = Some(Control {
                token: gone,
                seen: Instant::now() - TOKEN_IDLE - Duration::from_secs(1),
            });
        }
        let next = live
            .claim(None)
            .expect("the stale holder is not an obstacle");
        assert_ne!(next, gone);
        assert!(live.intend(gone, Intent::Arm).is_err());
    }

    #[test]
    fn the_published_state_is_a_string_the_bus_thread_rendered() {
        // A handler that formatted JSON under the lock would hold the bus up for
        // as long as it took, and the bus is the thing with a deadline.
        let live = Live::new();
        assert!(live.state().contains("starting"), "before the first cycle");
        live.publish("{\"phase\":\"idle\"}".into());
        assert_eq!(live.state(), "{\"phase\":\"idle\"}");
    }

    #[test]
    fn a_slip_cannot_break_out_of_the_json_it_travels_in() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("</script>"), "\\u003c/script\\u003e");
        assert_eq!(escape("l1\nl2"), "l1\\nl2");
    }
}
