//! Load-time validation, per [`protocol.md`] §5.
//!
//! Every rule here exists because breaking it produces a **plausible-looking
//! wrong number** rather than a visible failure. That is why the master refuses
//! to start a round on a file that fails one, and why all problems are collected
//! rather than reporting the first: a file with three mistakes should say three.
//!
//! The rules split in a way §5 does not spell out but the master has to respect.
//! Six of them are decidable from the file alone. Three need facts from the bus —
//! which inputs a node actually has, whether it saw both pulses, which MACs are
//! present — so they cannot run until devices answer. And one cannot run at all
//! yet, because nothing in the register map publishes what it needs
//! ([`software.md`] §8 #7); it reports as *unchecked* rather than passing
//! quietly, because a rule that silently does not run is worse than a missing
//! rule.
//!
//! [`protocol.md`]: https://github.com/perfilev-dev/beam402/blob/main/docs/protocol.md
//! [`software.md`]: https://github.com/perfilev-dev/beam402/blob/main/docs/software.md

use std::collections::BTreeSet;

use beam402_protocol::{DeviceClass, Lane, RunRecord};

use crate::model::{Beam, Mac, Mapping};

/// Which §5 rule a problem belongs to. `Structural` covers facts the file must
/// satisfy to be interpretable at all — §5 takes them for granted.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Rule {
    Structural,
    /// Every mapped `(address, input)` exists in that node's `input_present`.
    InputExists,
    /// No beam meaning duplicated within a lane.
    NoDuplicateMeaning,
    /// `stage` and `finish` for every declared lane — the minimum system.
    MinimumSystem,
    /// `trap_base` present if any `trap_*` beam is mapped.
    TrapBase,
    /// `trap_entry` and `trap_exit` on the same node and lane, so the interval
    /// closes inside one timer.
    TrapOneTimer,
    /// Exactly one margin source, and it reports both pulses seen.
    MarginSource,
    /// Exactly two nodes flagged `terminated` (**D09**).
    Termination,
    /// `guard` mapped for every lane if it is mapped for any.
    GuardSymmetry,
    /// Every `crystal_ppm` belongs to a MAC actually present on the bus.
    CrystalMacPresent,
    /// Every mapped `(address, input, lane)` is one the node can actually
    /// capture. **Not evaluable** — see `software.md` §8 #7.
    CaptureReachable,
}

impl Rule {
    pub const fn label(self) -> &'static str {
        match self {
            Rule::Structural => "structural",
            Rule::InputExists => "§5.1 input-exists",
            Rule::NoDuplicateMeaning => "§5.2 no-duplicate-meaning",
            Rule::MinimumSystem => "§5.3 minimum-system",
            Rule::TrapBase => "§5.4 trap-base",
            Rule::TrapOneTimer => "§5.5 trap-one-timer",
            Rule::MarginSource => "§5.6 margin-source",
            Rule::Termination => "§5.7 termination",
            Rule::GuardSymmetry => "§5.8 guard-symmetry",
            Rule::CrystalMacPresent => "§5.9 crystal-mac-present",
            Rule::CaptureReachable => "§8#7 capture-reachable",
        }
    }

    /// Whether the rule can be decided without talking to the bus.
    pub const fn is_static(self) -> bool {
        matches!(
            self,
            Rule::Structural
                | Rule::NoDuplicateMeaning
                | Rule::MinimumSystem
                | Rule::TrapBase
                | Rule::TrapOneTimer
                | Rule::Termination
                | Rule::GuardSymmetry
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// The master refuses to start a round.
    Error,
    /// Recorded, does not block. Only the MAC mismatch of **D08**.
    Warning,
    /// The rule exists and did not run. Never a pass.
    Unchecked,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub rule: Rule,
    pub severity: Severity,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Unchecked => "unchecked",
        };
        write!(f, "{tag}: [{}] {}", self.rule.label(), self.message)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub problems: Vec<Problem>,
}

impl Report {
    fn push(&mut self, rule: Rule, severity: Severity, message: impl Into<String>) {
        self.problems.push(Problem {
            rule,
            severity,
            message: message.into(),
        });
    }

    fn error(&mut self, rule: Rule, message: impl Into<String>) {
        self.push(rule, Severity::Error, message);
    }

    pub fn errors(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Warning)
    }

    pub fn unchecked(&self) -> impl Iterator<Item = &Problem> {
        self.problems
            .iter()
            .filter(|p| p.severity == Severity::Unchecked)
    }

    /// True when nothing blocks a round. Warnings and unchecked rules do not.
    pub fn may_start_a_round(&self) -> bool {
        self.errors().next().is_none()
    }

    pub fn has(&self, rule: Rule) -> bool {
        self.problems.iter().any(|p| p.rule == rule)
    }

    pub fn absorb(&mut self, other: Report) {
        self.problems.extend(other.problems);
    }
}

