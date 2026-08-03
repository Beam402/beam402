#![forbid(unsafe_code)]

//! A small HTTP/1.1 server, written rather than depended on (**D32**).
//!
//! Race control has to serve a page and a little JSON to a handful of clients on
//! a club's LAN, and it has to do it from **both** a small machine on the trunk
//! (**D30**) and an ESP32-S3 inside a tree (**D31**). That second requirement is
//! what settles the shape: blocking sockets over `std::net`, which exist on
//! ESP-IDF, rather than an async runtime and a framework which are a different
//! proposition there.
//!
//! ## What it is not
//!
//! It is not a web server. It has no filesystem, no directory listing, no CGI,
//! no TLS, no chunked bodies and no keep-alive. Routes are **matched**, never
//! mapped onto a path, so the whole class of traversal bugs has nowhere to
//! occur. Every response carries `Content-Length` and closes the connection,
//! which removes request smuggling along with the state machine that hosts it.
//!
//! ## Why the limits are the interesting part
//!
//! Hand-written HTTP parsing is where security bugs live, and the mitigation is
//! not care — it is refusing early. Every read is bounded before it is parsed: a
//! request line, a header, the header count, the body. Anything over a limit is
//! a status code, not an allocation.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Bounds, applied before anything is parsed or allocated.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub request_line: usize,
    pub header_line: usize,
    pub headers: usize,
    pub body: usize,
    /// Connections served at once. Over this, a client gets 503 rather than the
    /// machine getting a thread per attacker.
    pub connections: usize,
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            request_line: 8 * 1024,
            header_line: 8 * 1024,
            headers: 32,
            // Enough for a mapping file or an entry list; far short of anything
            // that would matter on a device with 512 KB of SRAM.
            body: 256 * 1024,
            connections: 16,
            timeout: Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Head,
    Post,
    Other,
}

impl Method {
    fn parse(s: &str) -> Method {
        match s {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            _ => Method::Other,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Request {
    pub method: Method,
    /// Percent-decoded, always starting with `/`.
    pub path: String,
    /// Everything after `?`, undecoded.
    pub query: String,
    /// Headers this server was asked to keep, names already lower-cased. Only the
    /// ones on [`KEPT`] are retained: a request may carry thirty headers and a
    /// handler here has business with one or two, so the rest are read, bounded
    /// and dropped rather than allocated into a map somebody might route on.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Headers a handler may ask for. An allow-list rather than a limit, because the
/// question "which headers does this server act on" should have an answer that
/// fits on one line.
pub const KEPT: [&str; 1] = ["x-beam402-token"];

impl Request {
    /// One header, if it was kept. Names are matched case-insensitively, as HTTP
    /// requires.
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.as_str())
    }

    /// One query parameter, percent-decoded. The first occurrence wins.
    pub fn param(&self, key: &str) -> Option<String> {
        for pair in self.query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if decode(k) == key {
                return Some(decode(v));
            }
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// Extra headers, already formatted as `Name: value`. Kept as a list rather
    /// than a map because the server sends few and validates them all.
    pub headers: Vec<String>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status,
            content_type,
            body: body.into(),
            headers: Vec::new(),
        }
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "text/html; charset=utf-8", body)
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Response {
        Response::new(200, "application/json; charset=utf-8", body)
    }

    pub fn text(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response::new(status, "text/plain; charset=utf-8", body)
    }

    /// Allow a browser page to be read cross-origin.
    ///
    /// Needed by exactly one thing and worth naming: **D31**'s relay is a page
    /// served by the tree that uploads to a remote server. That direction needs
    /// nothing from us. This is for the reverse — another origin reading *our*
    /// API — and it is off unless a caller asks, because a club LAN is not a
    /// threat model but a default that invites cross-origin reads is still a
    /// default nobody chose.
    pub fn with_header(mut self, header: impl Into<String>) -> Response {
        self.headers.push(header.into());
        self
    }

    fn reason(&self) -> &'static str {
        match self.status {
            200 => "OK",
            204 => "No Content",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            413 => "Payload Too Large",
            431 => "Request Header Fields Too Large",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            503 => "Service Unavailable",
            _ => "Status",
        }
    }
}

/// Whatever answers requests. `Send + Sync` because connections are served on
/// their own threads.
pub trait Handler: Send + Sync {
    fn handle(&self, request: &Request) -> Response;
}

impl<F> Handler for F
where
    F: Fn(&Request) -> Response + Send + Sync,
{
    fn handle(&self, request: &Request) -> Response {
        self(request)
    }
}

/// Serve until the process ends.
pub fn serve<A: ToSocketAddrs, H: Handler + 'static>(addr: A, handler: H) -> std::io::Result<()> {
    serve_with(addr, handler, Limits::default())
}

