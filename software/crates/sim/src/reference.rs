//! One venue and one clean pair, shared by everything that needs a bus to talk
//! to.
//!
//! It lives in the crate rather than in a test module because the simulator's
//! tests are not its only customer: the poller's tests, and the CLI's demo, need
//! the *same* bus, or three descriptions of a track drift apart and stop being
//! evidence about the same thing.
//!
//! The numbers below are **ground truth**. Every assertion anywhere compares what
//! a master can read on the bus against these, never against the simulator's
//! internals.

use beam402_mapping::Mapping;

use crate::scenario::Scenario;

/// Six timing nodes and a tree at 10. Start is split across two addresses so that
/// the two-lane capture model (**D20**) is exercised rather than assumed, and the
/// trap sits on its own node per `architecture.md` §12.
pub const VENUE: &str = r#"
[venue]
name = "Sim Strip"
lanes = 2

[geometry]
sixty_foot = 18.288
finish = 402.336
trap_base = 20.115

[margin]
source_address = 1

[[node]]
address = 1
label = "start-lane1"
terminated = true
[[node.input]]
index = 0
beam = "prestage"
lane = 1
[[node.input]]
index = 1
beam = "stage"
lane = 1
[[node.input]]
index = 2
beam = "guard"
lane = 1

[[node]]
address = 2
label = "start-lane2"
[[node.input]]
index = 0
beam = "prestage"
lane = 2
[[node.input]]
index = 1
beam = "stage"
lane = 2
[[node.input]]
index = 2
beam = "guard"
lane = 2

[[node]]
address = 3
label = "60ft"
[[node.input]]
index = 0
beam = "interval_60"
lane = 1
[[node.input]]
index = 1
beam = "interval_60"
lane = 2

[[node]]
address = 4
label = "trap"
[[node.input]]
index = 0
beam = "trap_entry"
lane = 1
[[node.input]]
index = 1
beam = "trap_exit"
lane = 1
[[node.input]]
index = 2
beam = "trap_entry"
lane = 2
[[node.input]]
index = 3
beam = "trap_exit"
lane = 2

[[node]]
address = 6
label = "finish"
terminated = true
[[node.input]]
index = 0
beam = "finish"
lane = 1
[[node.input]]
index = 1
beam = "finish"
lane = 2
"#;

pub const START_L1: u8 = 1;
pub const START_L2: u8 = 2;
pub const SIXTY: u8 = 3;
pub const TRAP: u8 = 4;
pub const FINISH: u8 = 6;
pub const TREE: u8 = 10;

/// Every address that answers on this bus, tree included. The tree is not in the
/// mapping file — it has no beams — so a master assembles its poll list from
/// both, and this is that list.
pub const ADDRESSES: [u8; 6] = [START_L1, START_L2, SIXTY, TRAP, FINISH, TREE];

pub const TRAP_BASE_M: f64 = 20.115;

pub const R1: f64 = 0.520;
pub const R2: f64 = 0.489;
pub const ET1: f64 = 10.412;
pub const ET2: f64 = 10.388;
pub const SIXTY1: f64 = 1.632;
pub const SIXTY2: f64 = 1.601;
pub const ENTRY1: f64 = 9.980;
pub const EXIT1: f64 = 10.310;

pub fn venue() -> Mapping {
    Mapping::parse(VENUE).expect("the reference venue must parse")
}

/// Two cars, both green, nothing broken.
pub fn clean_pair() -> String {
    format!(
        r#"
[scenario]
name = "clean pair"
seed = 42

[tree]
address = 10
mode = "standard"
random_delay_ms = 700
arm_at_s = 3.0

[[car]]
lane = 1
stage_at_s = 1.0
reaction_s = {R1}
[car.splits]
interval_60 = {SIXTY1}
trap_entry = {ENTRY1}
trap_exit = {EXIT1}
finish = {ET1}

[[car]]
lane = 2
stage_at_s = 1.2
reaction_s = {R2}
[car.splits]
interval_60 = {SIXTY2}
finish = {ET2}
"#
    )
}

pub fn scenario(text: &str) -> Scenario {
    Scenario::parse(text).expect("scenario must parse")
}
