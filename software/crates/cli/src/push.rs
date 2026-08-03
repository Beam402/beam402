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
//! ## TLS lives here and nowhere else
//!
//! **D32** put no TLS in the *server* and said the right place for it was the
//! client. This is that place (**D36**): `https` is spoken by rustls behind a
//! cargo feature, and nothing in the timing path or on a tree ever compiles it.
//! `--no-default-features` is the dependency-free build for a machine that never
//! leaves the track.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use beam402_event::sync::{prefix_digest, tail};

/// The header a write is authorized with (**D33**): the first writer claims the
/// event, and every later append presents the same secret.
const TOKEN_HEADER: &str = "X-Beam402-Token";

/// Either kind of connection, so the request writer above it does not know which.
///
/// The same shape as the [`Bus`](beam402_bus::Bus) seam and for the same reason:
/// one code path, and the transport is the only thing that varies.
enum Wire {
    Plain(TcpStream),
    #[cfg(feature = "tls")]
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Wire::Plain(s) => s.read(buf),
            #[cfg(feature = "tls")]
            Wire::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Wire::Plain(s) => s.write(buf),
            #[cfg(feature = "tls")]
            Wire::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Wire::Plain(s) => s.flush(),
            #[cfg(feature = "tls")]
            Wire::Tls(s) => s.flush(),
        }
    }
}

/// Open a connection, encrypted or not.
///
/// **Certificates are verified and there is no flag to stop that.** An
/// `--insecure` switch is the kind of thing that gets pasted into a club's script
/// once and stays there, and what it would be protecting is a season of results.
/// A receiver with a self-signed certificate is reached over plain `http` on a
/// network the club controls instead, which is at least honest about what it is.
fn connect(t: &Target) -> Result<Wire, String> {
    let addr = format!("{}:{}", t.host, t.port);
    let tcp = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
    let timeout = Duration::from_secs(20);
    let _ = tcp.set_read_timeout(Some(timeout));
    let _ = tcp.set_write_timeout(Some(timeout));
    if !t.tls {
        return Ok(Wire::Plain(tcp));
    }
    #[cfg(not(feature = "tls"))]
    {
        Err(format!(
            "{addr}: this build has no TLS — rebuild with the default features, or \
             reach the receiver over http on a network you control"
        ))
    }
    #[cfg(feature = "tls")]
    {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
        let name = rustls_pki_types::ServerName::try_from(t.host.clone()).map_err(|_| {
            format!(
                "{:?} is not a name a certificate can be checked against",
                t.host
            )
        })?;
        let conn = rustls::ClientConnection::new(std::sync::Arc::new(config), name)
            .map_err(|e| format!("TLS: {e}"))?;
        Ok(Wire::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
    }
}

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
    tls: bool,
}

fn target(url: &str) -> Result<Target, String> {
    let (tls, rest) = match url.split_once("://") {
        Some(("http", rest)) => (false, rest),
        Some(("https", rest)) => (true, rest),
        Some((scheme, _)) => return Err(format!("unknown scheme {scheme:?}")),
        // No scheme is plain HTTP, because that is what a club's own box is.
        None => (false, url),
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
        None => (authority.to_string(), if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("{url:?} names no host"));
    }
    Ok(Target {
        host,
        port,
        prefix: path.trim_end_matches('/').to_string(),
        tls,
    })
}

/// One request, one connection, closed after. The receiver closes too (**D32**),
/// so reading to EOF is the whole response.
fn request(
    t: &Target,
    method: &str,
    path: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: &[u8],
) -> Result<(u16, String), String> {
    let addr = format!("{}:{}", t.host, t.port);
    let mut s = connect(t)?;

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
    if let Some(token) = token {
        head.push_str(&format!("{TOKEN_HEADER}: {token}\r\n"));
    }
    // No compression, because this client does not decompress and the bodies are
    // two numbers. A reverse proxy with `encode` on would otherwise gzip a reply
    // this cannot read — which is the documented deployment (`deploy/Caddyfile`),
    // so it is not a hypothetical.
    head.push_str("Accept-Encoding: identity\r\n\r\n");

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
    // Headers end at the blank line; everything after it is the body.
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    // `beam402 host` always sends `Content-Length` and closes (**D32**), so this
    // would be unnecessary if it were the only thing ever answering. It is not:
    // the documented deployment puts a reverse proxy in front, and a proxy is free
    // to re-frame a response as chunked. Found by pushing at a real host.
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)?
    } else {
        body.to_string()
    };
    Ok((status, body))
}