/// What a device says about itself, as the bus-dependent rules need it. Built by
/// the master from the identity and pulse blocks — the rules do not do I/O.
#[derive(Clone, Copy, Debug)]
pub struct DeviceFacts {
    pub address: u8,
    pub mac: Mac,
    pub device_class: DeviceClass,
    pub protocol_version: u16,
    pub input_present: u16,
    /// `pulse_flags.seen_l1 && seen_l2` at the time the master looked.
    pub both_pulses_seen: bool,
}

impl DeviceFacts {
    pub fn has_input(&self, index: u8) -> bool {
        index < 16 && self.input_present & (1u16 << index) != 0
    }
}

/// Rules decidable from the file alone.
pub fn check_static(m: &Mapping) -> Report {
    let mut r = Report::default();
    structural(m, &mut r);
    no_duplicate_meaning(m, &mut r);
    minimum_system(m, &mut r);
    trap_rules(m, &mut r);
    termination(m, &mut r);
    guard_symmetry(m, &mut r);
    // §5.6 has a static half — the named node must exist in the file — and a bus
    // half, checked in check_against_bus.
    if m.node(m.margin.source_address).is_none() {
        r.error(
            Rule::MarginSource,
            format!(
                "margin.source_address {} is not a node in this file",
                m.margin.source_address
            ),
        );
    }
    r.push(
        Rule::CaptureReachable,
        Severity::Unchecked,
        "no register publishes which (input, lane) pairs a node can capture, so a \
         lane typo cannot be caught here — it reads as \"not seen this run\" and \
         quietly loses a split (software.md §8 #7)",
    );
    r
}

/// Rules that need the devices to have answered.
pub fn check_against_bus(m: &Mapping, facts: &[DeviceFacts]) -> Report {
    let mut r = Report::default();
    let find = |addr: u8| facts.iter().find(|f| f.address == addr);

    for node in &m.nodes {
        let Some(f) = find(node.address) else {
            r.error(
                Rule::InputExists,
                format!(
                    "node {} ({}) did not answer, so its inputs cannot be verified",
                    node.address, node.label
                ),
            );
            continue;
        };
        for input in &node.inputs {
            if !f.has_input(input.index) {
                r.error(
                    Rule::InputExists,
                    format!(
                        "node {} ({}) maps input {} as {}, but input_present is {:#06b}",
                        node.address, node.label, input.index, input.beam, f.input_present
                    ),
                );
            }
        }
        // D08: a swap in the field means copying DIP positions, so this is a
        // warning that gets the swap recorded, not a block.
        if let Some(expected) = node.mac {
            if expected != f.mac {
                r.push(
                    Rule::Structural,
                    Severity::Warning,
                    format!(
                        "node {} ({}) is {} on the bus, {} in the file — record the swap",
                        node.address, node.label, f.mac, expected
                    ),
                );
            }
        }
    }

    match find(m.margin.source_address) {
        Some(f) if !f.both_pulses_seen => r.error(
            Rule::MarginSource,
            format!(
                "margin source {} has not seen both start pulses, so its \
                 launch_margin_ticks is not meaningful (D20)",
                m.margin.source_address
            ),
        ),
        Some(_) => {}
        None => r.error(
            Rule::MarginSource,
            format!("margin source {} did not answer", m.margin.source_address),
        ),
    }

    let present: BTreeSet<Mac> = facts.iter().map(|f| f.mac).collect();
    for node in &m.nodes {
        if node.crystal_ppm.is_none() {
            continue;
        }
        match node.mac {
            None => r.error(
                Rule::CrystalMacPresent,
                format!(
                    "node {} ({}) carries crystal_ppm with no mac to key it to (D13)",
                    node.address, node.label
                ),
            ),
            Some(mac) if !present.contains(&mac) => r.error(
                Rule::CrystalMacPresent,
                format!(
                    "crystal_ppm for {mac} on node {} ({}), but no device on the bus \
                     reports that MAC",
                    node.address, node.label
                ),
            ),
            Some(_) => {}
        }
    }

    for c in m.temperature_corrections() {
        if !present.contains(&c.mac) {
            r.error(
                Rule::CrystalMacPresent,
                format!(
                    "temperature correction for {}, which is not on the bus",
                    c.mac
                ),
            );
        }
    }

    r
}

// ---------------------------------------------------------------------------

