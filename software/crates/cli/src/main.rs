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
use beam402_session::{Recorder, Replay};
use beam402_sim::{Scenario, Simulator};

/// Metadata tags in a session file. The mapping and the pairing ride in the
/// recording so that a session is self-contained: **D26**'s "here is the
/// session, replay it" is not an answer if it needs three other files.
const TAG_MAPPING: char = 'M';
const TAG_PAIRING: char = 'P';

mod host;
mod live;
mod meeting;
mod pages;
mod push;
mod round;
mod slip;
mod watch;

const USAGE: &str = "\
beam402 — race control (simulator only; no hardware exists yet)

USAGE:
    beam402 sim <scenario.toml> [OPTIONS]
    beam402 scope <scenario.toml> [OPTIONS] [-o page.html]
    beam402 scoreboard <scenario.toml> [OPTIONS] [-o board.html]
    beam402 replay <session.log>
    beam402 ladder <entries> --format pro|sportsman
    beam402 serve <scenario.toml> [OPTIONS] [-o 0.0.0.0:8402]
    beam402 event <sheet.toml> [--log <results.log>] [--draw <class>]
    beam402 sheet <entries.csv> --event <skeleton.toml> [-o sheet.toml]
    beam402 push <sheet.toml> --log <results.log> --to <url>
    beam402 host <directory> [-o 0.0.0.0:8403]

OPTIONS:
    --mapping <file>     venue mapping file (default: the built-in reference venue)
    --event <sheet>      run a meeting: classes, ladders and pairs off an entry sheet
    --log <file>         the meeting's result log; required by --event
    --draw <class>       close qualifying and draw that class's ladder
    --to <url>           where to push a day (D33); also serve's live push target
    --format <name>      heads-up | bracket | index (default: heads-up)
    --dial <l1>,<l2>     dial-ins in seconds, required by --format bracket
    --index <seconds>    class index, required by --format index
    --deep-staging       permit deep staging
    --bye <lane>         one car only, 1 or 2
    --record <file>      write the bus session, replayable with `beam402 replay`
    -o, --out <file>     where to write the page, or serve's listen address

`scope` runs the same round and writes one self-contained page — the strip, the
tree, live beam states, the bus tape and the event stream, all on one scrubbable
timeline. No server, no CDN: it opens from a file:// URL.

`scoreboard` draws what the spectator board showed, at the resolution a real
LED panel would have — a preview of the board and its fallback, not a second
design that will have to be reconciled with one.

`serve` runs a **live** round: the bus on its own thread, the operator page and
the board as clients. The operator takes control, then arms. Control is held by
one client at a time and expires if it stops asking, so a closed laptop frees
the event instead of stranding it.

With `--event` it runs a meeting instead of one pairing: the pair on deck comes
off the ladder with its dials, the operator records the result, and the ladder
advances. `--log` is required with it, because a day that is not being written
down cannot be resumed — and resuming is the whole reason the log exists.

`sheet` turns the registration desk's spreadsheet into an entry sheet, checked
against the classes a skeleton declares — every error names the row it came from,
because the person who can fix it is standing there (**D34**). Given a `.toml` it
validates and prints the entry list instead.

`push` carries a day to a receiver: the sheet once, then result lines from
wherever the receiver is. Idempotent and resumable, so pushing after every pair
and pushing the whole log that evening are the same operation (**D33**). `serve
--to <url>` does it on a timer while the event runs.

`host` is the receiving end — the same binary, run somewhere with an address. It
stores two files per event and derives everything with the same crate the tower
uses, which is what stops an online ladder from ever contradicting the one people
are racing off.

`replay` re-runs a recorded session through the real poller and the real race
logic and prints the slip again. Same session, same slip — or it says where the
two stopped agreeing.
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
    command: Command,
    path: String,
    mapping: Option<String>,
    event: Option<String>,
    log: Option<String>,
    draw: Option<String>,
    to: Option<String>,
    format: String,
    dial: Option<(f64, f64)>,
    index: Option<f64>,
    deep_staging: bool,
    bye: Option<u8>,
    record: Option<String>,
    out: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Sim,
    Replay,
    Scope,
    Scoreboard,
    Ladder,
    Serve,
    Event,
    Desk,
    Push,
    Host,
}

fn run() -> Result<String, String> {
    let args = parse(std::env::args().skip(1).collect())?;
    match args.command {
        Command::Sim => simulate(&args),
        Command::Replay => replay(&args),
        Command::Scope => scope(&args),
        Command::Scoreboard => scoreboard(&args),
        Command::Ladder => ladder(&args),
        Command::Serve => serve(&args),
        Command::Event => event(&args),
        Command::Desk => desk(&args),
        Command::Push => push_day(&args),
        Command::Host => {
            let addr = args.out.clone().unwrap_or_else(|| "0.0.0.0:8403".into());
            host::serve(std::path::Path::new(&args.path), &addr).map(|()| String::new())
        }
    }
}

