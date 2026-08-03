//! The receiving end (**D33**): somewhere for a day to arrive.
//!
//! This is the same binary as race control, run somewhere with a public address —
//! a club's VPS, a league's box, a laptop at home. **D23** buys that for free, and
//! it means the "cloud" in this project is a thing a club can host itself rather
//! than a service it depends on.
//!
//! ## It decides nothing
//!
//! It stores two files per event and derives everything with the same crate the
//! tower uses. No rules, no scoring, no pairing. That is what keeps the online
//! ladder from ever contradicting the one people are racing off, and it is why
//! this file is mostly routing.
//!
//! ## State is a directory
//!
//! `<root>/<slug>/sheet.toml` and `<root>/<slug>/results.log`, and that is all.
//! Inspectable with `cat`, backed up with `cp`, and re-uploadable — a day that
//! arrived here can be carried onward without this program's help.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use beam402_event::sync::{Held, SyncError};
use beam402_http::{Method, Request, Response};

use crate::live::escape;

/// One event's two files on disk.
struct Store {
    root: PathBuf,
}

impl Store {
    /// A slug that is safe to join onto a path.
    ///
    /// **D32** removes traversal by not having a filesystem behind the server;
    /// this one does have a filesystem, so the defence has to be here instead: an
    /// allow-list of characters, which also happens to be what makes a slug worth
    /// putting in a URL.
    fn slug(raw: &str) -> Option<String> {
        let ok = !raw.is_empty()
            && raw.len() <= 64
            && raw
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
        ok.then(|| raw.to_string())
    }

    fn dir(&self, slug: &str) -> PathBuf {
        self.root.join(slug)
    }

    fn read(&self, slug: &str) -> Option<Held> {
        let dir = self.dir(slug);
        let sheet = std::fs::read_to_string(dir.join("sheet.toml")).ok()?;
        let log = std::fs::read_to_string(dir.join("results.log")).unwrap_or_default();
        Some(Held::new(sheet, log))
    }

    fn write(&self, slug: &str, held: &Held) -> std::io::Result<()> {
        let dir = self.dir(slug);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("sheet.toml"), &held.sheet)?;
        std::fs::write(dir.join("results.log"), &held.log)
    }

    fn events(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for e in entries.flatten() {
                if let Some(slug) = e.file_name().to_str().and_then(Store::slug) {
                    if e.path().join("sheet.toml").is_file() {
                        out.push(slug);
                    }
                }
            }
        }
        out.sort();
        out
    }
}

/// Serve the receiving end.
///
/// One lock around the store, held only for the length of a request. Appends are
/// short and rare — a handful an hour per event — and the alternative is a
/// read-modify-write race that loses a result.
pub fn serve(root: &Path, addr: &str) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let store = Mutex::new(Store {
        root: root.to_path_buf(),
    });

    println!("beam402: hosting {} on http://{addr}", root.display());
    println!("  /                       events held here");
    println!("  /event/<slug>           the day, as it stands");
    println!("  POST /api/event/<slug>/sheet   the entry sheet");
    println!("  POST /api/event/<slug>/log     results, appended from an offset");

    beam402_http::serve(addr, move |r: &Request| {
        let store = match store.lock() {
            Ok(s) => s,
            // A poisoned lock means a handler panicked mid-write. Serving on
            // regardless would serve from a store nobody has looked at.
            Err(_) => return Response::text(500, "the store is in an unknown state\n"),
        };
        route(&store, r)
    })
    .map_err(|e| format!("{addr}: {e}"))
}

