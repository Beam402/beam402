//! Holds the documents to the register map (**D27**).
//!
//! `protocol.md` §0 wants exactly one source for the map. With the map living in
//! `beam402-protocol`, that rule keeps working with the arrow reversed — but it
//! lands differently on the two documents, and the difference is deliberate:
//!
//! ```text
//! render-map toml                  # print docs/registers.toml — generated in full
//! render-map check <path>          # ...and regenerating it changes nothing (CI)
//! render-map check-tables <path>   # protocol.md §3: every address, width and
//!                                  # flag bit agrees with the map (CI)
//! render-map tables                # what §3's tables would look like, printed
//! ```
//!
//! §3 is checked rather than replaced because its paragraphs explain *why* a
//! register reads the way it does, with cross-references no doc comment would
//! carry as well. Numbers drift; prose of that kind does not drift the same way.
//!
//! The same walk emits a C header if **D22** does not reverse and the node stays
//! on C. That is the whole cost of the fallback, and it is why it stays cheap.

use std::fmt::Write as _;
use std::process::ExitCode;

use beam402_protocol::blocks::{Access, Poll};
use beam402_protocol::map::{
    BlockDesc, GroupDesc, RegDesc, BEAM_MEANINGS, CONVENTIONS, FLAG_WORDS, LINK, OPCODES,
    PROTOCOL_VERSION, REGISTER_MAP,
};