/// The registration desk (**D34**): a spreadsheet in, an entry sheet out.
///
/// Given a `.toml` instead of a CSV it validates and prints the entry list, which
/// is the other half of the same job — what is about to be run, checkable by
/// somebody who does not read TOML.
fn desk(args: &Args) -> Result<String, String> {
    use beam402_event::{desk, Sheet};

    let text = read(&args.path)?;
    if args.path.ends_with(".toml") {
        let sheet = Sheet::parse(&text).map_err(|e| format!("{}: {e}", args.path))?;
        return Ok(desk::entry_list(&sheet));
    }

    let skeleton = args.event.as_ref().ok_or(
        "--event <skeleton.toml> is required: the classes are a rulebook the club keeps, \
         not a column of a spreadsheet somebody retypes every Saturday",
    )?;
    let out = desk::import(&read(skeleton)?, &text).map_err(|e| format!("{}: {e}", args.path))?;
    let sheet = Sheet::parse(&out).expect("import validates before it returns");

    match &args.out {
        Some(path) => {
            std::fs::write(path, &out).map_err(|e| format!("{path}: {e}"))?;
            Ok(format!(
                "{}\nwrote {} — {} entries in {} classes\n",
                desk::entry_list(&sheet),
                path,
                sheet.entries.len(),
                sheet.classes.len()
            ))
        }
        // No -o means the sheet itself, so this composes with a shell redirect.
        None => Ok(out),
    }
}

/// Carry a day to a receiver (**D33**).
fn push_day(args: &Args) -> Result<String, String> {
    let sheet_text = read(&args.path)?;
    let sheet =
        beam402_event::Sheet::parse(&sheet_text).map_err(|e| format!("{}: {e}", args.path))?;
    let slug = sheet.event.id.as_ref().ok_or(
        "this sheet has no [event] id — a day that is carried anywhere needs a name \
         there, and a slug like \"kaluga-2026-08-15\" is what ends up in the URL",
    )?;
    let url = args
        .to
        .as_ref()
        .ok_or("--to <url> is required: where the day is going")?;
    let log = match &args.log {
        Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        None => return Err("--log <file> is required: the day is the log".into()),
    };

    let r = push::push(url, slug, &sheet_text, &log)?;
    let mut out = String::new();
    use std::fmt::Write;
    if r.sheet_sent {
        let _ = writeln!(out, "sent the entry sheet");
    }
    let _ = writeln!(
        out,
        "{}/event/{slug}: {} line(s) there, {} added{}",
        url.trim_end_matches('/'),
        r.lines,
        r.added,
        if r.skipped > 0 {
            format!(", {} unparseable and mirrored as they arrived", r.skipped)
        } else {
            String::new()
        }
    );
    Ok(out)
}

/// The derived day, as JSON — for anyone building their own view of it.
pub fn event_json(day: &beam402_event::Progress, skipped: usize) -> String {
    use std::fmt::Write;
    let esc = live::escape;
    let mut classes = Vec::new();
    for name in day.class_names().map(str::to_string).collect::<Vec<_>>() {
        let mut s = format!("{{\"name\":\"{}\"", esc(&name));
        let _ = write!(s, ",\"entered\":{}", day.sheet().entries_in(&name).len());
        if let Some(field) = day.field(&name) {
            let seeds: Vec<String> = field
                .seeds()
                .map(|(seed, id)| {
                    format!("{{\"seed\":{seed},\"who\":\"{}\"}}", esc(&day.driver(id)))
                })
                .collect();
            let _ = write!(s, ",\"field\":[{}]", seeds.join(","));
        }
        match day.champion(&name) {
            Some(c) => {
                let _ = write!(s, ",\"champion\":{c}");
            }
            None => s.push_str(",\"champion\":null"),
        }
        if let Some(round) = day.round(&name) {
            let pairs: Vec<String> = round
                .pairs
                .iter()
                .map(|p| {
                    format!(
                        "{{\"position\":{},\"left\":{},\"right\":{},\"won\":{}}}",
                        p.position,
                        p.left,
                        p.right.map_or("null".into(), |r| r.to_string()),
                        round
                            .winner(p.position)
                            .map_or("null".into(), |w| w.to_string())
                    )
                })
                .collect();
            let _ = write!(
                s,
                ",\"round\":{{\"number\":{},\"name\":\"{}\",\"pairs\":[{}]}}",
                round.number,
                esc(&beam402_event::round_name(round.pairs.len(), round.number)),
                pairs.join(",")
            );
        }
        s.push('}');
        classes.push(s);
    }
    format!(
        "{{\"event\":{{\"name\":\"{}\",\"date\":\"{}\"}},\"skipped\":{skipped},\"classes\":[{}]}}",
        esc(&day.sheet().event.name),
        esc(&day.sheet().event.date),
        classes.join(",")
    )
}

