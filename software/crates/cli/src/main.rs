//! `beam402` — race control, such as it is.
//!
//! One subcommand today: run a scenario against the node simulator and print the
//! time slip. That is not a toy. It is the whole path — poller, staging machine,
//! run assembly, outcome — exercised over the same [`Bus`](beam402_bus::Bus)
//! seam a serial port will sit behind, so what changes when hardware arrives is
//! the transport and nothing above it.
//!
//! **Nothing here has run against hardware.** Every number it prints came from a
//! scenario file that stated it.

use std::process::ExitCode;

use beam402_mapping::Mapping;
use beam402_protocol::Lane;
use beam402_race::staging::Config;
use beam402_race::{Entry, Format, Pairing};
use beam402_sim::{Scenario, Simulator};

mod round;
mod slip;

const USAGE: &str = "\
beam402 — race control (simulator only; no hardware exists yet)

USAGE:
    beam402 sim <scenario.toml> [OPTIONS]

OPTIONS:
    --mapping <file>     venue mapping file (default: the built-in reference venue)
    --format <name>      heads-up | bracket | index (default: heads-up)
    --dial <l1>,<l2>     dial-ins in seconds, required by --format bracket
    --index <seconds>    class index, required by --format index
    --deep-staging       permit deep staging
    --bye <lane>         one car only, 1 or 2
";

fn main() -> ExitCode {
    match run() {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("beam402: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    scenario: String,
    mapping: Option<String>,
    format: String,
    dial: Option<(f64, f64)>,
    index: Option<f64>,
    deep_staging: bool,
    bye: Option<u8>,
}

fn run() -> Result<String, String> {
    let args = parse(std::env::args().skip(1).collect())?;

    let mapping = match &args.mapping {
        Some(path) => Mapping::parse(&read(path)?).map_err(|e| format!("{path}: {e}"))?,
        None => beam402_sim::reference::venue(),
    };
    // A mapping that does not describe a startable system is refused here rather
    // than producing a round with holes in it.
    let report = beam402_mapping::check_static(&mapping);
    if !report.may_start_a_round() {
        return Err(format!(
            "the mapping cannot start a round:\n{}",
            report
                .errors()
                .map(|f| format!("  {f}\n"))
                .collect::<String>()
        ));
    }

    let scenario =
        Scenario::parse(&read(&args.scenario)?).map_err(|e| format!("{}: {e}", args.scenario))?;
    let tree_address = scenario.tree.address;
    let pairing = pairing(&args)?;

    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }

    let mut sim = Simulator::new(&mapping, scenario).map_err(|e| e.to_string())?;
    let cfg = Config {
        deep_staging: args.deep_staging,
        ..Config::default()
    };
    let report = round::run(&mapping, &mut sim, &addresses, tree_address, &pairing, cfg)?;

    let mut out = slip::render(&report.round, &pairing, report.blocked, report.abandoned);
    out.push_str(&format!(
        "\n{} poll cycles, {:.1} s of bus time at 19,200 bps\n",
        report.cycles,
        report.bus_ms / 1000.0
    ));
    Ok(out)
}

fn pairing(args: &Args) -> Result<Pairing, String> {
    let format = match args.format.as_str() {
        "heads-up" => Format::HeadsUp,
        "bracket" => Format::Bracket,
        "index" => Format::Index {
            seconds: args.index.ok_or("--format index needs --index <seconds>")?,
        },
        other => return Err(format!("unknown format {other:?}")),
    };

    let dials = args.dial.unwrap_or((0.0, 0.0));
    let entry = |lane: Lane, dial: f64| Entry {
        lane,
        dial_s: (args.dial.is_some()).then_some(dial),
    };
    let entries = match args.bye {
        Some(1) => vec![entry(Lane::L1, dials.0)],
        Some(2) => vec![entry(Lane::L2, dials.1)],
        Some(n) => return Err(format!("--bye takes 1 or 2, not {n}")),
        None => vec![entry(Lane::L1, dials.0), entry(Lane::L2, dials.1)],
    };
    Pairing::new(format, entries).map_err(|e| e.to_string())
}

fn parse(argv: Vec<String>) -> Result<Args, String> {
    let mut it = argv.into_iter();
    match it.next().as_deref() {
        Some("sim") => {}
        Some("-h") | Some("--help") | None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
    let scenario = it.next().ok_or_else(|| format!("no scenario\n\n{USAGE}"))?;
    let mut args = Args {
        scenario,
        mapping: None,
        format: "heads-up".into(),
        dial: None,
        index: None,
        deep_staging: false,
        bye: None,
    };
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"))
        };
        match flag.as_str() {
            "--mapping" => args.mapping = Some(value()?),
            "--format" => args.format = value()?,
            "--index" => args.index = Some(number(&value()?)?),
            "--deep-staging" => args.deep_staging = true,
            "--bye" => args.bye = Some(number(&value()?)? as u8),
            "--dial" => {
                let raw = value()?;
                let (a, b) = raw
                    .split_once(',')
                    .ok_or("--dial takes two seconds separated by a comma")?;
                args.dial = Some((number(a)?, number(b)?));
            }
            other => return Err(format!("unknown option {other:?}\n\n{USAGE}")),
        }
    }
    Ok(args)
}