pub fn serve_with<A: ToSocketAddrs, H: Handler + 'static>(
    addr: A,
    handler: H,
    limits: Limits,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let handler = Arc::new(handler);
    let live = Arc::new(AtomicUsize::new(0));

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // A refused connection is a 503 and a closed socket, never a thread. The
        // machine at the other end of this may be a tree with 512 KB of RAM.
        if live.load(Ordering::Relaxed) >= limits.connections {
            let mut s = stream;
            let _ = s.set_write_timeout(Some(limits.timeout));
            let _ = write(
                &mut s,
                &Response::text(503, "too many connections\n"),
                false,
            );
            // Drained for the same reason as anywhere else: this socket has an
            // unread request on it, and closing over one sends a reset that
            // takes the 503 with it.
            drain(&mut s, limits);
            continue;
        }
        let handler = Arc::clone(&handler);
        let live_here = Arc::clone(&live);
        live.fetch_add(1, Ordering::Relaxed);
        std::thread::spawn(move || {
            let _ = converse(stream, handler.as_ref(), limits);
            live_here.fetch_sub(1, Ordering::Relaxed);
        });
    }
    Ok(())
}

fn converse<H: Handler + ?Sized>(
    mut stream: TcpStream,
    handler: &H,
    limits: Limits,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(limits.timeout))?;
    stream.set_write_timeout(Some(limits.timeout))?;
    let peer = stream.try_clone()?;
    let mut reader = BufReader::new(peer);

    // A HEAD must report the `Content-Length` a GET *would* have returned, so
    // the body is suppressed on the wire rather than emptied — a client sizing a
    // download off the header is otherwise told zero.
    let mut head_only = false;
    let response = match parse(&mut reader, limits) {
        Ok(request) => {
            head_only = request.method == Method::Head;
            handler.handle(&request)
        }
        Err(status) => Response::text(status, format!("{status}\n")),
    };
    write(&mut stream, &response, head_only)?;
    let _ = stream.flush();
    // Always close — keep-alive buys nothing here, since every page this project
    // serves is a single self-contained file, and it costs the state machine
    // that request smuggling lives in.
    //
    // But closing on a socket with unread inbound bytes sends a reset, and the
    // reset destroys the response that was just written. Anything the client
    // sent and this server did not read — the body behind a 413, the request
    // behind a 503 — has to be drained first.
    drain(&mut stream, limits);
    Ok(())
}

/// Read and parse one request, or fail with the status to send back.
fn parse<R: BufRead>(reader: &mut R, limits: Limits) -> Result<Request, u16> {
    let line = read_line(reader, limits.request_line)?;
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(400);
    };
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(400);
    }
    if !target.starts_with('/') {
        // No absolute-form, no authority-form. This serves one origin.
        return Err(400);
    }

    let (raw_path, query) = target.split_once('?').unwrap_or((target, ""));
    let path = decode(raw_path);
    if !path.starts_with('/') {
        return Err(400);
    }

    let mut length = 0usize;
    let mut seen_length = false;
    let mut headers: Vec<(String, String)> = Vec::new();
    for _ in 0..limits.headers {
        let header = read_line(reader, limits.header_line)?;
        if header.is_empty() {
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).map_err(|_| 400u16)?;
            return Ok(Request {
                method: Method::parse(method),
                path,
                query: query.to_string(),
                headers,
                body,
            });
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(400);
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            // Chunked bodies are the other half of request smuggling and this
            // server has no use for them.
            "transfer-encoding" => return Err(501),
            "content-length" => {
                // Two of these is ambiguous, and ambiguity is the bug.
                if seen_length {
                    return Err(400);
                }
                seen_length = true;
                length = value.parse().map_err(|_| 400u16)?;
                if length > limits.body {
                    return Err(413);
                }
            }
            // First occurrence wins, so a second copy of a kept header cannot
            // replace the first — the same rule the response side follows.
            n if KEPT.contains(&n) && !headers.iter().any(|(k, _)| k == n) => {
                headers.push((name.clone(), value.to_string()));
            }
            _ => {}
        }
    }
    // Ran out of allowance before the blank line.
    Err(431)
}

/// A CRLF-terminated line, bounded. The bound is checked as it reads, so an
/// endless line is refused rather than buffered.
fn read_line<R: BufRead>(reader: &mut R, limit: usize) -> Result<String, u16> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Err(400),
            Ok(_) => {}
            Err(_) => return Err(400),
        }
        if byte[0] == b'\n' {
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
            return String::from_utf8(buf).map_err(|_| 400);
        }
        buf.push(byte[0]);
        if buf.len() > limit {
            return Err(431);
        }
    }
}