/// Show a meeting: the classes, the fields, the round each is on, and what is
/// on deck.
///
/// State is **derived** from the result log rather than stored, so this is also
/// how a day is checked after the fact — the same argument **D26** makes about a
/// bus session.
fn event(args: &Args) -> Result<String, String> {
    use beam402_event::{Progress, Sheet};
    use std::fmt::Write;

    let sheet = Sheet::parse(&read(&args.path)?).map_err(|e| format!("{}: {e}", args.path))?;
    let log = match &args.log {
        // Missing is a day that has not started; unreadable is not.
        Some(path) => std::fs::read_to_string(path).unwrap_or_default(),
        None => String::new(),
    };
    let (mut day, skipped) = Progress::replay(sheet, &log);

    let mut out = String::new();

    // Drawing the ladder closes qualifying, so it is a deliberate act with a
    // class named rather than something that happens when a round is asked for.
    if let Some(class) = &args.draw {
        let path = args
            .log
            .as_ref()
            .ok_or("--draw needs --log: the draw is a line in the result log")?;
        let record = day.draw(class).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        use std::io::Write as _;
        writeln!(f, "{}", record.line()).map_err(|e| format!("{path}: {e}"))?;
        let _ = writeln!(out, "drew {class}: qualifying is closed\n");
    }
    let _ = writeln!(
        out,
        "{} — {}\n",
        day.sheet().event.name,
        day.sheet().event.date
    );
    if skipped > 0 {
        let _ = writeln!(
            out,
            "{skipped} unreadable line(s) in the log were skipped. One is a torn write\n             after a power cut; more than that is a file somebody has to look at.\n"
        );
    }

    for name in day.class_names().map(str::to_string).collect::<Vec<_>>() {
        let entries = day.sheet().entries_in(&name).len();
        let _ = writeln!(out, "{name}  ({entries} entered)");

        match day.field(&name) {
            None => {
                let _ = writeln!(out, "  qualifying — the ladder has not been drawn\n");
                continue;
            }
            Some(field) => {
                for (seed, id) in field.seeds() {
                    let _ = writeln!(out, "  {seed:>3}  {}", day.driver(id));
                }
            }
        }
        if let Some(champion) = day.champion(&name) {
            let id = day.field(&name).and_then(|f| f.entry(champion));
            let _ = writeln!(
                out,
                "\n  WINNER  {}",
                id.map(|i| day.driver(i)).unwrap_or_default()
            );
        } else if let Some(round) = day.round(&name) {
            let _ = writeln!(
                out,
                "\n  {}",
                beam402_event::round_name(round.pairs.len(), round.number)
            );
            for p in &round.pairs {
                let mark = if round.winner(p.position).is_some() {
                    " done"
                } else {
                    ""
                };
                match p.right {
                    Some(r) => {
                        let _ = writeln!(out, "    {:>3} v {}{mark}", p.left, r);
                    }
                    None => {
                        let _ = writeln!(out, "    {:>3}   bye{mark}", p.left);
                    }
                }
            }
        }
        let _ = writeln!(out);
    }

    match day.next_pair() {
        Some(deck) => {
            let _ = writeln!(out, "ON DECK  {} round {}", deck.class, deck.round);
            for (seed, id) in &deck.entries {
                let choice = if deck.lane_choice == Some(*seed) {
                    "  (lane choice)"
                } else {
                    ""
                };
                let _ = writeln!(out, "  {seed:>3}  {}{choice}", day.driver(*id));
            }
            if deck.is_bye() {
                let _ = writeln!(out, "  bye — it still has to be run to advance");
            }
        }
        None => {
            let _ = writeln!(out, "nothing on deck");
        }
    }
    Ok(out)
}