fn route(store: &Store, r: &Request) -> Response {
    let parts: Vec<&str> = r.path.trim_matches('/').split('/').collect();
    match (r.method, parts.as_slice()) {
        (Method::Get | Method::Head, [""]) => Response::html(index(store)),

        (Method::Get | Method::Head, ["event", slug]) => match held(store, slug) {
            Ok(held) => Response::html(page(slug, &held)),
            Err(res) => res,
        },
        (Method::Get | Method::Head, ["api", "event", slug]) => match held(store, slug) {
            Ok(held) => {
                let c = held.cursor();
                Response::json(format!(
                    "{{\"lines\":{},\"sheet\":\"{}\"}}",
                    c.lines, c.sheet
                ))
            }
            Err(res) => res,
        },
        (Method::Get | Method::Head, ["api", "event", slug, "state"]) => match held(store, slug) {
            Ok(held) => match held.day() {
                Ok((day, skipped)) => Response::json(crate::event_json(&day, skipped)),
                Err(why) => Response::text(500, format!("{why}\n")),
            },
            Err(res) => res,
        },
        // The log itself, because a mirror that cannot be copied onward is not
        // much of a mirror.
        (Method::Get | Method::Head, ["api", "event", slug, "log"]) => match held(store, slug) {
            Ok(held) => Response::text(200, held.log),
            Err(res) => res,
        },

        (Method::Post, ["api", "event", slug, "sheet"]) => {
            let Some(slug) = Store::slug(slug) else {
                return bad_slug();
            };
            let Ok(text) = String::from_utf8(r.body.clone()) else {
                return Response::text(400, "the sheet is not UTF-8\n");
            };
            let mut held = store.read(&slug).unwrap_or_default();
            match held.offer_sheet(&text) {
                Ok(c) => match store.write(&slug, &held) {
                    Ok(()) => Response::json(format!(
                        "{{\"lines\":{},\"sheet\":\"{}\"}}",
                        c.lines, c.sheet
                    )),
                    Err(e) => Response::text(500, format!("{e}\n")),
                },
                Err(e) => refused(&e),
            }
        }

        (Method::Post, ["api", "event", slug, "log"]) => {
            let Some(slug) = Store::slug(slug) else {
                return bad_slug();
            };
            let Some(from) = r.param("from").and_then(|v| v.parse::<usize>().ok()) else {
                return Response::text(400, "from is required and is a line count\n");
            };
            let prefix = r.param("prefix").unwrap_or_default();
            let Ok(body) = String::from_utf8(r.body.clone()) else {
                return Response::text(400, "the log is not UTF-8\n");
            };
            // No sheet means no event: results cannot be filed against nothing,
            // and creating an event from a log would create one with no classes.
            let Some(mut held) = store.read(&slug) else {
                return Response::text(404, "no such event — send the entry sheet first\n");
            };
            match held.append(from, &prefix, &body) {
                Ok(a) => match store.write(&slug, &held) {
                    Ok(()) => Response::json(format!(
                        "{{\"lines\":{},\"added\":{},\"skipped\":{}}}",
                        a.lines, a.added, a.skipped
                    )),
                    Err(e) => Response::text(500, format!("{e}\n")),
                },
                Err(e) => refused(&e),
            }
        }

        (Method::Get | Method::Head, _) => Response::text(404, "no such thing\n"),
        _ => Response::text(405, "method not allowed\n"),
    }
}

fn held(store: &Store, slug: &str) -> Result<Held, Response> {
    let slug = Store::slug(slug).ok_or_else(bad_slug)?;
    store
        .read(&slug)
        .ok_or_else(|| Response::text(404, "no such event\n"))
}

fn bad_slug() -> Response {
    Response::text(
        400,
        "an event id is lower-case letters, digits, - and _, up to 64 of them\n",
    )
}

/// Every sync refusal is a **409**, and each one carries the fact the client needs
/// to continue: the true line count, or the point of divergence. A client that
/// treats these as errors to give up on is a client that cannot resume.
fn refused(e: &SyncError) -> Response {
    let detail = match e {
        SyncError::Offset { held, .. } => format!(",\"lines\":{held}"),
        SyncError::Forked { at } => format!(",\"forked_at\":{at}"),
        _ => String::new(),
    };
    Response::new(
        409,
        "application/json; charset=utf-8",
        format!(
            "{{\"ok\":false,\"why\":\"{}\"{detail}}}",
            escape(&e.to_string())
        ),
    )
}