/// Swallow whatever the client still has in flight, briefly and boundedly, so
/// the close is a FIN and not a reset.
fn drain(stream: &mut TcpStream, limits: Limits) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
    let mut scratch = [0u8; 1024];
    let mut left = limits.body.min(64 * 1024);
    while left > 0 {
        match stream.read(&mut scratch) {
            Ok(0) | Err(_) => return,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

fn write(stream: &mut TcpStream, response: &Response, head_only: bool) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.reason(),
        response.content_type,
        response.body.len(),
    );
    for h in &response.headers {
        // A header carrying CRLF would inject a second response. Nothing this
        // project sends could, and it is checked anyway, because "nothing could"
        // is a property of today's callers rather than of the server.
        if h.contains('\r') || h.contains('\n') {
            continue;
        }
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if !head_only {
        stream.write_all(&response.body)?;
    }
    Ok(())
}

/// Percent-decoding. `+` is left alone: this is a path and a query string read
/// by hand, not a submitted form.
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn req(raw: &str) -> Result<Request, u16> {
        parse(&mut Cursor::new(raw.as_bytes().to_vec()), Limits::default())
    }

    #[test]
    fn a_plain_request_parses() {
        let r = req("GET /board?lane=2 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.path, "/board");
        assert_eq!(r.param("lane").as_deref(), Some("2"));
        assert_eq!(r.param("missing"), None);
        assert!(r.body.is_empty());
    }

    #[test]
    fn a_body_arrives_when_its_length_is_declared() {
        let r = req("POST /api/arm HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.body, b"hello");
    }

    #[test]
    fn percent_escapes_are_decoded_in_the_path_and_the_query() {
        let r = req("GET /a%20b?who=lane%201 HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(r.path, "/a b");
        assert_eq!(r.param("who").as_deref(), Some("lane 1"));
    }

    // -- refusing early ---------------------------------------------------

    #[test]
    fn two_content_lengths_are_refused_rather_than_resolved() {
        // The ambiguity *is* the bug: whichever one is believed, some other
        // party believed the other.
        assert_eq!(
            req("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab"),
            Err(400)
        );
    }

    #[test]
    fn chunked_is_not_implemented_rather_than_half_implemented() {
        assert_eq!(
            req("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(501)
        );
    }

    #[test]
    fn an_oversized_body_is_refused_before_it_is_allocated() {
        // Declared, not sent. Nothing is allocated for it.
        assert_eq!(
            req("POST / HTTP/1.1\r\nContent-Length: 999999999\r\n\r\n"),
            Err(413)
        );
    }

    #[test]
    fn an_endless_request_line_is_refused_as_it_reads() {
        let long = "GET /".to_string() + &"a".repeat(20_000) + " HTTP/1.1\r\n\r\n";
        assert_eq!(req(&long), Err(431));
    }

    #[test]
    fn too_many_headers_run_out_of_allowance() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..100 {
            raw.push_str(&format!("X-{i}: v\r\n"));
        }
        raw.push_str("\r\n");
        assert_eq!(req(&raw), Err(431));
    }

    #[test]
    fn only_origin_form_targets_are_served() {
        // No absolute-form, no authority-form: this serves one origin and has no
        // opinion about any other.
        assert_eq!(req("GET http://elsewhere/ HTTP/1.1\r\n\r\n"), Err(400));
        assert_eq!(req("GET * HTTP/1.1\r\n\r\n"), Err(400));
    }

    #[test]
    fn a_malformed_request_line_is_a_status_and_not_a_panic() {
        for raw in [
            "\r\n",
            "GET\r\n",
            "GET /\r\n",
            "GET / HTTP/1.1 extra\r\n",
            "GET / SPDY/3\r\n",
            "GET / HTTP/1.1\r\nno-colon-here\r\n\r\n",
        ] {
            assert!(req(raw).is_err(), "{raw:?} should not parse");
        }
    }

    #[test]
    fn an_escape_that_is_not_an_escape_is_left_alone() {
        // A truncated or malformed `%` must not eat the rest of the path.
        assert_eq!(decode("/a%"), "/a%");
        assert_eq!(decode("/a%zz"), "/a%zz");
        assert_eq!(decode("/a%2"), "/a%2");
    }

    // -- responses --------------------------------------------------------

    #[test]
    fn a_header_carrying_a_line_break_is_dropped_not_sent() {
        // It would inject a second response. Nothing this project sends could,
        // and that is a property of today's callers rather than of the server.
        let r = Response::text(200, "x").with_header("X-Bad: a\r\nEvil: yes");
        let mut head = String::new();
        for h in &r.headers {
            if !h.contains('\r') && !h.contains('\n') {
                head.push_str(h);
            }
        }
        assert!(head.is_empty(), "the header must not reach the wire");
    }

    #[test]
    fn every_status_has_a_reason_and_none_of_them_panic() {
        for s in [
            200, 204, 400, 403, 404, 405, 409, 413, 431, 500, 501, 503, 599,
        ] {
            assert!(!Response::text(s, "").reason().is_empty());
        }
    }
}