fn structural(m: &Mapping, r: &mut Report) {
    if m.venue.lanes < 1 || m.venue.lanes > 2 {
        r.error(
            Rule::Structural,
            format!(
                "venue.lanes is {}; the register map has exactly two lane records",
                m.venue.lanes
            ),
        );
    }
    if m.nodes.is_empty() {
        r.error(Rule::Structural, "no nodes declared");
    }

    let mut seen = BTreeSet::new();
    for node in &m.nodes {
        // protocol.md §1: addresses 1–63, and address 0 read from a switch is a
        // fault rather than a broadcast.
        if node.address < 1 || node.address > 63 {
            r.error(
                Rule::Structural,
                format!(
                    "node {} ({}) is outside the 1–63 DIP range",
                    node.address, node.label
                ),
            );
        }
        if !seen.insert(node.address) {
            r.error(
                Rule::Structural,
                format!("address {} is declared more than once", node.address),
            );
        }

        let mut inputs = BTreeSet::new();
        for input in &node.inputs {
            if input.index as usize >= RunRecord::INPUTS {
                r.error(
                    Rule::Structural,
                    format!(
                        "node {} ({}) maps input {}, but a run record holds {}",
                        node.address,
                        node.label,
                        input.index,
                        RunRecord::INPUTS
                    ),
                );
            }
            if !inputs.insert(input.index) {
                r.error(
                    Rule::Structural,
                    format!(
                        "node {} ({}) maps input {} twice",
                        node.address, node.label, input.index
                    ),
                );
            }
            match input.lane() {
                None => r.error(
                    Rule::Structural,
                    format!(
                        "node {} ({}) input {} is on lane {}; only 1 and 2 exist",
                        node.address, node.label, input.index, input.lane
                    ),
                ),
                Some(l) if l.number() > m.venue.lanes => r.error(
                    Rule::Structural,
                    format!(
                        "node {} ({}) input {} is on lane {}, but the venue declares {}",
                        node.address, node.label, input.index, input.lane, m.venue.lanes
                    ),
                ),
                Some(_) => {}
            }
        }
    }
}

fn no_duplicate_meaning(m: &Mapping, r: &mut Report) {
    for ((lane, beam), sites) in m.by_meaning() {
        if sites.len() > 1 {
            let where_ = sites
                .iter()
                .map(|s| format!("{}:{}", s.address, s.input))
                .collect::<Vec<_>>()
                .join(", ");
            r.error(
                Rule::NoDuplicateMeaning,
                format!(
                    "lane {lane} has {beam} mapped {} times ({where_})",
                    sites.len()
                ),
            );
        }
    }
}

fn minimum_system(m: &Mapping, r: &mut Report) {
    for lane in m.declared_lanes() {
        for beam in [Beam::Stage, Beam::Finish] {
            if m.site(lane, beam).is_none() {
                r.error(
                    Rule::MinimumSystem,
                    format!(
                        "lane {} has no {beam}; stage and finish are the minimum system",
                        lane.number()
                    ),
                );
            }
        }
    }
}

fn trap_rules(m: &Mapping, r: &mut Report) {
    let any_trap = m.sites().any(|s| s.beam.is_trap());
    if any_trap && m.geometry.trap_base.is_none() {
        r.error(
            Rule::TrapBase,
            "a trap beam is mapped but geometry.trap_base is absent, so no speed \
             can be computed",
        );
    }

    for lane in m.declared_lanes() {
        let entry = m.site(lane, Beam::TrapEntry);
        let exit = m.site(lane, Beam::TrapExit);
        match (entry, exit) {
            (Some(e), Some(x)) if e.address != x.address => r.error(
                Rule::TrapOneTimer,
                format!(
                    "lane {}: trap_entry is on node {} and trap_exit on node {}; the \
                     interval must close inside one timer (D20)",
                    lane.number(),
                    e.address,
                    x.address
                ),
            ),
            (Some(_), None) | (None, Some(_)) => r.error(
                Rule::TrapOneTimer,
                format!(
                    "lane {}: only one half of the trap is mapped",
                    lane.number()
                ),
            ),
            _ => {}
        }
    }
}

fn termination(m: &Mapping, r: &mut Report) {
    let n = m.nodes.iter().filter(|x| x.terminated).count();
    if n != 2 {
        r.error(
            Rule::Termination,
            format!("{n} node(s) flagged terminated; a linear bus has exactly two ends (D09)"),
        );
    }
}

fn guard_symmetry(m: &Mapping, r: &mut Report) {
    // §5's wording — "guard is mapped for every lane that has one" — is checkable
    // in one reading: if any lane guards, all declared lanes must. Its absence
    // changes how a stage break is interpreted, so lanes must not differ in it.
    let with: Vec<Lane> = m
        .declared_lanes()
        .filter(|l| m.site(*l, Beam::Guard).is_some())
        .collect();
    if with.is_empty() {
        return;
    }
    for lane in m.declared_lanes() {
        if !with.contains(&lane) {
            r.error(
                Rule::GuardSymmetry,
                format!(
                    "lane {} has no guard while another lane does; a break means \
                     something different in each lane then (architecture.md §2)",
                    lane.number()
                ),
            );
        }
    }
}