/// Undo chunked framing.
///
/// Bounded by the input it was given, so a malformed length or a missing
/// terminator ends the loop rather than spinning. Trailers are ignored: nothing
/// this client reads would be in one.
fn dechunk(body: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let Some((line, after)) = rest.split_once("\r\n") else {
            // Ran out mid-frame. Whatever arrived is what there is to report.
            return Ok(out);
        };
        // A chunk-extension after `;` is legal and irrelevant here.
        let size = line.split(';').next().unwrap_or("").trim();
        let n = usize::from_str_radix(size, 16)
            .map_err(|_| format!("a chunked reply with {size:?} as a length"))?;
        if n == 0 {
            return Ok(out);
        }
        if after.len() < n {
            out.push_str(after);
            return Ok(out);
        }
        out.push_str(&after[..n]);
        rest = after[n..].strip_prefix("\r\n").unwrap_or(&after[n..]);
    }
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
pub fn push(url: &str, slug: &str, token: &str, sheet: &str, log: &str) -> Result<Report, String> {
    let t = target(url)?;
    let digest = beam402_event::sync::digest(sheet);

    // Where is it, and is it even the same sheet?
    let (status, body) = request(&t, "GET", &format!("/api/event/{slug}"), None, None, b"")?;
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
                Some(token),
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
            Some(token),
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
    fn https_gets_tls_and_port_443_without_being_asked() {
        let t = target("https://results.example/beam").unwrap();
        assert!(t.tls);
        assert_eq!((t.port, t.prefix.as_str()), (443, "/beam"));
        let t = target("https://results.example:8443").unwrap();
        assert_eq!(t.port, 8443, "an explicit port still wins");
        // No scheme is plain HTTP, because that is what a club's own box is.
        assert!(!target("10.0.0.4:8403").unwrap().tls);
    }

    #[test]
    fn a_scheme_that_is_not_http_says_so_rather_than_being_attempted() {
        assert!(target("ftp://x").unwrap_err().contains("unknown scheme"));
        assert!(target("http://:80").is_err());
    }

    #[cfg(feature = "tls")]
    #[test]
    fn a_host_that_no_certificate_could_name_is_refused_before_connecting() {
        // Reached before any socket, so a malformed target fails at the prompt
        // rather than after a timeout.
        let t = Target {
            host: "not a hostname".into(),
            port: 443,
            prefix: String::new(),
            tls: true,
        };
        let e = match connect(&t) {
            Err(e) => e,
            Ok(_) => panic!("a malformed host must not reach a socket"),
        };
        assert!(
            e.contains("certificate can be checked against") || e.contains("not a hostname"),
            "{e}"
        );
    }

    #[test]
    fn a_chunked_reply_is_unwrapped() {
        // Found by pushing at a real https host: the reply came back chunked and
        // the chunk framing was printed as the body. `beam402 host` never chunks,
        // but the deployment puts a proxy in front of it and a proxy may.
        // Framed from the payload, so the lengths cannot be wrong in the test
        // rather than in the code — which is how this test failed first time.
        let chunk = |parts: &[&str]| {
            let mut out = String::new();
            for p in parts {
                out.push_str(&format!("{:x}\r\n{p}\r\n", p.len()));
            }
            out + "0\r\n\r\n"
        };
        let json = r#"{"ok":false,"why":"no"}"#;
        assert_eq!(dechunk(&chunk(&[json])).unwrap(), json);
        assert_eq!(
            dechunk(&chunk(&["hello", " world"])).unwrap(),
            "hello world"
        );
        // A chunk-extension after `;` is legal and irrelevant.
        assert_eq!(dechunk("5;a=b\r\nhello\r\n0\r\n\r\n").unwrap(), "hello");
        // Truncated mid-frame: report what arrived rather than spinning.
        assert_eq!(dechunk("5\r\nhel").unwrap(), "hel");
        assert_eq!(dechunk("").unwrap(), "");
        assert!(dechunk("zz\r\nx\r\n").is_err());
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