fn poll_name(p: Poll) -> &'static str {
    match p {
        Poll::EveryCycle => "every-cycle",
        Poll::Once => "once",
        Poll::OnGenerationChange => "on-generation-change",
        Poll::RoundRobin => "round-robin",
        Poll::OnFaultOrSlowRotation => "on-fault-or-slow-rotation",
        Poll::OnRequest => "on-request",
        Poll::Write => "write",
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("toml") => {
            print!("{}", render_toml());
            ExitCode::SUCCESS
        }
        Some("tables") => {
            print!("{}", render_tables());
            ExitCode::SUCCESS
        }
        Some("check") => match args.get(1) {
            Some(path) => check(path),
            None => usage(),
        },
        Some("check-tables") => match args.get(1) {
            Some(path) => check_tables(path),
            None => usage(),
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: render-map <toml|tables|check PATH|check-tables PATH>");
    ExitCode::FAILURE
}

fn check(path: &str) -> ExitCode {
    let generated = render_toml();
    let on_disk = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    if generated == on_disk {
        println!("{path} is up to date");
        return ExitCode::SUCCESS;
    }
    eprintln!("{path} differs from the map in beam402-protocol.\n");
    let mut shown = 0;
    for (i, (a, b)) in generated.lines().zip(on_disk.lines()).enumerate() {
        if a != b {
            eprintln!("  line {}:\n    generated: {a}\n    on disk:   {b}", i + 1);
            shown += 1;
            if shown == 10 {
                eprintln!("  ...");
                break;
            }
        }
    }
    let (g, d) = (generated.lines().count(), on_disk.lines().count());
    if g != d {
        eprintln!("  generated {g} lines, on disk {d}");
    }
    eprintln!("\nRun `render-map toml > {path}` if the map is what changed.");
    ExitCode::FAILURE
}

// ---------------------------------------------------------------------------
// Checking protocol.md §3 in place
// ---------------------------------------------------------------------------

/// Verify the register tables in a prose document against the map.
///
/// §3 is not regenerated wholesale, and deliberately: its paragraphs explain
/// *why* a register reads the way it does, with cross-references no doc comment
/// would carry as well. What drifts is the numbers, so the numbers are what gets
/// checked — every address, width and flag bit in the document must exist in the
/// crate, saying the same thing.
///
/// The verified-row count is printed because a table parser that silently
/// matches nothing is worse than no check at all: it reports success.
fn check_tables(path: &str) -> ExitCode {
    let doc = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut problems: Vec<String> = Vec::new();
    let mut block: Option<&BlockDesc> = None;
    let mut flags: Option<&'static beam402_protocol::map::FlagWordDesc> = None;
    let (mut regs_checked, mut bits_checked) = (0usize, 0usize);

    for (n, raw) in doc.lines().enumerate() {
        let line = raw.trim();
        let no = n + 1;

        if let Some(rest) = line.strip_prefix("### ") {
            block = REGISTER_MAP
                .iter()
                .find(|b| rest.starts_with(&addr_list(b)));
            flags = None;
            continue;
        }
        // A line of the form `status_flags`: introduces that word's bit table.
        if let Some(name) = line.strip_suffix("`:").and_then(|s| s.strip_prefix('`')) {
            flags = FLAG_WORDS.iter().find(|w| w.name == name);
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();
        if cells.len() < 2 || cells[0].starts_with("---") {
            continue;
        }
        // §3 introduces a bit table either as "`run_flags`:" above it or as its
        // own header row, "| Bit | `run_flags` |". Both appear in the document.
        if cells[0] == "Bit" {
            if let Some(name) = backticked(cells[1]).first() {
                flags = FLAG_WORDS.iter().find(|w| w.name == *name);
            }
            continue;
        }

        if let Some(w) = flags {
            // "| 5, 6 | `width_marginal_l1`, `width_marginal_l2` |" — positional.
            let numbers: Vec<u8> = cells[0]
                .split(',')
                .filter_map(|s| s.trim().parse::<u8>().ok())
                .collect();
            let names = backticked(cells[1]);
            if numbers.is_empty() || names.len() != numbers.len() {
                continue; // header row, or a reserved-range row with no names
            }
            for (bit, name) in numbers.iter().zip(names) {
                bits_checked += 1;
                match w.bits.iter().find(|b| b.name == name) {
                    Some(b) if b.n == *bit => {}
                    Some(b) => problems.push(format!(
                        "{path}:{no}: {}.{name} is bit {} in the crate, {bit} in the document",
                        w.name, b.n
                    )),
                    None => problems.push(format!(
                        "{path}:{no}: {}.{name} does not exist in the crate",
                        w.name
                    )),
                }
            }
            continue;
        }

        let Some((offset, block_here)) = row_offset(cells[0], block) else {
            continue;
        };
        let Some(ty) = cells
            .get(1)
            .map(|c| c.split('×').next().unwrap_or("").trim())
        else {
            continue;
        };
        if ty.is_empty() || !ty.starts_with(['u', 'i']) {
            continue;
        }
        regs_checked += 1;
        match find_reg(block_here, offset) {
            Some(r) if r.ty.wire_name() == ty => {}
            Some(r) => problems.push(format!(
                "{path}:{no}: {}+{offset} is {} in the crate, {ty} in the document",
                block_here.name,
                r.ty.wire_name()
            )),
            None => problems.push(format!(
                "{path}:{no}: {} has no register at offset {offset}",
                block_here.name
            )),
        }
    }

    // A parser that matched nothing would otherwise pass silently.
    if regs_checked < 40 || bits_checked < 30 {
        eprintln!(
            "{path}: only {regs_checked} registers and {bits_checked} flag bits matched — \
             the tables moved out from under this check, fix the parser before trusting it"
        );
        return ExitCode::FAILURE;
    }
    if problems.is_empty() {
        println!(
            "{path}: {regs_checked} registers and {bits_checked} flag bits agree with the map"
        );
        return ExitCode::SUCCESS;
    }
    for p in &problems {
        eprintln!("{p}");
    }
    eprintln!(
        "\n{} disagreement(s) with beam402-protocol.",
        problems.len()
    );
    ExitCode::FAILURE
}

fn backticked(cell: &str) -> Vec<&str> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.contains(' '))
        .collect()
}

