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

use beam402_bus::{Bus, CallUp, Paced};
use beam402_event::EntryId;
use beam402_mapping::Mapping;
use beam402_poller::{Phase as BusPhase, Poller};
use beam402_protocol::Lane;
use beam402_race::staging::{Action, Config, Phase, Staging};
use beam402_race::{decide, Outcome, Pairing, Round, RunBuilder};

use crate::meeting::Meeting;
use crate::round::{self, STEP_MS};
use crate::slip;

/// How long control survives without being renewed.
const TOKEN_IDLE: Duration = Duration::from_secs(20);

/// What a client can ask the bus thread to do.
/// Not `Copy`: a called foul carries a rulebook's word for it (**D37**), and a
/// word is a `String`. Nothing here is in a hot path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Intent {
    Arm,
    Abort,
    /// Clear the round and set up for the next pair.
    Next,
    /// Write the result down and advance the ladder. The operator's, for the same
    /// reason arming is — see [`crate::meeting`].
    Record,
    /// Exchange lanes, which is lane choice being exercised.
    Swap,
    /// Put a particular car on the line, in a particular lane. The derived queue
    /// is a default; cars arrive in the order they arrive. Calling into the other
    /// lane is how a practice pass gets its second car.
    Call(EntryId, Lane),
    /// Close qualifying and draw the ladder. The operator's, because how many
    /// passes a club gives is a club's business and no count here can know it.
    Draw,
    /// Run this pair with one car in it — the other could not make the call — or
    /// put the other car back. A toggle on the lane that is running.
    Single(Lane),
    /// That entry's last pass does not count (**D37**).
    Void(EntryId),
    /// That entry is out of the class that is running (**D37**).
    Scratch(EntryId),
    /// A foul an official called, in their rulebook's word. In a round it decides
    /// the pair, because that is what calling one means (**D37**).
    Foul(EntryId, String),
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
    /// The meeting, when one was loaded. Without it this runs the single pairing
    /// it was given, which is what a test session or a grudge race is.
    meeting: Option<Meeting>,
    armed: bool,
    cycles: u64,
    /// How long the round has been [`Phase::Complete`], in loop time.
    ///
    /// The staging machine calls a round complete when every car has been *seen*
    /// to cross the finish beam, which is the right statement about the strip and
    /// not yet a statement about the numbers: the ETs are latched in the nodes and
    /// arrive on a later poll (**D25**). This is how long they have had to.
    complete_ms: u64,
    /// A car never reached the finish beam, so this round's numbers are not late —
    /// they do not exist.
    abandoned: bool,
    note: String,
}

/// How long a finished round is given to produce its numbers before "not yet"
/// becomes "never".
///
/// Recording inside that window wrote a result with no time in it — a driver's
/// real pass logged as `-`, which seeds them at the back of a class they were
/// leading. Past it, a lane that was seen at the finish and still has nothing is a
/// record that is not coming, and refusing forever would strand the day.
///
/// Wide enough for several poll cycles at 19,200 bps with a node down, short
/// enough that nobody standing at the panel thinks the button is broken.
const SETTLE_MS: u64 = 3_000;

/// Whether a finished round is **whole**.
///
/// Whole means every lane that raced has its ET, or there is no longer any reason
/// to expect one: the round was abandoned, or the records have had [`SETTLE_MS`]
/// to arrive and did not. That last case is a node that stopped answering, and the
/// honest record for it is the one with `-` in it — written deliberately, rather
/// than by beating the poll loop to a button.
///
/// Only ever asked about a round the staging machine already calls complete. A
/// pairing sitting on the line has no times either, and that is not the same
/// question.
fn settled(round: &Round, pairing: &Pairing, abandoned: bool, complete_ms: u64) -> bool {
    if abandoned || complete_ms >= SETTLE_MS {
        return true;
    }
    pairing
        .entries()
        .iter()
        .all(|e| round.lane(e.lane).is_some_and(|r| r.et_s.is_some()))
}

impl<'m, B: Bus + Paced + CallUp> Runtime<'m, B> {
    pub fn new(
        bus: B,
        mapping: &'m Mapping,
        pairing: Pairing,
        addresses: Vec<u8>,
        tree: u8,
        cfg: Config,
    ) -> Self {
        let mut staging = Staging::with_config(mapping, cfg);
        staging.racing(pairing.entries().iter().map(|e| e.lane));
        Runtime {
            bus,
            mapping,
            pairing,
            addresses: addresses.clone(),
            tree,
            poller: Poller::new(addresses),
            staging,
            builder: RunBuilder::new(mapping),
            meeting: None,
            armed: false,
            cycles: 0,
            complete_ms: 0,
            abandoned: false,
            note: String::new(),
        }
    }