/// Run a round and serve it, the way **D30** and **D31** both say race control
/// works: everything a human looks at arrives over the LAN from the one process
/// that holds the numbers.
///
/// The round is simulated and complete before the first client connects. That is
/// this command's whole limitation and it is deliberate — serving a *live* round
/// needs the poll loop on its own thread beside the server, sharing state under
/// a control token, and that is the next piece rather than a thing to sketch
/// here.
/// Run a live round and serve it, the way **D30** and **D31** both say race
/// control works: the bus on its own thread, everybody else a client.
///
/// The **operator** arms it. The staging machine reaching `Ready` means the tree
/// *may* be armed; nothing leaves the master until somebody holding control says
/// so, which is the difference between this and `beam402 sim`.
fn serve(args: &Args) -> Result<String, String> {
    use beam402_http::{Method, Request, Response};
    use live::{Intent, Live, Runtime};

    let mapping = load_mapping(args)?;
    let scenario =
        Scenario::parse(&read(&args.path)?).map_err(|e| format!("{}: {e}", args.path))?;
    let tree_address = scenario.tree.address;
    let pairing = pairing(args)?;
    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }
    let sim = Simulator::new(&mapping, scenario).map_err(|e| e.to_string())?;
    let cfg = Config {
        deep_staging: args.deep_staging,
        ..Config::default()
    };

    // The meeting, if there is one. Loaded before the socket is bound: a sheet
    // that is wrong is worth failing on, and failing on it while a club is
    // already connected is worse than failing at the prompt.
    let meeting = match (&args.event, &args.log) {
        (Some(sheet), Some(log)) => Some(
            meeting::Meeting::open(&read(sheet)?, std::path::Path::new(log))
                .map_err(|e| format!("{sheet}: {e}"))?,
        ),
        (Some(_), None) => {
            return Err("--event needs --log: a day that is not being written down \
                        cannot be resumed, and resuming is what the log is for"
                .into())
        }
        (None, Some(_)) => return Err("--log is only meaningful with --event".into()),
        (None, None) => None,
    };

    let addr = args
        .out
        .clone()
        .unwrap_or_else(|| "0.0.0.0:8402".to_string());
    let venue = mapping.venue.name.clone();
    let operator = pages::operator(&venue);
    let board = pages::board(&venue);
    let shared = Live::new();

    // The bus thread, and it is the only thing that touches the bus — **D05**
    // allows exactly one master, and a mutex is not a master. It owns the
    // mapping for the life of the process, which is why that is leaked rather
    // than borrowed: threading a lifetime through a thread spawn to save a few
    // kilobytes that live until exit anyway is a poor trade.
    let owned: &'static Mapping = Box::leak(Box::new(mapping));
    let bus_side = std::sync::Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut runtime = Runtime::new(sim, owned, pairing, addresses, tree_address, cfg);
        if let Some(m) = meeting {
            runtime = runtime.with_meeting(m);
        }
        loop {
            runtime.step(&bus_side);
            live::pace();
        }
    });

    // Live upload (**D33**), and it is deliberately not wired into the bus thread.
    // The pusher needs nothing from the runtime — only the log file, which is
    // already on disk before anything else knows a result exists. So it cannot
    // delay a poll cycle, cannot lose a result if it fails, and retries by simply
    // running again. "After every pair" and "that evening" are the same call.
    if let (Some(url), Some(log)) = (args.to.clone(), args.log.clone()) {
        let sheet_path = args.event.clone().ok_or("--to needs --event")?;
        let sheet_text = read(&sheet_path)?;
        let slug = beam402_event::Sheet::parse(&sheet_text)
            .map_err(|e| format!("{sheet_path}: {e}"))?
            .event
            .id
            .ok_or("pushing needs an [event] id in the sheet")?;
        println!("beam402: pushing to {url}/event/{slug} every 20 s");
        std::thread::spawn(move || loop {
            let log_text = std::fs::read_to_string(&log).unwrap_or_default();
            match push::push(&url, &slug, &sheet_text, &log_text) {
                Ok(r) if r.added > 0 => println!("beam402: pushed {} line(s)", r.added),
                Ok(_) => {}
                Err(e) => eprintln!("beam402: push failed, will retry: {e}"),
            }
            std::thread::sleep(std::time::Duration::from_secs(20));
        });
    }

    println!("beam402: serving on http://{addr}");
    println!("  /            operator — take control, then arm");
    println!("  /board       spectator scoreboard, live");
    println!("  /api/state   everything both pages render from");

    let api = shared;
    beam402_http::serve(addr.as_str(), move |r: &Request| {
        let token = r.param("token").and_then(|t| t.parse::<u64>().ok());
        match (r.method, r.path.as_str()) {
            (Method::Get | Method::Head, "/") => Response::html(operator.clone()),
            (Method::Get | Method::Head, "/board") => Response::html(board.clone()),
            (Method::Get | Method::Head, "/api/state") => Response::json(api.state()),
            (Method::Get | Method::Head, "/api/health") => Response::json(r#"{"ok":true}"#),

            // Claiming and renewing are the same call, so a client that keeps
            // asking keeps control and one that stops asking loses it.
            (Method::Post, "/api/control") => match api.claim(token) {
                Some(t) => Response::json(format!("{{\"token\":{t}}}")),
                None => Response::json(r#"{"token":null,"why":"another client holds control"}"#),
            },
            (Method::Post, "/api/release") => {
                if let Some(t) = token {
                    api.release(t);
                }
                Response::json(r#"{"ok":true}"#)
            }
            (
                Method::Post,
                path @ ("/api/arm" | "/api/abort" | "/api/next" | "/api/record" | "/api/swap"),
            ) => {
                let intent = match path {
                    "/api/arm" => Intent::Arm,
                    "/api/abort" => Intent::Abort,
                    "/api/record" => Intent::Record,
                    "/api/swap" => Intent::Swap,
                    _ => Intent::Next,
                };
                match token.map(|t| api.intend(t, intent)) {
                    Some(Ok(())) => Response::json(r#"{"ok":true}"#),
                    // 409, not 403: the request is well formed and the caller is
                    // not forbidden — somebody else is holding the start.
                    Some(Err(why)) => Response::new(
                        409,
                        "application/json; charset=utf-8",
                        format!("{{\"ok\":false,\"why\":\"{why}\"}}"),
                    ),
                    None => Response::text(400, "no token\n"),
                }
            }

            (Method::Get | Method::Head, _) => Response::text(404, "no such thing\n"),
            _ => Response::text(405, "method not allowed\n"),
        }
    })
    .map_err(|e| format!("{addr}: {e}"))?;
    Ok(String::new())
}

/// Print a ladder, so it can be checked against a rulebook.
///
/// Sanctioning bodies publish their own and they are not all the same. The
/// crate says to check the table; this is what you check it with, and it needs
/// no entries, no event and no hardware — just a style and a number of cars.
fn ladder(args: &Args) -> Result<String, String> {
    use beam402_event::ladder::{first_round, next_round, Style};
    use std::fmt::Write;

    let entries: usize = args
        .path
        .parse()
        .map_err(|_| format!("{:?} is not a number of entries", args.path))?;
    let style = match args.format.as_str() {
        "pro" => Style::Pro,
        "sportsman" => Style::Sportsman,
        other => return Err(format!("unknown ladder style {other:?} — pro or sportsman")),
    };

    let mut out = String::new();
    let _ = writeln!(out, "{} ladder, {entries} entries\n", args.format);
    let mut pairs = first_round(&style, entries);
    let mut round = 1;
    while !pairs.is_empty() {
        let _ = writeln!(out, "{}", beam402_event::round_name(pairs.len(), round));
        for p in &pairs {
            match p.right {
                Some(r) => {
                    let _ = writeln!(out, "  {:>3} v {}", p.left, r);
                }
                // Named, not blank: a bye is a result and somebody has to run it.
                None => {
                    let _ = writeln!(out, "  {:>3}   bye", p.left);
                }
            }
        }
        let _ = writeln!(out);
        // Played out with the better qualifier always winning, which is the only
        // assumption that shows the *shape* of the ladder rather than a result.
        let winners: Vec<usize> = pairs
            .iter()
            .map(|p| match p.right {
                Some(r) => p.left.min(r),
                None => p.left,
            })
            .collect();
        pairs = next_round(&style, round, &winners);
        round += 1;
    }
    let _ = writeln!(out, "Shown with the better qualifier winning every round,");
    let _ = writeln!(out, "so this is the ladder's shape and not a prediction.");
    let _ = writeln!(out, "Check it against your rulebook.");
    Ok(out)
}

/// Run a round and draw what the spectator board showed while it happened.
///
/// The board changes at four moments and not otherwise, so the page holds four
/// frames rather than one per poll cycle. That is not a saving — it is the
/// design: a board that changed every hundred milliseconds would be unreadable
/// from the stands, and one that showed a running number would be showing a
/// number that is not final.
fn scoreboard(args: &Args) -> Result<String, String> {
    use beam402_scoreboard::{html, Board, Show};

    let mapping = load_mapping(args)?;
    let scenario =
        Scenario::parse(&read(&args.path)?).map_err(|e| format!("{}: {e}", args.path))?;
    let tree_address = scenario.tree.address;
    let pairing = pairing(args)?;
    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }

    let sim = Simulator::new(&mapping, scenario).map_err(|e| e.to_string())?;
    let (mut tap, seen) = watch::Tap::new(sim);
    let mut rec = watch::Recording::new(&mapping, seen, &addresses);
    let cfg = Config {
        deep_staging: args.deep_staging,
        ..Config::default()
    };
    let report = round::run_watched(
        &mapping,
        &mut tap,
        &addresses,
        tree_address,
        &pairing,
        cfg,
        &mut rec,
    )?;

    let board = Board::REFERENCE;
    let mut shots = Vec::new();
    let mut last = None;
    for f in &rec.frames {
        let show = match f.phase.as_str() {
            "idle" => Show::Idle,
            "staging" | "ready" | "armed" => Show::Staging,
            "complete" => Show::Result,
            _ => Show::Running,
        };
        if last == Some(show) {
            continue;
        }
        last = Some(show);
        shots.push(html::Shot {
            t_ms: f.t_ms,
            show: format!("{show:?}").to_lowercase(),
            frame: beam402_scoreboard::render(
                board,
                show,
                &mapping.venue.name,
                &report.round,
                &pairing,
            ),
        });
    }

    let out = args.out.clone().unwrap_or_else(|| "board.html".to_string());
    std::fs::write(
        &out,
        html::page(
            board,
            &mapping.venue.name,
            &format!("{} · {} poll cycles", args.path, report.cycles),
            &shots,
        ),
    )
    .map_err(|e| format!("{out}: {e}"))?;
    Ok(format!("wrote {out}\n"))
}

/// Run a scenario and write one page you can look at.
///
/// It goes through the same loop `sim` does, with an observer attached that the
/// loop cannot detect — a round that ran differently when watched would not be
/// the round the page is of.
fn scope(args: &Args) -> Result<String, String> {
    let mapping = load_mapping(args)?;
    let scenario =
        Scenario::parse(&read(&args.path)?).map_err(|e| format!("{}: {e}", args.path))?;
    let tree_address = scenario.tree.address;
    let pairing = pairing(args)?;
    let handicap = pairing.handicap_ms().map_err(|e| e.to_string())?;
    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }

    let sim = Simulator::new(&mapping, scenario).map_err(|e| e.to_string())?;
    let (mut tap, seen) = watch::Tap::new(sim);
    let mut rec = watch::Recording::new(&mapping, seen, &addresses);
    let cfg = Config {
        deep_staging: args.deep_staging,
        ..Config::default()
    };
    let report = round::run_watched(
        &mapping,
        &mut tap,
        &addresses,
        tree_address,
        &pairing,
        cfg,
        &mut rec,
    )?;

    let slip = slip::render(&report.round, &pairing, report.blocked, report.abandoned);
    let capture = watch::capture(
        &mapping,
        &report.round,
        rec.frames,
        rec.finish_seen_ms,
        rec.tree.as_ref(),
        slip,
        args.format.clone(),
        args.dial,
        handicap,
        format!("{} · {} poll cycles", args.path, report.cycles),
    );

    let out = args.out.clone().unwrap_or_else(|| "scope.html".to_string());
    std::fs::write(&out, beam402_scope::page(&capture)).map_err(|e| format!("{out}: {e}"))?;
    Ok(format!("wrote {out}\n"))
}

fn load_mapping(args: &Args) -> Result<Mapping, String> {
    let mapping = match &args.mapping {
        Some(path) => Mapping::parse(&read(path)?).map_err(|e| format!("{path}: {e}"))?,
        None => beam402_sim::reference::venue(),
    };
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
    Ok(mapping)
}

/// Re-run a recording. Everything above the bus is the same code that produced
/// it, so the slip is either identical or the divergence is named.
fn replay(args: &Args) -> Result<String, String> {
    // The pairing comes out of the recording, so a flag that would change it is
    // refused rather than ignored: a replay whose dial-ins came from the command
    // line is not a replay of anything.
    if args.dial.is_some() || args.index.is_some() || args.bye.is_some() || args.mapping.is_some() {
        return Err("replay takes its mapping and pairing from the recording".into());
    }
    let text = read(&args.path)?;
    let mut session = Replay::parse(&text).map_err(|e| format!("{}: {e}", args.path))?;

    let mapping = Mapping::parse(&session.meta(TAG_MAPPING))
        .map_err(|e| format!("{}: the recorded mapping does not parse: {e}", args.path))?;
    let describe = session.meta(TAG_PAIRING);
    let pairing = pairing_from(&describe)?;
    let tree_address = tree_from(&describe)?;
    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }

    let report = round::run(
        &mapping,
        &mut session,
        &addresses,
        tree_address,
        &pairing,
        Config::default(),
    )?;
    let mut out = slip::render(&report.round, &pairing, report.blocked, report.abandoned);
    match session.divergence() {
        // Not a warning to bury under the slip. A replay that diverged is a
        // statement about the *code*, not about the race: the numbers above were
        // produced by a different program than the one that recorded them.
        Some(why) => out.push_str(&format!(
            "\nDIVERGED — this build does not reproduce the recording:\n  {why}\n"
        )),
        None => out.push_str("\nreplayed clean: every transaction matched the recording\n"),
    }
    Ok(out)
}

