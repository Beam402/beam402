//! The event, riding on the bus thread.
//!
//! [`beam402_event`] knows the arithmetic of a meeting and nothing about a bus;
//! [`crate::live`] knows the bus and nothing about a meeting. This is the join,
//! and it is one direction only: an on-deck pair becomes the [`Pairing`] a round
//! is run with, and a finished round becomes a line in a file.
//!
//! ## The operator records, the machine proposes
//!
//! Same rule as arming, for the same reason. The timing system can say which lane
//! took the stripe; it cannot know that the car in it was in the right class, or
//! that a protest is standing. So a completed round sits there with its result
//! showing until somebody holding control says to write it down — and what gets
//! written is what the beams measured, never a re-derivation of it.
//!
//! ## What it refuses
//!
//! A result the timing system could not decide is **not recorded**. Nobody's day
//! ends because a poll cycle came back empty. The log is a text file precisely so
//! that the answer in that case is a human appending a line to it, which is a
//! thing the format supports rather than a thing it works around.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use beam402_event::{round_name, EntryId, OnDeck, Progress, Seed, Sheet};
use beam402_protocol::Lane;
use beam402_race::{decide, foul, Foul, Outcome, Pairing, Round};

use crate::live::escape;

/// What is on the line: one car, or two on a practice pass.
///
/// **Not a shrunk eliminator.** There is no seed and no position — a seed is what
/// qualifying produces, so it cannot also be how qualifying identifies a car. The
/// entry number is enough, and it is what every `Q` line in the log names.
///
/// Two cars are how a practice day actually runs: two roll up, both go, and each
/// gets its own line. They are not a pair — nobody advances and nothing is won,
/// which is why this is a list of cars rather than a [`OnDeck`].
#[derive(Clone, Debug)]
pub struct OnLine {
    pub class: String,
    pub cars: Vec<(EntryId, Lane)>,
}

impl OnLine {
    fn lane_of(&self, entry: EntryId) -> Option<Lane> {
        self.cars.iter().find(|(e, _)| *e == entry).map(|&(_, l)| l)
    }
}

pub struct Meeting {
    day: Progress,
    log: PathBuf,
    /// The car on the line, while any class is still qualifying. Mutually
    /// exclusive with `deck`: a run has either a pair in it or one car.
    line: Option<OnLine>,
    deck: Option<OnDeck>,
    /// Who is in which lane, from [`Progress::lanes_for`] — the one place that
    /// correspondence is decided, so that a winning *lane* off the bus can become
    /// a winning *seed* in the log without two functions having to agree.
    lanes: Vec<(Lane, Seed, EntryId)>,
    swap: bool,
    /// How many pairs were in the round this pair came from, kept so the round's
    /// name stays put: recording the last pair advances the class, and the panel
    /// must not rename the round the operator is still looking at.
    deck_pairs: usize,
    /// The lane running a **single**: the other car could not make the call, so
    /// this pair goes down the track with one car in it. Named by lane rather than
    /// by seed because the tree is told lanes, and cleared by [`Meeting::load`]
    /// with everything else about the pair.
    single: Option<Lane>,
    /// The line written for the pair currently on deck, if it has been recorded.
    recorded: Option<String>,
    skipped: usize,
}