/// "0x0041", "0x0014–0x0016" or "+0x06 + 6*i*" → an offset within a block.
fn row_offset<'a>(cell: &str, current: Option<&'a BlockDesc>) -> Option<(u16, &'a BlockDesc)> {
    let relative = cell.starts_with('+');
    let hex = cell
        .split_once("0x")?
        .1
        .split(|c: char| !c.is_ascii_hexdigit())
        .next()?;
    let value = u16::from_str_radix(hex, 16).ok()?;
    if relative {
        current.map(|b| (value, b))
    } else {
        REGISTER_MAP.iter().find_map(|b| {
            b.addrs
                .iter()
                .find(|a| value >= **a && value < **a + b.len)
                .map(|a| (value - a, b))
        })
    }
}

fn find_reg(b: &BlockDesc, offset: u16) -> Option<&'static RegDesc> {
    if let Some(r) = b.regs.iter().find(|r| r.offset == offset) {
        return Some(r);
    }
    b.groups.iter().find_map(|g| {
        g.regs.iter().find(|r| {
            g.offset + r.offset == offset
                || offset >= g.offset && (offset - g.offset) % g.size == r.offset
        })
    })
}

// ---------------------------------------------------------------------------
// docs/registers.toml
// ---------------------------------------------------------------------------

fn render_toml() -> String {
    let mut s = String::new();
    let hr = "# ---------------------------------------------------------------------------\n";

    s.push_str("# Beam402 — machine-readable register map\n#\n");
    s.push_str("# GENERATED by `cargo run -p beam402-render-map -- toml`. Do not edit by hand.\n");
    s.push_str("# The map lives in software/crates/protocol; this file and the tables in\n");
    s.push_str("# protocol.md §3 are printed from it, so an offset or a flag bit exists in\n");
    s.push_str("# exactly one place. A register map maintained by hand in a document and\n");
    s.push_str("# transcribed into two codebases drifts silently, and the failure it produces\n");
    s.push_str("# is a valid number read from the wrong register.\n#\n");
    s.push_str("# Nothing has run against hardware. Keep it accurate anyway.\n\n");

    let _ = writeln!(s, "protocol_version = {PROTOCOL_VERSION}\n");

    s.push_str(hr);
    s.push_str("# Conventions (see protocol.md §2)\n");
    s.push_str(hr);
    s.push_str("[conventions]\n");
    let _ = writeln!(
        s,
        "word_order = {:?}     # a u32 at A has its high 16 bits at A",
        CONVENTIONS.word_order
    );
    let _ = writeln!(s, "signed = {:?}", CONVENTIONS.signed);
    let _ = writeln!(
        s,
        "tick_hz = {}                   # 12.5 ns; wraps at ~53.7 s (D20)",
        group_digits(CONVENTIONS.tick_hz)
    );
    let _ = writeln!(s, "temperature_unit = {:?}", CONVENTIONS.temperature_unit);
    let _ = writeln!(s, "voltage_unit = {:?}", CONVENTIONS.voltage_unit);
    let _ = writeln!(s, "reserved_policy = {:?}", CONVENTIONS.reserved_policy);
    s.push_str("function_codes = { read = 3, write_single = 6, write_multiple = 16 }\n\n");

    s.push_str("[link]\n");
    let _ = writeln!(s, "baud = {}", LINK.baud);
    let _ = writeln!(s, "framing = {:?}", LINK.framing);
    let _ = writeln!(s, "address_min = {}", LINK.address_min);
    let _ = writeln!(s, "address_max = {}", LINK.address_max);
    let _ = writeln!(s, "response_timeout_ms = {}", LINK.response_timeout_ms);
    let _ = writeln!(s, "retries = {}", LINK.retries);
    let _ = writeln!(s, "broadcast_used = {}\n", LINK.broadcast_used);

    s.push_str("[device_class]\ntiming_node = 1\ntree_module = 2\n");

    for b in REGISTER_MAP {
        s.push('\n');
        s.push_str(hr);
        let _ = writeln!(s, "# {} — {}", addr_list(b), heading(b));
        s.push_str(hr);
        s.push_str("[[block]]\n");
        let _ = writeln!(s, "name = {:?}", b.name);
        if b.addrs.len() == 1 {
            let _ = writeln!(s, "addr = {:#06X}", b.addrs[0]);
        } else {
            let addrs: Vec<String> = b.addrs.iter().map(|a| format!("{a:#06X}")).collect();
            let _ = writeln!(s, "addr = [{}]", addrs.join(", "));
            let lanes: Vec<String> = b.lanes.iter().map(u8::to_string).collect();
            let _ = writeln!(s, "lanes = [{}]", lanes.join(", "));
            let _ = writeln!(s, "stride = {:#04X}", b.stride);
        }
        let _ = writeln!(s, "length = {}", b.len);
        let _ = writeln!(s, "poll = {:?}", poll_name(b.poll));
        if b.access == Access::Write {
            s.push_str("access = \"write\"\n");
        }
        if b.atomic {
            s.push_str("atomic = true\n");
        }
        if let Some(dc) = b.device_class {
            let _ = writeln!(s, "device_class = {dc}");
        }
        if !b.doc.is_empty() {
            let _ = writeln!(s, "doc = {:?}", b.doc);
        }
        for r in b.regs {
            s.push_str(&reg_toml("block.reg", r));
        }
        for g in b.groups {
            s.push_str(&group_toml(g));
        }
    }

    s.push('\n');
    s.push_str(hr);
    s.push_str("# Flag words\n");
    s.push_str(hr);
    for w in FLAG_WORDS {
        s.push_str("[[flags]]\n");
        let _ = writeln!(s, "name = {:?}", w.name);
        s.push_str("bits = [\n");
        for b in w.bits {
            if b.doc.is_empty() {
                let _ = writeln!(s, "    {{ n = {}, name = {:?} }},", b.n, b.name);
            } else {
                let _ = writeln!(
                    s,
                    "    {{ n = {}, name = {:?}, doc = {:?} }},",
                    b.n, b.name, b.doc
                );
            }
        }
        s.push_str("]\n\n");
    }

    s.push_str(hr);
    s.push_str("# Command opcodes\n");
    s.push_str(hr);
    s.push_str("[opcode]\n");
    let pad = OPCODES.iter().map(|o| o.name.len()).max().unwrap_or(0);
    for o in OPCODES {
        if o.args.is_empty() {
            let _ = writeln!(s, "{} = {}", o.name, o.code);
        } else {
            let lhs = format!("{} = {}", o.name, o.code);
            let _ = writeln!(s, "{lhs:width$}  # {}", o.args, width = pad + 5);
        }
    }

    s.push('\n');
    s.push_str(hr);
    s.push_str("# Beam meanings — a closed set. An unknown value is a mapping-file load error,\n");
    s.push_str("# not a warning: a typo must not silently drop a beam.\n");
    s.push_str(hr);
    s.push_str("[beam]\nmeanings = [\n");
    for m in BEAM_MEANINGS {
        let _ = writeln!(s, "    {m:?},");
    }
    s.push_str("]\n");
    s
}