fn number(s: &str) -> Result<f64, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("{s:?} is not a number"))
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use beam402_race::staging::Config;
    use beam402_sim::reference::{venue, ADDRESSES, TREE};

    /// The case bracket racing exists for, priced in full. Every number here is
    /// stated by the scenario and recovered off the bus; nothing is read out of
    /// the simulator's internals.
    const BRACKET: &str = r#"
[scenario]
name = "12.34 vs 7.50"
seed = 7
[tree]
address = 10
mode = "standard"
random_delay_ms = 400
arm_at_s = 600.0
[[car]]
lane = 1
stage_at_s = 2.0
reaction_s = 0.500
[car.splits]
interval_60 = 2.104
finish = 12.340
[[car]]
lane = 2
stage_at_s = 2.2
reaction_s = 0.540
[car.splits]
interval_60 = 0.951
trap_entry = 7.120
trap_exit = 7.310
finish = 7.500
"#;

    fn slip_for(format: Format, dials: Option<(f64, f64)>) -> String {
        let mapping = venue();
        let mut sim = Simulator::new(&mapping, Scenario::parse(BRACKET).unwrap()).unwrap();
        let entry = |lane: Lane, d: f64| Entry {
            lane,
            dial_s: dials.map(|_| d),
        };
        let dials = dials.unwrap_or((0.0, 0.0));
        let pairing = Pairing::new(
            format,
            vec![entry(Lane::L1, dials.0), entry(Lane::L2, dials.1)],
        )
        .unwrap();
        let report = round::run(
            &mapping,
            &mut sim,
            &ADDRESSES,
            TREE,
            &pairing,
            Config::default(),
        )
        .expect("the round must complete");
        slip::render(&report.round, &pairing, report.blocked, report.abandoned)
    }

    #[test]
    fn a_bracket_slip_names_the_winner_and_the_reason() {
        let slip = slip_for(Format::Bracket, Some((12.34, 7.50)));
        assert!(
            slip.contains("WIN  lane 1 — first to the finish by 0.0400 s"),
            "{slip}"
        );
        // Both drivers on their dial, so the round is the 0.040 s between how
        // they left — and the loser's ET is 4.8 seconds quicker.
        assert!(slip.contains("12.3400"), "{slip}");
        assert!(slip.contains("7.5000"), "{slip}");
        assert!(slip.contains("381.13 km/h"), "{slip}");
    }

    #[test]
    fn the_same_cars_heads_up_produce_the_other_winner() {
        // Nothing about the cars changed. Without the spot, the quicker car
        // simply wins — which is the sentence the whole format exists to stop
        // being the only possible one.
        let slip = slip_for(Format::HeadsUp, None);
        assert!(slip.contains("WIN  lane 2"), "{slip}");
        assert!(!slip.contains("dial            12"), "no dial to print");
    }

    #[test]
    fn a_breakout_loses_the_round_and_says_so_on_the_slip() {
        let slip = slip_for(Format::Bracket, Some((12.34, 7.55)));
        assert!(slip.contains("lane 2: broke out by 0.050 s"), "{slip}");
        assert!(slip.contains("WIN  lane 1 — opponent broke out"), "{slip}");
    }

    #[test]
    fn an_unmeasured_split_is_an_em_dash_with_a_reason() {
        // The rule the slip exists to keep: never a zero, never a blank, and
        // never a number nobody measured.
        let slip = slip_for(Format::Bracket, Some((12.34, 7.50)));
        assert!(slip.contains("—"), "{slip}");
        assert!(
            slip.contains("lane 1 trap_entry: — (beam not broken, node 4 input 0)"),
            "{slip}"
        );
        // And the stage beam is never listed: the tire leaving it is the zero of
        // the run it starts, so its time cannot be inside that run (D16).
        assert!(!slip.contains("stage:"), "{slip}");
    }
}