impl Meeting {
    /// Open a meeting: the entry sheet, and the log it is being written to.
    ///
    /// A log that does not exist yet is a day that has not started, not an error.
    /// One that exists is replayed, and that is the whole of the restart story —
    /// there is no other state to recover.
    pub fn open(sheet: &str, log: &Path) -> Result<Meeting, String> {
        let sheet = Sheet::parse(sheet).map_err(|e| e.to_string())?;
        let text = match std::fs::read_to_string(log) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", log.display())),
        };
        let (day, skipped) = Progress::replay(sheet, &text);
        let mut m = Meeting {
            day,
            log: log.to_path_buf(),
            line: None,
            deck: None,
            lanes: Vec::new(),
            deck_pairs: 0,
            swap: false,
            single: None,
            recorded: None,
            skipped,
        };
        m.load();
        Ok(m)
    }

    /// Pick up whatever is next. Called at startup and after every recorded pair,
    /// so the queue is always read from the ladder rather than tracked alongside
    /// it.
    fn load(&mut self) {
        self.recorded = None;
        self.single = None;

        // **Qualifying first, and across every class.** A club runs time trials
        // for everybody and then eliminations, so a class drawn early waits for
        // the rest to finish qualifying. That is a real limitation and a chosen
        // one: the alternative is guessing an interleaving nobody asked for, and
        // an operator who wants one can already get it by drawing a class late.
        let qualifying = self.day.qualifying().map(str::to_string);
        self.line = qualifying.and_then(|class| {
            self.day.next_on_the_line(&class).map(|entry| OnLine {
                class,
                // One car, in lane 1, and `swap` moves it. Which lane a solo car
                // uses is a track decision — a shut-down that drains better, a beam
                // being cleaned — not a rule. A second car is the operator's
                // `call`, because only they can see that two rolled up.
                cars: vec![(entry, Lane::L1)],
            })
        });
        if self.line.is_some() {
            self.deck = None;
            self.lanes = Vec::new();
            self.deck_pairs = 0;
            self.swap = false;
            return;
        }

        self.deck = self.day.next_pair();
        self.swap = match &self.deck {
            // Lane choice is a right; putting its holder in lane 1 is the common
            // exercise of it and the operator can still swap. Defaulting the
            // other way would mean the holder had to ask for what they had won.
            Some(d) => d.lane_choice.is_some() && d.lane_choice == d.right,
            None => false,
        };
        self.lanes = match &self.deck {
            Some(d) => self.day.lanes_for(d, self.swap),
            None => Vec::new(),
        };
        self.deck_pairs = self
            .deck
            .as_ref()
            .and_then(|d| self.day.round(&d.class))
            .map_or(0, |r| r.pairs.len());
    }

    /// What is on deck. The runtime does not ask — it takes the pairing and asks
    /// [`Meeting::nothing_to_run`] — but this is how a test says which shape of run
    /// it is driving, and it is the question a reader has here.
    #[allow(dead_code)]
    pub fn deck(&self) -> Option<&OnDeck> {
        self.deck.as_ref()
    }

    /// The pairing for what is on deck: lanes from the operator's choice, dials
    /// from the entry sheet, format from the class.
    pub fn pairing(&self) -> Result<Pairing, String> {
        if let Some(line) = &self.line {
            return self
                .day
                .line_for(&line.class, &line.cars)
                .map_err(|e| e.to_string());
        }
        let deck = self.deck.as_ref().ok_or("nothing is on deck")?;
        // A single is one car with its own dial and its class's format, which is
        // exactly what a car on the line is — so it is built the same way rather
        // than by taking a pair apart.
        if let Some(lane) = self.single {
            let (_, _, id) = self
                .lanes
                .iter()
                .copied()
                .find(|&(l, ..)| l == lane)
                .ok_or("that lane is not in this pair")?;
            return self
                .day
                .line_for(&deck.class, &[(id, lane)])
                .map_err(|e| e.to_string());
        }
        self.day
            .pairing_for(deck, self.swap)
            .map_err(|e| e.to_string())
    }

    /// Run this pair with one car in it, or put the other car back.
    ///
    /// **A single, not a bye.** The other car could not make the call — it broke,
    /// it did not show — so the ladder still has two seeds in this pair and the
    /// record is a win rather than a bye. What changes is what the tree is waiting
    /// for, which is the one thing the timing system cannot work out for itself.
    ///
    /// A toggle, so putting the other car back needs no second verb: a crew that
    /// fixes the car in the lanes is an ordinary afternoon.
    pub fn single(&mut self, lane: Lane) -> Result<(), String> {
        if self.recorded.is_some() {
            return Err("that pair has been recorded — it has already been raced".into());
        }
        let deck = self.deck.as_ref().ok_or("nothing is on deck")?;
        if deck.is_bye() {
            return Err("that pair is already a bye — there is one car in it".into());
        }
        if !self.lanes.iter().any(|&(l, ..)| l == lane) {
            return Err(format!("lane {} is not in this pair", lane.number()));
        }
        self.single = if self.single == Some(lane) {
            None
        } else {
            Some(lane)
        };
        Ok(())
    }

    /// The car on the line, while a class is qualifying. Same standing as
    /// [`Meeting::deck`].
    #[allow(dead_code)]
    pub fn line(&self) -> Option<&OnLine> {
        self.line.as_ref()
    }

    /// Nothing left to run: no car on the line and no pair on deck.
    ///
    /// Not the same question as "is there a pair", which is what the arming
    /// interlock used to ask. A day in qualifying has no pair either, and refusing
    /// to arm for that reason refuses every time trial there is.
    pub fn nothing_to_run(&self) -> bool {
        self.line.is_none() && self.deck.is_none()
    }

    /// Put a particular car on the line, in a particular lane.
    ///
    /// The derived queue is a default, not an order of running: cars arrive at the
    /// lanes in whatever order they arrive, and a queue an operator cannot
    /// override is the "shrunk eliminator" this is not supposed to be. Any entry in
    /// the qualifying class will do, however many passes it has had.
    ///
    /// Calling into the *other* lane is how a practice pass gets its second car.
    /// Calling a car that is already on the line moves it, and calling into an
    /// occupied lane replaces whoever was in it — both are what the words mean, and
    /// neither needs a separate button.
    pub fn call(&mut self, entry: EntryId, lane: Lane) -> Result<(), String> {
        let line = self.line.as_ref().ok_or("no class is qualifying")?;
        if self.recorded.is_some() {
            return Err("that pass has been recorded — clear the round first".into());
        }
        let class = line.class.clone();
        if !self
            .day
            .sheet()
            .entries_in(&class)
            .iter()
            .any(|e| e.id == entry)
        {
            return Err(format!("#{} is not entered in {class}", entry.0));
        }
        let Some(line) = self.line.as_mut() else {
            return Err("no class is qualifying".into());
        };
        line.cars.retain(|&(e, l)| e != entry && l != lane);
        line.cars.push((entry, lane));
        // Lane order, so lane 1 reads first everywhere it is shown.
        line.cars.sort_by_key(|&(_, l)| l.number());
        Ok(())
    }

    /// Close qualifying and draw the ladder.
    ///
    /// The operator's call, like arming and recording. Nothing here counts passes
    /// to decide qualifying is over: how many a club gives is a club's business,
    /// and the moment it ends is the moment somebody says so.
    pub fn draw(&mut self) -> Result<String, String> {
        let class = match &self.line {
            Some(line) => line.class.clone(),
            None => return Err("no class is qualifying".into()),
        };
        if self.day.attempts(&class).is_empty() {
            return Err(format!(
                "{class} has no qualifying passes — a ladder drawn on nothing seeds by entry \
                 number, which is a draw nobody agreed to"
            ));
        }
        let record = self.day.draw(&class).map_err(|e| e.to_string())?;
        let line = record.line();
        self.append(&line)?;
        self.load();
        Ok(line)
    }

    /// Exchange lanes. Refused once a result is in the log, because the pair it
    /// describes has already been raced.
    pub fn swap(&mut self) -> Result<(), String> {
        if self.recorded.is_some() {
            return Err(
                "that pair has been recorded — swapping lanes would describe a different race"
                    .into(),
            );
        }
        // On the line this is "the other lane" for one car and an exchange for two.
        if let Some(line) = self.line.as_mut() {
            for (_, lane) in line.cars.iter_mut() {
                *lane = match *lane {
                    Lane::L1 => Lane::L2,
                    Lane::L2 => Lane::L1,
                };
            }
            line.cars.sort_by_key(|&(_, l)| l.number());
            return Ok(());
        }
        let deck = self.deck.clone().ok_or("nothing is on deck")?;
        self.swap = !self.swap;
        self.lanes = self.day.lanes_for(&deck, self.swap);
        // The single follows the car. A swap exchanges which lane each seed is in,
        // so leaving the lane alone here would silently change *who* is running
        // alone — which is the one thing this must not do quietly.
        self.single = self.single.map(|l| match l {
            Lane::L1 => Lane::L2,
            Lane::L2 => Lane::L1,
        });
        Ok(())
    }

    /// Whether a completed round has a result nobody has written down. This is
    /// what stops "next pair" from quietly discarding one.
    pub fn owes_a_record(&self, round: &Round, pairing: &Pairing) -> bool {
        if self.recorded.is_some() {
            return false;
        }
        // A qualifying pass is owed a line as soon as the car went, whether or not
        // it got to the other end: a car that stops on the track still qualifies,
        // at the back. What is *not* owed a line is a lane that produced nothing.
        if let Some(line) = &self.line {
            return line.cars.iter().any(|&(_, lane)| {
                round
                    .lane(lane)
                    .is_some_and(|r| r.has_time() || r.reaction_s.is_some())
            });
        }
        let Some(deck) = &self.deck else { return false };
        // A single is asked the bye's question for the bye's reason: there is
        // nobody in the other lane to lose to.
        if let Some(lane) = self.single {
            return round.lane(lane).is_some_and(|r| r.has_time());
        }
        if deck.is_bye() {
            // Same question `record` asks: did the car make a pass. Going through
            // the outcome here would let `Next` walk past a bye that broke out.
            return pairing
                .entries()
                .first()
                .and_then(|e| round.lane(e.lane))
                .is_some_and(|r| r.has_time());
        }
        matches!(decide(round, pairing), Outcome::Win { .. })
    }

    /// Write the result down and advance the ladder.
    ///
    /// The outcome comes from [`decide`]; nothing here re-derives it. All this
    /// does is turn a lane into a seed, which is the one translation the bus side
    /// cannot make on its own.
    pub fn record(&mut self, round: &Round, pairing: &Pairing) -> Result<String, String> {
        if let Some(written) = &self.recorded {
            return Err(format!("already recorded: {written}"));
        }

        // A qualifying pass. No winner to decide and no seed to translate: the
        // attempt is what the beams measured against what the driver dialled, and
        // an entry that did not finish still qualifies — at the back.
        if let Some(on_line) = self.line.clone() {
            // One line per car that went. A practice pass with two cars is two
            // attempts and nothing more — no winner, no seed, nobody advancing — so
            // this is a loop rather than a second kind of record.
            //
            // Appended one at a time rather than gathered and flushed: if the second
            // append fails, the file holds the first pass and a restart replays it,
            // which leaves a car to re-record instead of a car to remember.
            let mut lines = Vec::new();
            for (entry, lane) in on_line.cars {
                let Some(run) = round.lane(lane) else {
                    continue;
                };
                if !(run.has_time() || run.reaction_s.is_some()) {
                    continue;
                }
                // The dial the tree was actually told, not one looked up again here.
                let dial_s = pairing
                    .entries()
                    .iter()
                    .find(|e| e.lane == lane)
                    .and_then(|e| e.dial_s);
                let record = self
                    .day
                    .qualified(&on_line.class, entry, run.et_s, dial_s, run.is_red())
                    .map_err(|e| e.to_string())?;
                let written = record.line();
                self.append(&written)?;
                lines.push(written);
            }
            if lines.is_empty() {
                return Err(
                    "nothing in either lane — either the car did not go or a beam did \
                     not see it, and the two have opposite consequences, so it has to \
                     be written by hand"
                        .into(),
                );
            }
            let line = lines.join("\n");
            self.recorded = Some(line.clone());
            return Ok(line);
        }

        let deck = self.deck.clone().ok_or("nothing is on deck")?;

        // A single. The same question a bye is asked, for the same reason — there
        // is nobody in the other lane to lose to, so a red light or a breakout does
        // not cost the round and `decide` would stall the class by calling it a
        // no-contest. But the *record* is a win: the pair has two seeds in it, one
        // of them could not make the call, and a ladder that called this a bye
        // would be describing a draw that never happened.
        if let Some(lane) = self.single {
            if !round.lane(lane).is_some_and(|r| r.has_time()) {
                return Err(
                    "no timed pass on the single — that is either a car that did \
                            not go or a beam that did not see it, and the two have \
                            opposite consequences, so it has to be written by hand"
                        .into(),
                );
            }
            let seed = self
                .lanes
                .iter()
                .find(|&&(l, ..)| l == lane)
                .map(|&(_, seed, _)| seed)
                .ok_or_else(|| format!("lane {} is not in this pair", lane.number()))?;
            let record = self
                .day
                .won(&deck.class, deck.position, seed)
                .map_err(|e| e.to_string())?;
            let line = record.line();
            self.append(&line)?;
            return self.finish(&deck, round, pairing, line);
        }

        // A bye is asked a different question. `completed` means the car made a
        // full pass — a fact the beams answer — and not whether the pass was
        // clean: you cannot lose to nobody, and whether a breakout on a bye costs
        // the round is a class rule rather than a thing this plumbing decides. So
        // it is `has_time`, not the outcome, and a bye that broke out still
        // advances instead of stalling the class.
        if deck.is_bye() {
            let lane = pairing
                .entries()
                .first()
                .map(|e| e.lane)
                .ok_or("a bye with no car in it")?;
            if !round.lane(lane).is_some_and(|r| r.has_time()) {
                return Err("no timed pass on the bye — that is either a car that did \
                            not go or a beam that did not see it, and the two have \
                            opposite consequences, so it has to be written by hand"
                    .into());
            }
            let record = self
                .day
                .bye(&deck.class, deck.position, true)
                .map_err(|e| e.to_string())?;
            let line = record.line();
            self.append(&line)?;
            return self.finish(&deck, round, pairing, line);
        }

        let record = match decide(round, pairing) {
            Outcome::Win { lane, .. } => {
                let seed = self
                    .lanes
                    .iter()
                    .find(|(l, ..)| *l == lane)
                    .map(|(_, seed, _)| *seed)
                    .ok_or_else(|| format!("lane {} is not in this pair", lane.number()))?;
                self.day.won(&deck.class, deck.position, seed)
            }
            // Deliberately not written as a loss. See the module note: a poll
            // cycle that came back empty must not end somebody's day, so this
            // says what it cannot decide and leaves the log alone.
            Outcome::NoContest => {
                return Err("the timing system cannot say who won — nothing recorded".into())
            }
        }
        .map_err(|e| e.to_string())?;

        let line = record.line();
        self.append(&line)?;
        self.finish(&deck, round, pairing, line)
    }

    /// The result is written; note why, and remember what was written.
    fn finish(
        &mut self,
        deck: &OnDeck,
        round: &Round,
        pairing: &Pairing,
        result: String,
    ) -> Result<String, String> {
        let mut lines = vec![result];
        lines.extend(self.note_fouls(deck, round, pairing)?);
        let written = lines.join("\n");
        self.recorded = Some(written.clone());
        Ok(written)
    }

    /// Write down what the beams measured about *why* (**D37**).
    ///
    /// Called after the result, for every lane in the run — including a lane that
    /// fouled and still won, and a bye or a single, where the foul costs nothing.
    /// That is the point: the rule this exists for counts a driver's red lights
    /// across a whole competition, and a red light that cost nothing is still one.
    ///
    /// Read with the same [`foul`] `decide` uses, so there is no second opinion
    /// about what a foul is.
    fn note_fouls(
        &mut self,
        deck: &OnDeck,
        round: &Round,
        pairing: &Pairing,
    ) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        for (lane, _, entry) in self.lanes.clone() {
            // Only the lanes this run actually had cars in: a single leaves the
            // other one empty, and an empty lane cannot foul.
            if !pairing.entries().iter().any(|e| e.lane == lane) {
                continue;
            }
            let Some(run) = round.lane(lane) else {
                continue;
            };
            let Some(f) = foul(run, pairing.breakout_limit(lane)) else {
                continue;
            };
            let kind = match f {
                Foul::RedLight { .. } => "red",
                Foul::Breakout { .. } => "breakout",
            };
            let record = self
                .day
                .fouled(
                    &deck.class,
                    deck.round,
                    deck.position,
                    entry,
                    kind,
                    Some(f.amount()),
                )
                .map_err(|e| e.to_string())?;
            let line = record.line();
            self.append(&line)?;
            lines.push(line);
        }
        Ok(lines)
    }

    /// Append one line, opening and closing the file around it.
    ///
    /// Held open would be faster and would also mean a killed process losing
    /// whatever the buffer still had. This runs a handful of times an hour and the
    /// thing it is protecting is a day of racing.
    fn append(&self, line: &str) -> Result<(), String> {
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)
            .map_err(|e| format!("{}: {e}", self.log.display()))?;
        writeln!(f, "{line}").map_err(|e| format!("{}: {e}", self.log.display()))?;
        f.flush()
            .map_err(|e| format!("{}: {e}", self.log.display()))
    }

    /// Move to the next pair on the ladder.
    pub fn next(&mut self) {
        self.load();
    }

    /// The panel: what is on deck, and the round it belongs to.
    pub fn json(&self) -> String {
        // Qualifying: one car, and the queue behind it. The queue is sent whole so
        // the operator can call any of it rather than only accept the next — and so
        // the panel can show what a driver wants to know, which is the pass they
        // are currently in on.
        if let Some(on_line) = &self.line {
            let attempts = self.day.attempts(&on_line.class);
            let queue: Vec<String> = self
                .day
                .sheet()
                .entries_in(&on_line.class)
                .into_iter()
                .map(|e| {
                    let mine: Vec<&beam402_event::Attempt> =
                        attempts.iter().filter(|a| a.entry == e.id).collect();
                    let best = mine
                        .iter()
                        .filter_map(|a| a.et_s)
                        .fold(f64::INFINITY, f64::min);
                    format!(
                        "{{\"number\":{},\"who\":\"{}\",\"runs\":{},\"best\":{},\"lane\":{}}}",
                        e.id.0,
                        escape(&self.day.driver(e.id)),
                        mine.len(),
                        if best.is_finite() {
                            format!("{best:.4}")
                        } else {
                            "null".into()
                        },
                        // The lane it is in, so the panel can offer the other one.
                        on_line
                            .lane_of(e.id)
                            .map_or("null".to_string(), |l| l.number().to_string()),
                    )
                })
                .collect();
            let cars: Vec<String> = on_line
                .cars
                .iter()
                .map(|&(id, lane)| {
                    format!(
                        "{{\"lane\":{},\"who\":\"{}\",\"dial\":{}}}",
                        lane.number(),
                        escape(&self.day.driver(id)),
                        self.day
                            .sheet()
                            .entry(id)
                            .and_then(|e| e.dial_s)
                            .map_or("null".to_string(), |d| format!("{d:.4}")),
                    )
                })
                .collect();
            return format!(
                "{{\"on\":true,\"phase\":\"qualifying\",\"class\":\"{}\",\
\"round\":\"qualifying\",\"recorded\":{},\"skipped\":{},\
\"cars\":[{}],\"queue\":[{}]}}",
                escape(&on_line.class),
                match &self.recorded {
                    Some(l) => format!("\"{}\"", escape(l)),
                    None => "null".into(),
                },
                self.skipped,
                cars.join(","),
                queue.join(",")
            );
        }

        let Some(deck) = &self.deck else {
            let champions: Vec<String> = self
                .day
                .class_names()
                .filter_map(|c| {
                    let seed = self.day.champion(c)?;
                    let id = self.day.field(c)?.entry(seed)?;
                    Some(format!(
                        "{{\"class\":\"{}\",\"who\":\"{}\"}}",
                        escape(c),
                        escape(&self.day.driver(id))
                    ))
                })
                .collect();
            return format!(
                "{{\"on\":false,\"skipped\":{},\"champions\":[{}]}}",
                self.skipped,
                champions.join(",")
            );
        };

        // Only while the ladder is still *on* the round this pair belongs to.
        // Recording the last pair of a round advances the class immediately, and
        // drawing the next round's bracket against a pair that has not been
        // cleared yet showed the operator a round nobody has run.
        let pairs = self
            .day
            .round(&deck.class)
            .filter(|r| r.number == deck.round)
            .map(|r| {
                r.pairs
                    .iter()
                    .map(|p| {
                        format!(
                            "{{\"position\":{},\"left\":{},\"right\":{},\"won\":{}}}",
                            p.position,
                            p.left,
                            p.right.map_or("null".into(), |r| r.to_string()),
                            r.winner(p.position)
                                .map_or("null".into(), |w| w.to_string()),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        // From the count taken when the pair was picked up, so the name does not
        // change under the operator between recording and clearing.
        let round = round_name(self.deck_pairs, deck.round);

        // The whole field, so the panel can name every seed on the ladder and not
        // only the two cars in front of the operator.
        let field = self
            .day
            .field(&deck.class)
            .map(|f| {
                f.seeds()
                    .map(|(seed, id)| {
                        format!(
                            "{{\"seed\":{seed},\"who\":\"{}\"}}",
                            escape(&self.day.driver(id))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();

        let cars = self
            .lanes
            .iter()
            .map(|(lane, seed, id)| {
                // Red lights so far, across the whole day and qualifying included
                // (**D37**). Shown rather than acted on: a rulebook that ends
                // somebody's day on the third one is applied by an official, and
                // this is what puts "two already" in front of them.
                format!(
                    "{{\"lane\":{},\"seed\":{seed},\"who\":\"{}\",\"choice\":{},\"reds\":{}}}",
                    lane.number(),
                    escape(&self.day.driver(*id)),
                    deck.lane_choice == Some(*seed),
                    self.day.fouls_of(*id, "red"),
                )
            })
            .collect::<Vec<_>>()
            .join(",");

        format!(
            "{{\"on\":true,\"phase\":\"eliminations\",\"class\":\"{}\",\
\"round\":\"{round}\",\"position\":{},\
\"bye\":{},\"single\":{},\"swapped\":{},\"recorded\":{},\"skipped\":{},\
\"cars\":[{cars}],\"pairs\":[{pairs}],\"field\":[{field}]}}",
            escape(&deck.class),
            deck.position,
            deck.is_bye(),
            self.single
                .map_or("null".to_string(), |l| l.number().to_string()),
            self.swap,
            match &self.recorded {
                Some(l) => format!("\"{}\"", escape(l)),
                None => "null".into(),
            },
            self.skipped,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_race::{Format, LaneRun};

    const SHEET: &str = r#"
[event]
name = "Club day"
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

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("beam402-{name}-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// A round where the given lane won on the stripe by a tenth. Both cars left
    /// together, so the margin is the difference in their ETs.
    fn round_won_by(lane: Lane) -> Round {
        let mut r = Round::default();
        r.launch_margin_s = Some(0.0);
        for l in Lane::ALL {
            r.set_lane(
                l,
                LaneRun {
                    reaction_s: Some(0.500),
                    et_s: Some(if l == lane { 10.000 } else { 10.100 }),
                    ..LaneRun::default()
                },
            );
        }
        r
    }

    fn open(log: &Path) -> Meeting {
        let mut m = Meeting::open(SHEET, log).unwrap();
        // Qualify the field so there is a ladder to run.
        for (n, et) in [(1u32, 9.91), (2, 9.95), (3, 9.99), (4, 10.20)] {
            let r = m
                .day
                .qualified("Super Gas", EntryId(n), Some(et), None, false)
                .unwrap();
            m.append(&r.line()).unwrap();
        }
        let r = m.day.draw("Super Gas").unwrap();
        m.append(&r.line()).unwrap();
        m.next();
        m
    }

    #[test]
    fn a_winning_lane_becomes_a_winning_seed() {
        // The whole point of the module. The bus can only say "lane 1"; the log
        // has to say which car that was.
        let log = tmp("lane-to-seed");
        let mut m = open(&log);
        let deck = m.deck().cloned().expect("a pair on deck");
        let pairing = m.pairing().unwrap();

        // Pro ladder, four cars: 1 v 4. Whoever is in lane 1 is the seed the
        // record must name.
        let expected = m
            .lanes
            .iter()
            .find(|(l, ..)| *l == Lane::L1)
            .map(|(_, s, _)| *s)
            .unwrap();
        let line = m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        assert!(
            line.starts_with(&format!("W Super_Gas {} {} {expected}", 1, deck.position)),
            "{line}"
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn swapping_lanes_swaps_which_seed_a_lane_records() {
        // The failure this guards: lane choice exercised, the result read off the
        // bus unchanged, and the wrong car advanced.
        let log = tmp("swap");
        let mut m = open(&log);
        let seed_in_lane_1 = |m: &Meeting| {
            m.lanes
                .iter()
                .find(|(l, ..)| *l == Lane::L1)
                .map(|(_, s, _)| *s)
                .unwrap()
        };
        let before = seed_in_lane_1(&m);
        m.swap().unwrap();
        assert_ne!(before, seed_in_lane_1(&m), "the cars changed places");

        // And the log follows: lane 1 winning now names the other car.
        let pairing = m.pairing().unwrap();
        let line = m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        assert!(
            line.ends_with(&format!(" {}", seed_in_lane_1(&m))),
            "{line} should name seed {}",
            seed_in_lane_1(&m)
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_result_the_timing_cannot_decide_is_not_written() {
        // Nobody's day ends because a poll cycle came back empty.
        let log = tmp("nocontest");
        let mut m = open(&log);
        let pairing = m.pairing().unwrap();
        let err = m.record(&Round::default(), &pairing).unwrap_err();
        assert!(err.contains("cannot say who won"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .filter(|l| l.starts_with('W'))
                .count(),
            0
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_pair_cannot_be_recorded_twice() {
        let log = tmp("twice");
        let mut m = open(&log);
        let pairing = m.pairing().unwrap();
        m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        assert!(m
            .record(&round_won_by(Lane::L1), &pairing)
            .unwrap_err()
            .contains("already recorded"));
        assert!(m.swap().is_err(), "and lanes cannot be swapped after it");
        std::fs::remove_file(&log).ok();
    }

    /// A car that could not make the call. Its opponent runs alone, has to make a
    /// timed pass to advance, and the ladder records a **win** rather than a bye —
    /// two seeds were drawn into that pair and only one of them raced.
    #[test]
    fn a_single_runs_one_car_and_records_a_win() {
        let log = tmp("single");
        let mut m = open(&log);
        let deck = m.deck().cloned().expect("a pair on deck");
        let alone = Lane::L1;
        let seed = m
            .lanes
            .iter()
            .find(|&&(l, ..)| l == alone)
            .map(|&(_, s, _)| s)
            .unwrap();

        m.single(alone).unwrap();
        let pairing = m.pairing().unwrap();
        assert_eq!(pairing.entries().len(), 1, "the tree waits for one car");
        assert_eq!(pairing.entries()[0].lane, alone);

        // A single that did not make a pass is not a result: it is a car that did
        // not go or a beam that did not see it, and only a person can tell.
        assert!(m
            .record(&Round::default(), &pairing)
            .unwrap_err()
            .contains("no timed pass on the single"));

        // One that did is a win for the seed that was in that lane.
        let line = m.record(&round_won_by(alone), &pairing).unwrap();
        assert_eq!(
            line,
            format!("W Super_Gas 1 {} {seed}", deck.position),
            "a win, not a bye"
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_single_follows_the_car_when_the_lanes_are_swapped() {
        // The quiet failure this guards: the operator marks lane 1 as running
        // alone, then swaps lanes for some other reason, and the *other* car is
        // silently the one that runs.
        let log = tmp("single-swap");
        let mut m = open(&log);
        let who = |m: &Meeting, lane: Lane| {
            m.lanes
                .iter()
                .find(|&&(l, ..)| l == lane)
                .map(|&(_, s, _)| s)
                .unwrap()
        };
        m.single(Lane::L1).unwrap();
        let running = who(&m, Lane::L1);
        m.swap().unwrap();
        assert_eq!(
            m.pairing().unwrap().entries()[0].lane,
            Lane::L2,
            "the car moved lanes"
        );
        assert_eq!(who(&m, Lane::L2), running, "and it is the same car running");

        // Pressing it again puts the other car back, because crews fix things.
        m.single(Lane::L2).unwrap();
        assert_eq!(m.pairing().unwrap().entries().len(), 2);
        std::fs::remove_file(&log).ok();
    }

    /// A practice day: two cars roll up, both go, and each gets its own line.
    /// Nothing is won, nobody advances — which is why this is a list of cars and
    /// not a pair.
    #[test]
    fn two_cars_on_a_practice_pass_are_two_attempts_and_nothing_else() {
        let log = tmp("practice");
        // Nothing drawn, so the day is on the line: one car, lane 1, from the queue.
        let mut m = Meeting::open(SHEET, &log).unwrap();
        let on = m.line().expect("a car on the line").clone();
        assert_eq!(on.cars, vec![(EntryId(1), Lane::L1)]);

        m.call(EntryId(2), Lane::L2).unwrap();
        let pairing = m.pairing().unwrap();
        assert_eq!(pairing.entries().len(), 2, "two cars in one run");

        let written = m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2, "one line per car: {written}");
        assert!(
            lines.iter().all(|l| l.starts_with("Q Super_Gas ")),
            "attempts, not a result: {written}"
        );
        assert!(written.contains("Q Super_Gas 1 "), "{written}");
        assert!(written.contains("Q Super_Gas 2 "), "{written}");

        // On disk, so a restart replays both.
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().filter(|l| l.starts_with("Q ")).count(), 2);
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn calling_a_car_moves_it_rather_than_copying_it() {
        let log = tmp("call");
        let mut m = Meeting::open(SHEET, &log).unwrap();
        m.call(EntryId(2), Lane::L2).unwrap();
        assert_eq!(m.line().unwrap().cars.len(), 2);

        // Into the lane the other car is in: it moves, and the line is one car
        // again. Two entries in one lane is not a thing a strip can do.
        m.call(EntryId(2), Lane::L1).unwrap();
        assert_eq!(m.line().unwrap().cars, vec![(EntryId(2), Lane::L1)]);

        // A third car in the other lane, and `swap` exchanges them.
        m.call(EntryId(3), Lane::L2).unwrap();
        m.swap().unwrap();
        let on = m.line().unwrap();
        assert_eq!(on.lane_of(EntryId(2)), Some(Lane::L2));
        assert_eq!(on.lane_of(EntryId(3)), Some(Lane::L1));

        // And a number nobody entered is refused by number, because the person
        // reading the refusal is holding the entry list.
        let err = m.call(EntryId(99), Lane::L1).unwrap_err();
        assert!(err.contains("#99"), "{err}");
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn the_ladder_moves_on_and_the_next_pair_is_ready() {
        let log = tmp("advance");
        let mut m = open(&log);
        let first = m.deck().unwrap().position;
        let pairing = m.pairing().unwrap();
        m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        assert!(!m.owes_a_record(&round_won_by(Lane::L1), &pairing));
        m.next();
        assert_ne!(
            m.deck().expect("the other half of round one").position,
            first
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_restart_picks_the_meeting_up_where_it_was() {
        // The claim the log format exists to support, over a real file.
        let log = tmp("restart");
        let mut m = open(&log);
        let pairing = m.pairing().unwrap();
        m.record(&round_won_by(Lane::L1), &pairing).unwrap();
        let expected = {
            m.next();
            m.deck().unwrap().position
        };

        let reopened = Meeting::open(SHEET, &log).unwrap();
        assert_eq!(reopened.skipped, 0);
        assert_eq!(
            reopened.deck().expect("still a pair to run").position,
            expected
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn recording_the_last_pair_of_a_round_does_not_rename_the_round() {
        // Seen on the operator page: the class advances the instant the last pair
        // is recorded, so the panel relabelled the round the operator was still
        // looking at and drew a bracket nobody had run.
        let log = tmp("relabel");
        let mut m = open(&log);
        for _ in 0..2 {
            let pairing = m.pairing().unwrap();
            m.record(&round_won_by(Lane::L1), &pairing).unwrap();
            if m.deck().is_some() {
                let json = m.json();
                assert!(json.contains("\"round\":\"semi-final\""), "{json}");
            }
            m.next();
        }
        // And once the operator clears it, the final is the final.
        assert!(m.json().contains("\"round\":\"final\""), "{}", m.json());
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn a_bye_that_broke_out_still_advances() {
        // Found by running a meeting: a bye is scored against a dial like any run,
        // so an unopposed car quicker than its dial came back `NoContest` and the
        // class stopped. You cannot lose to nobody — `completed` asks whether the
        // pass happened, and that is a question the beams answer.
        const THREE: &str = r#"
[event]
name = "Byes"
date = "2026-08-15"
[[class]]
name = "Bracket"
format = "bracket"
seeding = "closest-to-dial"
ladder = "pro"
[[entry]]
number = 1
driver = "A"
class = "Bracket"
dial_s = 13.50
[[entry]]
number = 2
driver = "B"
class = "Bracket"
dial_s = 12.00
[[entry]]
number = 3
driver = "C"
class = "Bracket"
dial_s = 11.00
"#;
        let log = tmp("bye-breakout");
        let mut m = Meeting::open(THREE, &log).unwrap();
        let r = m.day.draw("Bracket").unwrap();
        m.append(&r.line()).unwrap();
        m.next();

        // Walk to the bye. Three cars on a pro ladder means one of the two pairs
        // has nobody in the other lane.
        while !m.deck().expect("a pair").is_bye() {
            let pairing = m.pairing().unwrap();
            m.record(&round_won_by(Lane::L1), &pairing).unwrap();
            m.next();
        }
        let deck = m.deck().unwrap().clone();
        let pairing = m.pairing().unwrap();
        assert!(pairing.is_bye());

        // A pass well under the dial: a breakout, and irrelevant on a bye.
        let mut round = Round::default();
        round.set_lane(
            pairing.entries()[0].lane,
            LaneRun {
                reaction_s: Some(0.500),
                et_s: Some(9.000),
                ..LaneRun::default()
            },
        );
        assert!(matches!(decide(&round, &pairing), Outcome::NoContest));
        assert!(
            m.owes_a_record(&round, &pairing),
            "and it is owed, not skipped"
        );
        let line = m.record(&round, &pairing).unwrap();
        // The bye advances **and** the breakout is written down (**D37**). It cost
        // nothing here — there is nobody to lose to — and it is still a foul the
        // driver committed, which is the whole reason a rulebook can count them.
        let mut lines = line.lines();
        assert_eq!(
            lines.next().unwrap(),
            format!("B Bracket 1 {} run", deck.position)
        );
        assert_eq!(
            lines.next().unwrap(),
            format!("F Bracket 1 {} 1 breakout 4.5000", deck.position)
        );
        assert_eq!(lines.next(), None);

        // No pass at all is the case that does need a human.
        let log2 = tmp("bye-notime");
        let mut m = Meeting::open(THREE, &log2).unwrap();
        let r = m.day.draw("Bracket").unwrap();
        m.append(&r.line()).unwrap();
        m.next();
        while !m.deck().expect("a pair").is_bye() {
            let pairing = m.pairing().unwrap();
            m.record(&round_won_by(Lane::L1), &pairing).unwrap();
            m.next();
        }
        let pairing = m.pairing().unwrap();
        assert!(m
            .record(&Round::default(), &pairing)
            .unwrap_err()
            .contains("written by hand"));
        std::fs::remove_file(&log).ok();
        std::fs::remove_file(&log2).ok();
    }

    #[test]
    fn an_index_class_carries_its_index_into_the_pairing() {
        // The dials come off the sheet, not off a flag — which is the reason the
        // event layer exists at all.
        let log = tmp("index");
        let m = open(&log);
        assert_eq!(
            m.pairing().unwrap().format(),
            Format::Index { seconds: 9.90 }
        );
        std::fs::remove_file(&log).ok();
    }
}