    /// Run a meeting rather than a single pairing. The pair on deck replaces the
    /// one this was built with, and every `Next` takes the next one off the ladder.
    pub fn with_meeting(mut self, meeting: Meeting) -> Self {
        self.meeting = Some(meeting);
        self.take_pairing();
        self
    }

    /// Adopt the on-deck pairing, if there is a meeting to take one from.
    fn take_pairing(&mut self) {
        let Some(meeting) = &self.meeting else { return };
        match meeting.pairing() {
            // The tree waits on the lanes this pairing has cars in, which is one
            // of them for a bye. Set here rather than at each caller because every
            // path to a new pairing comes through this function.
            Ok(p) => {
                self.staging.racing(p.entries().iter().map(|e| e.lane));
                self.pairing = p;
            }
            // Left showing: a class whose entry sheet cannot produce a pairing is
            // something an operator has to see, not something to fall back from.
            Err(e) => self.note = e,
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
                    // Nothing more is coming for a car that never reached the
                    // finish beam, so the round is as whole as it will get.
                    self.abandoned = true;
                    self.note = "abandoned: a car never reached the finish beam".into()
                }
            }
        }

        // How long the numbers have had to come back. Measured about this round
        // rather than the day, so anything that is not a finished round clears it.
        self.complete_ms = if self.staging.phase() == Phase::Complete {
            self.complete_ms + STEP_MS
        } else {
            0
        };

        for intent in intents {
            match intent {
                Intent::Arm => self.do_arm(),
                Intent::Abort => self.do_abort(),
                Intent::Next => self.do_next(),
                Intent::Record => self.do_record(),
                Intent::Swap => self.do_swap(),
                Intent::Call(entry, lane) => self.do_call(entry, lane),
                Intent::Draw => self.do_draw(),
                Intent::Single(lane) => self.do_single(lane),
                Intent::Void(entry) => self.do_note(|m| m.void_last(entry)),
                Intent::Scratch(entry) => self.do_note(|m| m.scratch(entry)),
                Intent::Foul(entry, kind) => self.do_foul(entry, &kind),
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
        // A meeting with nothing left to run is a meeting that is over. The staging
        // machine cannot know that — a car on the beams looks the same either way —
        // so the interlock belongs here. It asks whether anything is *to run*
        // rather than whether a pair is on deck: qualifying has no pair either, and
        // the narrower question refused every time trial there is.
        if self.meeting.as_ref().is_some_and(|m| m.nothing_to_run()) {
            self.note = "nothing to run — every class has finished".into();
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
                self.abandoned = false;
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

    /// Write the round down. The timing system decides *what*; this decides
    /// nothing and only says when.
    fn do_record(&mut self) {
        let round = self.builder.round();
        let pairing = self.pairing.clone();
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded — start with --event to record results".into();
            return;
        };
        if self.staging.phase() != Phase::Complete {
            self.note = "the round is not over".into();
            return;
        }
        // **Seen at the finish beam is not the same as measured.** The staging
        // machine goes `Complete` on the beam edge; the ET is latched in the node
        // and arrives on a later poll (**D25**). Recording in between wrote the
        // pass down with no time in it — a real 11.85 logged as `-`, which seeds
        // that driver at the back of a class they were leading, silently. So the
        // gate is the round being whole rather than the car being over the line.
        if !settled(&round, &pairing, self.abandoned, self.complete_ms) {
            self.note = "the finish records have not come back yet".into();
            return;
        }
        self.note = match meeting.record(&round, &pairing) {
            Ok(line) => format!("recorded: {line}"),
            Err(why) => why,
        };
    }

    /// Lane choice, exercised. Before the arm only: after it the handicap is
    /// latched in the tree and the lanes are the ones the cars are sitting in.
    fn do_swap(&mut self) {
        if self.armed {
            self.note = "already armed — abort first".into();
            return;
        }
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match meeting.swap() {
            Ok(()) => {
                self.note.clear();
                self.take_pairing();
            }
            Err(why) => self.note = why,
        }
    }

    /// Put a named car in a named lane.
    ///
    /// **Placing a car, not clearing a round.** Two cars on a practice pass are two
    /// calls, so a call that cleared would throw the first car away the moment the
    /// second was named. Moving on is `Next`, which is what it already means — and a
    /// call after a recorded pass is refused by the meeting saying to clear the
    /// round, which points at the right button rather than guessing at intent.
    fn do_call(&mut self, entry: EntryId, lane: Lane) {
        let round = self.builder.round();
        if self
            .meeting
            .as_ref()
            .is_some_and(|m| m.owes_a_record(&round, &self.pairing))
        {
            self.note = "this pass has a result that has not been recorded".into();
            return;
        }
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match meeting.call(entry, lane) {
            Ok(()) => {
                self.note.clear();
                self.take_pairing();
            }
            Err(why) => self.note = why,
        }
    }

    /// Run the pair on deck with one car in it, or put the other car back.
    ///
    /// Refused once armed, for the same reason a swap is: the tree has already been
    /// told what it is starting, and this changes exactly that.
    fn do_single(&mut self, lane: Lane) {
        if self.armed {
            self.note = "already armed — abort first".into();
            return;
        }
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match meeting.single(lane) {
            Ok(()) => {
                self.note.clear();
                self.take_pairing();
            }
            Err(why) => self.note = why,
        }
    }

    /// A line an official appends: a void, a scratch. Neither touches the ladder
    /// and neither ends a round, so there is no interlock to apply — what they
    /// change is a field that has not been drawn yet (**D37**).
    fn do_note(&mut self, write: impl FnOnce(&mut Meeting) -> Result<String, String>) {
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match write(meeting) {
            Ok(line) => {
                self.note = format!("wrote: {line}");
                self.take_pairing();
            }
            Err(why) => self.note = why,
        }
    }

    /// A foul an official called. In a round it decides the pair, so it moves the
    /// ladder and the round is cleared behind it exactly as recording one does.
    fn do_foul(&mut self, entry: EntryId, kind: &str) {
        let round = self.builder.round();
        let pairing = self.pairing.clone();
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match meeting.judged(&round, &pairing, entry, kind) {
            Ok(line) => self.note = format!("called: {line}"),
            Err(why) => self.note = why,
        }
    }

    /// Close qualifying and draw the ladder.
    ///
    /// No `armed` guard, deliberately, and for the same reason `Next` has none:
    /// `armed` stays set through a *completed* run, and "record the last pass, then
    /// close qualifying" is the order an operator works in. Refusing that would
    /// mean pressing next before draw to clear a flag, which is a flag leaking into
    /// a procedure. What must not be lost is a pass nobody wrote down, and that is
    /// the guard below.
    fn do_draw(&mut self) {
        // Same guard as `Next`, because the draw is a bigger door to lose a pass
        // through: once the ladder is drawn, `qualified` refuses, so an unrecorded
        // pass is not late — it is gone.
        let round = self.builder.round();
        if self
            .meeting
            .as_ref()
            .is_some_and(|m| m.owes_a_record(&round, &self.pairing))
        {
            self.note = "this pass has a result that has not been recorded".into();
            return;
        }
        let Some(meeting) = self.meeting.as_mut() else {
            self.note = "no event is loaded".into();
            return;
        };
        match meeting.draw() {
            Ok(line) => {
                self.note = format!("drawn: {line}");
                self.builder.clear_round();
                self.staging.reset();
                self.poller.set_phase(BusPhase::Live);
                self.poller.release_tree(self.tree);
                self.armed = false;
                self.take_pairing();
                self.bus.call_up();
            }
            Err(why) => self.note = why,
        }
    }

    fn do_next(&mut self) {
        // A result nobody wrote down must not be thrown away by the button that
        // brings up the next pair.
        let round = self.builder.round();
        if self
            .meeting
            .as_ref()
            .is_some_and(|m| m.owes_a_record(&round, &self.pairing))
        {
            self.note = "this round has a result that has not been recorded".into();
            return;
        }
        // Nor by the button that brings up the next pair while the numbers are
        // still on their way. Clearing the round here does not stop the records
        // arriving — they are latched in the nodes and a poll is already asking
        // (**D25**) — so what used to happen is that the previous car's ET landed
        // in the next pair's round and showed on the panel before it had staged.
        if self.staging.phase() == Phase::Complete
            && !settled(&round, &self.pairing, self.abandoned, self.complete_ms)
        {
            self.note = "the finish records have not come back yet".into();
            return;
        }

        self.builder.clear_round();
        self.staging.reset();
        self.abandoned = false;
        self.poller.set_phase(BusPhase::Live);
        self.poller.release_tree(self.tree);
        // Deliberately **no** refetch. The nodes still hold the last round's
        // records, latched and current (**D25**), and asking for them again would
        // reassemble the round that was just cleared. What starts the next one is
        // a generation moving, which is a car.
        self.armed = false;
        self.note.clear();
        if let Some(meeting) = self.meeting.as_mut() {
            meeting.next();
            self.take_pairing();
        }
        // The next pair pulls up. Nothing on a real strip; a simulator has to be
        // told, which is what `CallUp` is for.
        self.bus.call_up();
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

        let event = match &self.meeting {
            Some(m) => m.json(),
            None => "null".to_string(),
        };

        format!(
            "{{\"phase\":\"{phase}\",\"ready\":{},\"armed\":{},\"settled\":{},\
\"held\":{},\"holder\":{},\
\"cycles\":{},\"bus_ms\":{:.0},\"note\":\"{}\",\"winner\":\"{verdict}\",\
\"board\":{{\"w\":{},\"h\":{},\"bits\":\"{}\"}},\"event\":{event},\
\"lanes\":[{lanes}],\"nodes\":[{nodes}],\"slip\":\"{}\"}}",
            self.staging.is_ready(),
            self.armed,
            // Whether the round is whole, so the panel can stop offering `record`
            // in the window where it would write a pass down with no time in it.
            settled(&round, &self.pairing, self.abandoned, self.complete_ms),
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
pub fn escape(s: &str) -> String {
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

    /// The defect a practice day found. The staging machine calls a round complete
    /// on the **finish beam**; the ET is latched in the node and comes back on a
    /// later poll (**D25**) — 1.3 s later on the reference venue. The panel offered
    /// `record` for that whole beat, and a press inside it wrote
    /// `Q Unlimited 17 - - -`: a real 11.85 logged as no time at all, which seeds
    /// that driver at the back of a class they were leading. Silently, because a
    /// pass with no time is a legitimate record — it is what a car that stops on
    /// the track gets.
    #[test]
    fn a_finished_round_is_not_whole_until_its_numbers_arrive() {
        use beam402_race::{Entry, Format, LaneRun};

        let one_lane = Pairing::new(
            Format::HeadsUp,
            vec![Entry {
                lane: Lane::L1,
                dial_s: None,
            }],
        )
        .unwrap();
        let timed = |et: f64| {
            let mut r = Round::default();
            r.set_lane(
                Lane::L1,
                LaneRun {
                    reaction_s: Some(0.412),
                    et_s: Some(et),
                    ..LaneRun::default()
                },
            );
            r
        };
        // The window: over the line, nothing off the node yet.
        let mut crossed = Round::default();
        crossed.set_lane(
            Lane::L1,
            LaneRun {
                reaction_s: Some(0.412),
                ..LaneRun::default()
            },
        );
        assert!(!settled(&crossed, &one_lane, false, 0));
        assert!(!settled(&crossed, &one_lane, false, SETTLE_MS - STEP_MS));

        // The number arrives and the round is whole.
        assert!(settled(&timed(11.85), &one_lane, false, 0));

        // Two ways it is whole without one. A car that never reached the finish
        // beam has no number coming...
        assert!(settled(&crossed, &one_lane, true, 0));
        // ...and neither has a node that stopped answering, which is the case that
        // must not strand the day: past the grace, `-` is the honest record and it
        // is written deliberately.
        assert!(settled(&crossed, &one_lane, false, SETTLE_MS));

        // Both lanes, because a pair is only as recordable as its slower node.
        let two = Pairing::new(
            Format::HeadsUp,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: None,
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: None,
                },
            ],
        )
        .unwrap();
        assert!(!settled(&timed(11.85), &two, false, 0), "lane 2 has nothing");
    }

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

    /// Four cars, an index class, and nobody touching anything except through the
    /// four buttons an operator has.
    #[test]
    fn a_class_runs_down_its_ladder_over_the_bus_and_the_log_holds_the_day() {
        use beam402_event::{EntryId, Progress, Sheet};
        use beam402_race::{Entry, Format};
        use beam402_sim::reference::{clean_pair, venue, ADDRESSES, TREE};

        const SHEET: &str = r#"
[event]
name = "Ladder over the bus"
date = "2026-08-15"
[[class]]
name = "Super Gas"
format = "index"
index_s = 9.90
seeding = "quickest-et"
ladder = "pro"
[[entry]]
number = 1
driver = "A"
class = "Super Gas"
[[entry]]
number = 2
driver = "B"
class = "Super Gas"
[[entry]]
number = 3
driver = "C"
class = "Super Gas"
[[entry]]
number = 4
driver = "D"
class = "Super Gas"
"#;

        let mut log = std::env::temp_dir();
        log.push(format!("beam402-ladder-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);

        // Qualifying is not what this test is about, so it is written straight
        // into the log — which also means the run below starts from a replay
        // rather than from something held in memory.
        {
            let mut day = Progress::new(Sheet::parse(SHEET).unwrap());
            let mut lines = Vec::new();
            for (n, et) in [(1u32, 9.91), (2, 9.95), (3, 9.99), (4, 10.20)] {
                lines.push(
                    day.qualified("Super Gas", EntryId(n), Some(et), None, false)
                        .unwrap()
                        .line(),
                );
            }
            lines.push(day.draw("Super Gas").unwrap().line());
            std::fs::write(&log, lines.join("\n") + "\n").unwrap();
        }

        let mapping = venue();
        // No `arm_at_s`: every arm in this test comes from the operator, which is
        // the only way a meeting of several rounds can work.
        let text = clean_pair().replace("arm_at_s = 3.0", "");
        let sim =
            beam402_sim::Simulator::new(&mapping, beam402_sim::Scenario::parse(&text).unwrap())
                .unwrap();
        let meeting = Meeting::open(SHEET, &log).unwrap();

        let live = Live::new();
        let token = live.claim(None).expect("control at the start");
        let idle = Pairing::new(
            Format::HeadsUp,
            vec![
                Entry {
                    lane: beam402_protocol::Lane::L1,
                    dial_s: None,
                },
                Entry {
                    lane: beam402_protocol::Lane::L2,
                    dial_s: None,
                },
            ],
        )
        .unwrap();
        let mut rt = Runtime::new(
            sim,
            &mapping,
            idle,
            ADDRESSES.to_vec(),
            TREE,
            Config::default(),
        )
        .with_meeting(meeting);

        let mut cleared_between_rounds = true;
        let mut recorded = 0;
        let mut just_moved_on = false;
        for _ in 0..4_000 {
            if rt.meeting.as_ref().unwrap().deck().is_none() {
                break;
            }
            let round = rt.builder.round();
            if just_moved_on {
                // The fix `do_next` documents: the nodes still hold the last
                // round's records, and asking for them again would rebuild the
                // round that was just cleared.
                cleared_between_rounds &= round
                    .lane(beam402_protocol::Lane::L1)
                    .and_then(|r| r.et_s)
                    .is_none();
                just_moved_on = false;
            }
            if rt.staging.phase() == Phase::Complete {
                if rt
                    .meeting
                    .as_ref()
                    .unwrap()
                    .owes_a_record(&round, &rt.pairing)
                {
                    live.intend(token, Intent::Record).unwrap();
                    recorded += 1;
                } else {
                    live.intend(token, Intent::Next).unwrap();
                    just_moved_on = true;
                }
            } else if rt.staging.is_ready() && !rt.armed {
                live.intend(token, Intent::Arm).unwrap();
            }
            rt.step(&live);
        }

        assert_eq!(recorded, 3, "1 v 4, 2 v 3 and a final: {}", rt.note);
        assert!(cleared_between_rounds, "a round carried over into the next");

        // The day, as the file on disk holds it. Nothing in memory is consulted:
        // if this passes, race control could have been restarted at any point.
        let text = std::fs::read_to_string(&log).unwrap();
        let (day, skipped) = Progress::replay(Sheet::parse(SHEET).unwrap(), &text);
        assert_eq!(skipped, 0);
        assert!(day.champion("Super Gas").is_some(), "the class was settled");
        assert_eq!(
            text.lines().filter(|l| l.starts_with("W ")).count(),
            3,
            "one line per pair and no more"
        );
        std::fs::remove_file(&log).ok();
    }

    /// Qualifying, over the bus, with one car on the track and nothing in the
    /// other lane — which is the whole point. The scenario has a single `[[car]]`,
    /// so nothing is standing in lane 2 to satisfy a tree that waits for the lanes
    /// the *track* declares: before `Staging::racing` this could not arm, and a
    /// test driving it would spin until the loop gave up.
    ///
    /// It runs the shape of a small class end to end: two passes for one entry,
    /// the operator closing qualifying, and the one-car ladder that comes out of
    /// it — whose first round is a bye, also one car, also impossible before.
    #[test]
    fn one_car_qualifies_over_the_bus_and_wins_the_ladder_it_draws() {
        use beam402_event::{Progress, Sheet};
        use beam402_race::{Entry, Format};
        use beam402_sim::reference::{venue, ADDRESSES, TREE};

        const SHEET: &str = r#"
[event]
name = "Time trials"
date = "2026-08-15"
[[class]]
name = "Super Gas"
format = "index"
index_s = 9.90
seeding = "quickest-et"
ladder = "pro"
[[entry]]
number = 7
driver = "Solo"
class = "Super Gas"
"#;

        // One car, and no `arm_at_s`: every arm is the operator's.
        const SOLO: &str = r#"
[scenario]
name = "solo pass"
seed = 42

[tree]
address = 10
mode = "standard"
random_delay_ms = 700

[[car]]
lane = 1
stage_at_s = 1.0
reaction_s = 0.520
[car.splits]
interval_60 = 1.632
trap_entry = 8.900
trap_exit = 9.900
finish = 10.412
"#;

        let mut log = std::env::temp_dir();
        log.push(format!("beam402-qual-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&log);

        let mapping = venue();
        let sim =
            beam402_sim::Simulator::new(&mapping, beam402_sim::Scenario::parse(SOLO).unwrap())
                .unwrap();
        let meeting = Meeting::open(SHEET, &log).unwrap();
        assert!(
            meeting.line().is_some(),
            "a day with nothing drawn starts in qualifying"
        );

        let live = Live::new();
        let token = live.claim(None).expect("control at the start");
        let idle = Pairing::new(
            Format::HeadsUp,
            vec![Entry {
                lane: beam402_protocol::Lane::L1,
                dial_s: None,
            }],
        )
        .unwrap();
        let mut rt = Runtime::new(
            sim,
            &mapping,
            idle,
            ADDRESSES.to_vec(),
            TREE,
            Config::default(),
        )
        .with_meeting(meeting);

        let mut passes = 0;
        let mut byes = 0;
        for _ in 0..6_000 {
            let m = rt.meeting.as_ref().unwrap();
            let qualifying = m.line().is_some();
            if !qualifying && m.deck().is_none() {
                break;
            }
            let round = rt.builder.round();
            if rt.staging.phase() == Phase::Complete {
                if rt
                    .meeting
                    .as_ref()
                    .unwrap()
                    .owes_a_record(&round, &rt.pairing)
                {
                    live.intend(token, Intent::Record).unwrap();
                    if qualifying {
                        passes += 1;
                    } else {
                        byes += 1;
                    }
                } else if qualifying && passes >= 2 {
                    // Two passes is this club's session. Nothing counts them for
                    // the operator, which is the point of `Draw` being an intent.
                    live.intend(token, Intent::Draw).unwrap();
                } else {
                    live.intend(token, Intent::Next).unwrap();
                }
            } else if rt.staging.is_ready() && !rt.armed {
                live.intend(token, Intent::Arm).unwrap();
            }
            rt.step(&live);
        }

        assert_eq!(passes, 2, "two qualifying passes: {}", rt.note);
        assert_eq!(byes, 1, "a field of one is a bye: {}", rt.note);

        // The day as the file holds it, replayed by the same crate the receiver
        // uses. Nothing in memory is consulted.
        let text = std::fs::read_to_string(&log).unwrap();
        let (day, skipped) = Progress::replay(Sheet::parse(SHEET).unwrap(), &text);
        assert_eq!(skipped, 0, "every line replayed: {text}");
        assert_eq!(text.lines().filter(|l| l.starts_with("Q ")).count(), 2);
        assert_eq!(text.lines().filter(|l| l.starts_with("D ")).count(), 1);
        assert_eq!(day.attempts("Super Gas").len(), 2);
        // The ET the scenario stated, recovered through beams, bus and log.
        let best = day.attempts("Super Gas")[0].et_s.unwrap();
        assert!(
            (best - 10.412).abs() < 0.002,
            "the pass the scenario stated: {best}"
        );
        assert!(day.champion("Super Gas").is_some(), "the class was settled");
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_slip_cannot_break_out_of_the_json_it_travels_in() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("</script>"), "\\u003c/script\\u003e");
        assert_eq!(escape("l1\nl2"), "l1\\nl2");
    }
}