fn simulate(args: &Args) -> Result<String, String> {
    // A mapping that does not describe a startable system is refused here rather
    // than producing a round with holes in it.
    let mapping = load_mapping(args)?;

    let scenario =
        Scenario::parse(&read(&args.path)?).map_err(|e| format!("{}: {e}", args.path))?;
    let tree_address = scenario.tree.address;
    let pairing = pairing(args)?;

    let mut addresses: Vec<u8> = mapping.nodes.iter().map(|n| n.address).collect();
    if !addresses.contains(&tree_address) {
        addresses.push(tree_address);
    }

    let sim = Simulator::new(&mapping, scenario).map_err(|e| e.to_string())?;
    let cfg = Config {
        deep_staging: args.deep_staging,
        ..Config::default()
    };

    let report = match &args.record {
        Some(path) => {
            let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
            let mut rec = Recorder::new(sim, std::io::BufWriter::new(file))
                .map_err(|e| format!("{path}: {e}"))?;
            // The mapping and the pairing go in first, so the file answers "what
            // was this a race between" with nothing else on the machine.
            let raw = match &args.mapping {
                Some(p) => read(p)?,
                None => beam402_sim::reference::VENUE.to_string(),
            };
            rec.meta(TAG_MAPPING, &raw)
                .and_then(|()| rec.meta(TAG_PAIRING, &describe(args, tree_address)))
                .map_err(|e| format!("{path}: {e}"))?;
            round::run(&mapping, &mut rec, &addresses, tree_address, &pairing, cfg)?
        }
        None => {
            let mut sim = sim;
            round::run(&mapping, &mut sim, &addresses, tree_address, &pairing, cfg)?
        }
    };

    let mut out = slip::render(&report.round, &pairing, report.blocked, report.abandoned);
    out.push_str(&format!(
        "\n{} poll cycles, {:.1} s of bus time at 19,200 bps\n",
        report.cycles,
        report.bus_ms / 1000.0
    ));
    Ok(out)
}