// -- the pages -----------------------------------------------------------

const STYLE: &str = "\
:root{--ink:#1a1c1e;--dim:#5f6a70;--faint:#98a2a8;--line:#dfe3e5;--card:#fff;--bg:#f4f5f6;\
--accent:#0b6fb4;--win:#0a7a3f;--mono:ui-monospace,SFMono-Regular,Menlo,monospace}\
@media(prefers-color-scheme:dark){:root{--ink:#e7e4dd;--dim:#8c979e;--faint:#5a656b;\
--line:#252c31;--card:#121618;--bg:#0b0d0e;--accent:#5aa9e6;--win:#38d26b}}\
*{box-sizing:border-box}\
body{background:var(--bg);color:var(--ink);margin:0;padding:24px 20px;\
font:14px/1.55 var(--mono);max-width:820px;margin-inline:auto;\
font-variant-numeric:tabular-nums}\
h1{font-size:17px;font-weight:600;letter-spacing:.1em;text-transform:uppercase;margin:0 0 2px}\
.sub{color:var(--dim);font-size:12px;margin-bottom:20px}\
.card{background:var(--card);border:1px solid var(--line);border-radius:4px;\
padding:14px 16px;margin-bottom:14px}\
h2{font-size:12px;font-weight:600;letter-spacing:.12em;text-transform:uppercase;\
color:var(--dim);margin:0 0 10px}\
table{border-collapse:collapse;width:100%}\
th{text-align:left;font-weight:400;color:var(--faint);font-size:11px;letter-spacing:.06em;\
text-transform:uppercase;padding:0 10px 5px 0;border-bottom:1px solid var(--line)}\
td{padding:5px 10px 5px 0;border-bottom:1px solid var(--line);white-space:nowrap}\
td.n{text-align:right}\
.seed{color:var(--accent)}\
.won{color:var(--win)}\
.pair{display:flex;gap:10px;align-items:baseline;padding:3px 0;color:var(--dim)}\
.pair .who{color:var(--ink)}\
.mark{width:1.3em;color:var(--win)}\
ul{list-style:none;padding:0;margin:0}li{padding:4px 0}\
a{color:var(--accent)}\
footer{color:var(--faint);font-size:11px;margin-top:18px}\
.warn{color:#b06000;font-size:12px}\
@media(prefers-color-scheme:dark){.warn{color:#ffa92b}}\
";

fn index(store: &Store) -> String {
    let events = store.events();
    let mut list = String::new();
    for slug in &events {
        let name = store
            .read(slug)
            .and_then(|h| beam402_event::Sheet::parse(&h.sheet).ok())
            .map(|s| format!("{} — {}", s.event.name, s.event.date))
            .unwrap_or_else(|| slug.clone());
        list.push_str(&format!(
            "<li><a href=\"/event/{0}\">{1}</a> <span class=seed>{0}</span></li>",
            escape_html(slug),
            escape_html(&name)
        ));
    }
    if events.is_empty() {
        list.push_str("<li class=warn>Nothing has been uploaded here yet.</li>");
    }
    format!(
        "<title>beam402 — events</title><style>{STYLE}</style>\
<h1>beam402</h1><div class=sub>Days that have been carried here.</div>\
<div class=card><h2>Events</h2><ul>{list}</ul></div>\
<footer>A mirror. Every number here was measured at a track and derived from the \
same result log the tower used.</footer>"
    )
}

