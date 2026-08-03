//! Sending a day somewhere (**D33**), and the client half of the three refusals.
//!
//! Three requests: ask where the receiver is, offer the sheet, append the tail.
//! Idempotent, resumable, and safe to run on a timer — which is what makes "live"
//! and "that evening from the car park" the same code path rather than two.
//!
//! ## A 409 is not an error
//!
//! Every refusal the receiver makes carries the fact needed to continue. An offset
//! mismatch means somebody already applied this; the answer is to ask again and
//! send from there, which the next tick does anyway. A client that treated these
//! as failures would be a client that cannot resume, and resuming is the whole
//! point.
//!
//! ## No TLS
//!
//! **D32** put no TLS in the server and **D33** left the question here. This
//! client speaks plain HTTP, which is right for a club hosting its own receiver on
//! a LAN or a VPS behind something that terminates TLS, and wrong for pushing
//! across the open internet to a public host. Adding a TLS crate is a dependency
//! decision worth making against a real server rather than in advance — and it is
//! *only* a decision for this file, because a push client never runs on a tree,
//! which is the constraint that shaped D32 in the first place.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use beam402_event::sync::{prefix_digest, tail};

pub struct Report {
    pub lines: usize,
    pub added: usize,
    pub skipped: usize,
    /// True when the sheet on the receiver was created or replaced.
    pub sheet_sent: bool,
}

/// Where a receiver is: host, port, and any path it is mounted under.
#[derive(Debug)]
struct Target {
    host: String,
    port: u16,
    prefix: String,
}

fn target(url: &str) -> Result<Target, String> {
    let rest = match url.split_once("://") {
        Some(("http", rest)) => rest,
        Some(("https", _)) => {
            return Err(
                "this client speaks plain HTTP only — put TLS in front of the \
                        receiver, or host it on the club's own network"
                    .into(),
            )
        }
        Some((scheme, _)) => return Err(format!("unknown scheme {scheme:?}")),
        None => url,
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse().map_err(|_| format!("{p:?} is not a port"))?,
        ),
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err(format!("{url:?} names no host"));
    }
    Ok(Target {
        host,
        port,
        prefix: path.trim_end_matches('/').to_string(),
    })
}

/// One request, one connection, closed after. The receiver closes too (**D32**),
/// so reading to EOF is the whole response.
fn request(
    t: &Target,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", t.host, t.port);
    let mut s = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
    let timeout = Duration::from_secs(20);
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));

    let mut head = format!(
        "{method} {}{path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\
         Content-Length: {}\r\n",
        t.prefix,
        t.host,
        body.len()
    );
    if let Some(ct) = content_type {
        head.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    head.push_str("\r\n");

    s.write_all(head.as_bytes())
        .and_then(|()| s.write_all(body))
        .map_err(|e| format!("{addr}: {e}"))?;

    let mut raw = Vec::new();
    s.read_to_end(&mut raw)
        .map_err(|e| format!("{addr}: {e}"))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| format!("{addr}: not an HTTP response"))?;
    // Headers end at the blank line; everything after it is the body. There is no
    // chunking to unwrap because the receiver does not do any.
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// One field out of a small flat JSON object.
///
/// Enough for `{"lines":12,"sheet":"..."}` and nothing more. The alternative is a
/// JSON parser in a binary whose whole stance is that it does not have one, for a
/// payload this program's own server wrote.
fn field(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(q) = rest.strip_prefix('"') {
        return q.split('"').next().map(str::to_string);
    }
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

fn number(json: &str, key: &str) -> Option<usize> {
    field(json, key)?.parse().ok()
}

/// Push a day. Safe to call repeatedly: it asks where the receiver is and sends
/// only what is missing.
pub fn push(url: &str, slug: &str, sheet: &str, log: &str) -> Result<Report, String> {
    let t = target(url)?;
    let digest = beam402_event::sync::digest(sheet);

    // Where is it, and is it even the same sheet?
    let (status, body) = request(&t, "GET", &format!("/api/event/{slug}"), None, b"")?;
    let (mut from, sheet_sent) = match status {
        200 if field(&body, "sheet").as_deref() == Some(digest.as_str()) => {
            (number(&body, "lines").unwrap_or(0), false)
        }
        // Absent, or holding a different sheet: offer this one. A late entry added
        // at the desk is exactly this case, and the receiver refuses it only if the
        // log it already holds would be orphaned.
        200 | 404 => {
            let (s, b) = request(
                &t,
                "POST",
                &format!("/api/event/{slug}/sheet"),
                Some("text/plain; charset=utf-8"),
                sheet.as_bytes(),
            )?;
            if s != 200 {
                return Err(why(s, &b));
            }
            (number(&b, "lines").unwrap_or(0), true)
        }
        s => return Err(why(s, &body)),
    };

    // Append the tail. One retry, because the only refusal worth retrying is an
    // offset that moved under us, and the receiver hands back the true count.
    for attempt in 0..2 {
        let (status, body) = request(
            &t,
            "POST",
            &format!(
                "/api/event/{slug}/log?from={from}&prefix={}",
                prefix_digest(log, from)
            ),
            Some("text/plain; charset=utf-8"),
            tail(log, from).as_bytes(),
        )?;
        match status {
            200 => {
                return Ok(Report {
                    lines: number(&body, "lines").unwrap_or(0),
                    added: number(&body, "added").unwrap_or(0),
                    skipped: number(&body, "skipped").unwrap_or(0),
                    sheet_sent,
                })
            }
            409 if attempt == 0 => match number(&body, "lines") {
                Some(held) => from = held,
                None => return Err(why(status, &body)),
            },
            s => return Err(why(s, &body)),
        }
    }
    Err("the receiver's log kept moving — is something else uploading to it?".into())
}

fn why(status: u16, body: &str) -> String {
    match field(body, "why") {
        Some(w) => format!("{status}: {w}"),
        None => format!("{status}: {}", body.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_becomes_a_host_a_port_and_a_prefix() {
        let t = target("http://results.example:8402/beam").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.prefix.as_str()),
            ("results.example", 8402, "/beam")
        );
        let t = target("results.example").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.prefix.as_str()),
            ("results.example", 80, "")
        );
        let t = target("http://10.0.0.4/").unwrap();
        assert_eq!(t.prefix, "", "a bare slash is not a prefix");
    }

    #[test]
    fn https_is_refused_with_the_reason_rather_than_attempted() {
        // Failing to connect would be worse than saying so: a club would spend the
        // evening wondering about its firewall.
        let e = target("https://results.example").unwrap_err();
        assert!(e.contains("plain HTTP"), "{e}");
        assert!(target("ftp://x").is_err());
        assert!(target("http://:80").is_err());
    }

    #[test]
    fn one_field_out_of_a_small_object() {
        let j = r#"{"lines":12,"sheet":"deadbeefdeadbeef","added":3}"#;
        assert_eq!(number(j, "lines"), Some(12));
        assert_eq!(number(j, "added"), Some(3));
        assert_eq!(field(j, "sheet").as_deref(), Some("deadbeefdeadbeef"));
        assert_eq!(field(j, "nope"), None);
        assert_eq!(
            field(r#"{"ok":false,"why":"the log is 6 lines long"}"#, "why").as_deref(),
            Some("the log is 6 lines long")
        );
    }
}