/// The pairing as a session writes it down: one `key = value` per line, so a
/// human reading the file sees what the race was and a replay can rebuild it.
fn describe(args: &Args, tree: u8) -> String {
    let mut out = format!("format = {}\ntree = {tree}\n", args.format);
    if let Some((a, b)) = args.dial {
        out.push_str(&format!("dial = {a},{b}\n"));
    }
    if let Some(i) = args.index {
        out.push_str(&format!("index = {i}\n"));
    }
    if let Some(b) = args.bye {
        out.push_str(&format!("bye = {b}\n"));
    }
    out
}

fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim())
}

fn tree_from(text: &str) -> Result<u8, String> {
    field(text, "tree")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| "the recording does not say which address the tree was".to_string())
}

/// Rebuild the pairing a session was recorded with. It goes through the same
/// [`Pairing::new`] as a live round, so a recording carrying an impossible
/// handicap is refused here rather than replayed into a wrong slip.
fn pairing_from(text: &str) -> Result<Pairing, String> {
    let args = Args {
        command: Command::Replay,
        path: String::new(),
        mapping: None,
        event: None,
        log: None,
        draw: None,
        to: None,
        format: field(text, "format").unwrap_or("heads-up").to_string(),
        dial: match field(text, "dial") {
            Some(v) => {
                let (a, b) = v.split_once(',').ok_or("malformed dial in the recording")?;
                Some((number(a)?, number(b)?))
            }
            None => None,
        },
        index: match field(text, "index") {
            Some(v) => Some(number(v)?),
            None => None,
        },
        deep_staging: false,
        bye: match field(text, "bye") {
            Some(v) => Some(number(v)? as u8),
            None => None,
        },
        record: None,
        out: None,
    };
    pairing(&args)
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
    let command = match it.next().as_deref() {
        Some("sim") => Command::Sim,
        Some("replay") => Command::Replay,
        Some("scope") => Command::Scope,
        Some("scoreboard") => Command::Scoreboard,
        Some("ladder") => Command::Ladder,
        Some("serve") => Command::Serve,
        Some("event") => Command::Event,
        Some("sheet") => Command::Desk,
        Some("push") => Command::Push,
        Some("host") => Command::Host,
        Some("-h") | Some("--help") | None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("unknown command {other:?}\n\n{USAGE}")),
    };
    let path = it
        .next()
        .ok_or_else(|| format!("no file given\n\n{USAGE}"))?;
    let mut args = Args {
        command,
        path,
        mapping: None,
        event: None,
        log: None,
        draw: None,
        to: None,
        format: "heads-up".into(),
        dial: None,
        index: None,
        deep_staging: false,
        bye: None,
        record: None,
        out: None,
    };
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .ok_or_else(|| format!("{flag} needs a value\n\n{USAGE}"))
        };
        match flag.as_str() {
            "--mapping" => args.mapping = Some(value()?),
            "--event" => args.event = Some(value()?),
            "--log" => args.log = Some(value()?),
            "--draw" => args.draw = Some(value()?),
            "--to" => args.to = Some(value()?),
            "--record" => args.record = Some(value()?),
            "-o" | "--out" => args.out = Some(value()?),
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

    /// Record a round in memory and replay it, returning both slips.
    fn recorded_and_replayed() -> (String, String) {
        let mapping = venue();
        let sim = Simulator::new(&mapping, Scenario::parse(BRACKET).unwrap()).unwrap();
        let pairing = Pairing::new(
            Format::Bracket,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: Some(12.34),
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: Some(7.50),
                },
            ],
        )
        .unwrap();

        let mut buf = Vec::new();
        let live = {
            let mut rec = Recorder::new(sim, &mut buf).unwrap();
            rec.meta(TAG_MAPPING, beam402_sim::reference::VENUE)
                .unwrap();
            rec.meta(
                TAG_PAIRING,
                "format = bracket\ntree = 10\ndial = 12.34,7.50\n",
            )
            .unwrap();
            let report = round::run(
                &mapping,
                &mut rec,
                &ADDRESSES,
                TREE,
                &pairing,
                Config::default(),
            )
            .expect("the recorded round must complete");
            slip::render(&report.round, &pairing, report.blocked, report.abandoned)
        };

        let text = String::from_utf8(buf).unwrap();
        let mut session = Replay::parse(&text).unwrap();
        let replayed_mapping = Mapping::parse(&session.meta(TAG_MAPPING)).unwrap();
        let report = round::run(
            &replayed_mapping,
            &mut session,
            &ADDRESSES,
            TREE,
            &pairing,
            Config::default(),
        )
        .expect("the replayed round must complete");
        assert_eq!(session.divergence(), None, "the replay must not diverge");
        let again = slip::render(&report.round, &pairing, report.blocked, report.abandoned);
        (live, again)
    }

    #[test]
    fn a_recorded_session_replays_to_the_same_slip() {
        // **D26**'s claim, as an assertion rather than a sentence: here is the
        // session, replay it, get the same ET. Not a summary compared against
        // itself — the replay drives the real poller, the real staging machine
        // and the real race logic through the same seam.
        let (live, again) = recorded_and_replayed();
        assert_eq!(live, again);
        assert!(live.contains("WIN  lane 1 — first to the finish by 0.0400 s"));
    }

    #[test]
    fn the_recording_carries_the_venue_it_was_run_on() {
        // A session that needs three other files to mean anything is not
        // evidence about a disputed round.
        let mapping = venue();
        let sim = Simulator::new(&mapping, Scenario::parse(BRACKET).unwrap()).unwrap();
        let mut buf = Vec::new();
        {
            let mut rec = Recorder::new(sim, &mut buf).unwrap();
            rec.meta(TAG_MAPPING, beam402_sim::reference::VENUE)
                .unwrap();
        }
        let text = String::from_utf8(buf).unwrap();
        let session = Replay::parse(&text).unwrap();
        let back = Mapping::parse(&session.meta(TAG_MAPPING)).unwrap();
        assert_eq!(back.venue.name, "Sim Strip");
        assert_eq!(back.nodes.len(), mapping.nodes.len());
    }

    /// Run the bracket round with an observer attached, as `scope` does.
    fn watched() -> beam402_scope::Capture {
        let mapping = venue();
        let sim = Simulator::new(&mapping, Scenario::parse(BRACKET).unwrap()).unwrap();
        let pairing = Pairing::new(
            Format::Bracket,
            vec![
                Entry {
                    lane: Lane::L1,
                    dial_s: Some(12.34),
                },
                Entry {
                    lane: Lane::L2,
                    dial_s: Some(7.50),
                },
            ],
        )
        .unwrap();
        let (mut tap, seen) = watch::Tap::new(sim);
        let mut rec = watch::Recording::new(&mapping, seen, &ADDRESSES);
        let report = round::run_watched(
            &mapping,
            &mut tap,
            &ADDRESSES,
            TREE,
            &pairing,
            Config::default(),
            &mut rec,
        )
        .expect("the round must complete");
        watch::capture(
            &mapping,
            &report.round,
            rec.frames,
            rec.finish_seen_ms,
            rec.tree.as_ref(),
            String::new(),
            "bracket".into(),
            Some((12.34, 7.50)),
            pairing.handicap_ms().unwrap(),
            String::new(),
        )
    }

    #[test]
    fn no_car_leaves_before_its_own_green() {
        // The picture that was wrong. The master is silent across the launch, so
        // the cascade was drawn as unknown — and two cars left a tree that never
        // went green, which reads as a monumental false start. A page asserting a
        // foul the record denies is worse than one that draws less.
        let c = watched();
        for lane in [1u8, 2] {
            let green = c
                .lamp_at
                .iter()
                .find(|l| l.lane == lane && l.lamp == 5)
                .expect("every lane that ran has a green")
                .t_ms;
            let launch = c.launch_ms[lane as usize - 1].expect("and a launch");
            assert!(
                launch >= green,
                "lane {lane} left {} ms before its green",
                green - launch
            );
        }
    }

    #[test]
    fn the_pictures_own_numbers_agree_with_the_slip() {
        // Only one instant in the drawing is approximate — where the round sits
        // on the loop's clock. Every interval inside it is a register, and this
        // is what says so: reaction times to the millisecond, and the two greens
        // exactly the handicap apart.
        let c = watched();
        let green = |lane: u8| {
            c.lamp_at
                .iter()
                .find(|l| l.lane == lane && l.lamp == 5)
                .unwrap()
                .t_ms as i64
        };
        assert_eq!(
            green(2) - green(1),
            c.handicap_ms[1] as i64,
            "the greens are the handicap apart, not a poll cycle plus it"
        );
        for (lane, rt) in [(1u8, 500i64), (2, 540)] {
            let shown = c.launch_ms[lane as usize - 1].unwrap() as i64 - green(lane);
            assert_eq!(shown, rt, "lane {lane} shows the reaction the slip prints");
        }
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