fn page(slug: &str, held: &Held) -> String {
    let (day, skipped) = match held.day() {
        Ok(v) => v,
        Err(why) => {
            return format!(
                "<title>beam402</title><style>{STYLE}</style><h1>beam402</h1>\
<div class=card><h2>This event will not load</h2><div class=warn>{}</div></div>",
                escape_html(&why)
            )
        }
    };
    let sheet = day.sheet();
    let mut body = String::new();

    if skipped > 0 {
        body.push_str(&format!(
            "<div class=card><div class=warn>{skipped} line(s) of this day's log do not \
parse. They are mirrored as they arrived — one is a torn write after a power cut, \
more than that is a file somebody has to look at.</div></div>"
        ));
    }

    for name in day.class_names().map(str::to_string).collect::<Vec<_>>() {
        body.push_str(&format!("<div class=card><h2>{}</h2>", escape_html(&name)));

        match day.field(&name) {
            None => body.push_str(&format!(
                "<div class=sub style=margin:0>Qualifying — {} entered, the ladder has \
not been drawn.</div>",
                sheet.entries_in(&name).len()
            )),
            Some(field) => {
                body.push_str("<table><tr><th>seed</th><th>driver</th><th>car</th></tr>");
                for (seed, id) in field.seeds() {
                    let car = sheet.entry(id).map(|e| e.car.clone()).unwrap_or_default();
                    let champion = day.champion(&name) == Some(seed);
                    body.push_str(&format!(
                        "<tr><td class=\"n seed\">{seed}</td><td{}>{}{}</td><td>{}</td></tr>",
                        if champion { " class=won" } else { "" },
                        escape_html(&day.driver(id)),
                        if champion { " — winner" } else { "" },
                        escape_html(&car)
                    ));
                }
                body.push_str("</table>");
            }
        }

        if let Some(round) = day.round(&name) {
            if day.champion(&name).is_none() {
                body.push_str(&format!(
                    "<h2 style=margin-top:14px>{}</h2>",
                    escape_html(&beam402_event::round_name(round.pairs.len(), round.number))
                ));
                for p in &round.pairs {
                    let won = round.winner(p.position);
                    let side = |s: Option<usize>| match s {
                        None => "<span>bye</span>".to_string(),
                        Some(s) => format!(
                            "<span class=seed>{s}</span> <span class=\"who{}\">{}</span>",
                            if won == Some(s) { " won" } else { "" },
                            escape_html(
                                &day.field(&name)
                                    .and_then(|f| f.entry(s))
                                    .map(|id| day.driver(id))
                                    .unwrap_or_default()
                            )
                        ),
                    };
                    body.push_str(&format!(
                        "<div class=pair><span class=mark>{}</span>{}<span>v</span>{}</div>",
                        if won.is_some() { "✓" } else { "" },
                        side(Some(p.left)),
                        side(p.right)
                    ));
                }
            }
        }
        body.push_str("</div>");
    }

    format!(
        "<title>{} — beam402</title><style>{STYLE}</style>\
<h1>{}</h1><div class=sub>{} · <span class=seed>{}</span> · \
<a href=\"/api/event/{}/log\">result log</a> · <a href=\"/\">all events</a></div>{body}\
<footer>Derived from the result log this day was raced off — not a second copy of \
the ladder. Nothing here has run against hardware.</footer>",
        escape_html(&sheet.event.name),
        escape_html(&sheet.event.name),
        escape_html(&sheet.event.date),
        escape_html(slug),
        escape_html(slug),
    )
}

/// HTML text escaping. Small and separate from the JSON one next door, because
/// the two have different unsafe characters and sharing them is how a page ends
/// up with the wrong one.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_cannot_reach_out_of_the_store() {
        // D32 removes traversal by having no filesystem; this server has one, so
        // the allow-list is where that defence lives instead.
        for bad in [
            "../etc", "a/b", "..", "Club-Day", "club day", "", "a\0b", "./x",
        ] {
            assert_eq!(Store::slug(bad), None, "{bad:?} was accepted");
        }
        assert_eq!(
            Store::slug("kaluga-2026-08-15").as_deref(),
            Some("kaluga-2026-08-15")
        );
    }

    #[test]
    fn a_drivers_name_cannot_carry_markup_into_the_page() {
        assert_eq!(
            escape_html("<script>x</script>"),
            "&lt;script&gt;x&lt;/script&gt;"
        );
        assert_eq!(escape_html("O'Neill & Sons"), "O&#39;Neill &amp; Sons");
    }
}