fn reg_toml(table: &str, r: &RegDesc) -> String {
    let mut s = format!(
        "\n[[{table}]]\noffset = {}\nname = {:?}\n",
        r.offset, r.name
    );
    let _ = writeln!(s, "type = {:?}", r.ty.wire_name());
    if let Some(f) = r.flags {
        let _ = writeln!(s, "flags = {f:?}");
    }
    if let Some(e) = r.enumeration {
        let _ = writeln!(s, "enum = {e:?}");
    }
    if r.count > 1 {
        let _ = writeln!(s, "count = {}", r.count);
    }
    if !r.doc.is_empty() {
        let _ = writeln!(s, "doc = {:?}", r.doc);
    }
    s
}

fn group_toml(g: &GroupDesc) -> String {
    let mut s = format!(
        "\n[[block.group]]\nname = {:?}\ncount = {}\noffset = {}\nsize = {}\n",
        g.name, g.count, g.offset, g.size
    );
    for r in g.regs {
        s.push_str(&reg_toml("block.group.reg", r));
    }
    s
}

fn addr_list(b: &BlockDesc) -> String {
    b.addrs
        .iter()
        .map(|a| format!("{a:#06X}"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn heading(b: &BlockDesc) -> String {
    let title = match b.name {
        "digest" => "Digest. Read every poll cycle (D25).",
        "identity" => "Identity. Static after boot.",
        "status" => "Status and counters.",
        "telemetry" => "Telemetry. One device per cycle, round-robin.",
        "pulse" => "Pulse observation. Present on EVERY device (D24).",
        "run_record" => "Run records. MUST be read in one transaction (protocol.md §2).",
        "tree" => "Tree module only (device_class = 2). See software.md §5.",
        "command" => "Commands (FC6 / FC16). Confirmed by reading command_seq_echo.",
        other => other,
    };
    title.to_string()
}

fn group_digits(v: u32) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('_');
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------------------
// protocol.md §3
// ---------------------------------------------------------------------------

fn render_tables() -> String {
    let mut s = String::from("## 3. Register map\n\n");
    s.push_str("<!-- GENERATED by `cargo run -p beam402-render-map -- tables`. -->\n\n");
    s.push_str("Blocks are laid out so the cheapest and most frequent read is also the first.\n");

    for b in REGISTER_MAP {
        let head = heading(b);
        let _ = write!(s, "\n### {} — {head}\n\n", addr_list(b));
        // The heading already carries the short form for most blocks; only the
        // longer note is worth repeating underneath it.
        if !b.doc.is_empty() && !head.contains(b.doc) {
            let _ = write!(s, "{}\n\n", b.doc);
        }
        s.push_str("| Addr | Type | Name |\n|---|---|---|\n");
        let base = b.addrs[0];
        for r in b.regs {
            s.push_str(&reg_row(base, r, 0));
        }
        for g in b.groups {
            for r in g.regs {
                let addr = g.offset + r.offset;
                let span = if r.ty.words() > 1 {
                    format!("+{:#04X}..+{:#04X}", addr, addr + r.ty.words() - 1)
                } else {
                    format!("+{addr:#04X}")
                };
                let _ = writeln!(
                    s,
                    "| {} + {}*i | {} | `{}[i]` |",
                    span,
                    g.size,
                    r.ty.wire_name(),
                    r.name
                );
            }
            let _ = write!(s, "\nfor *i* = 0..{}.\n", g.count - 1);
        }
    }

    s.push_str("\n### Flag words\n");
    for w in FLAG_WORDS {
        let _ = write!(s, "\n`{}`:\n\n| Bit | Meaning |\n|---|---|\n", w.name);
        for b in w.bits {
            if b.doc.is_empty() {
                let _ = writeln!(s, "| {} | `{}` |", b.n, b.name);
            } else {
                let _ = writeln!(s, "| {} | `{}` — {} |", b.n, b.name, b.doc);
            }
        }
    }

    s.push_str("\n### Command opcodes\n\n| Opcode | Command | Arguments |\n|---|---|---|\n");
    for o in OPCODES {
        let _ = writeln!(s, "| {} | `{}` | {} |", o.code, o.name, o.args);
    }
    s
}

fn reg_row(base: u16, r: &RegDesc, extra: u16) -> String {
    let addr = base + r.offset + extra;
    let words = r.words();
    let span = if words > 1 {
        format!("{:#06X}–{:#06X}", addr, addr + words - 1)
    } else {
        format!("{addr:#06X}")
    };
    let ty = if r.count > 1 {
        format!("{} × {}", r.ty.wire_name(), r.count)
    } else {
        r.ty.wire_name().to_string()
    };
    let note = if r.doc.is_empty() {
        String::new()
    } else {
        format!(" — {}", r.doc)
    };
    format!("| {span} | {ty} | `{}`{note} |\n", r.name)
}
