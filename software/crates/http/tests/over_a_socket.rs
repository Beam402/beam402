//! The server against a real socket.
//!
//! Unit tests over a `Cursor` prove the parser; they do not prove that the
//! thing binds, answers, closes, and refuses what it should. **D32** puts this
//! server in the binary that a club's LAN can reach, so the tests that matter
//! are the ones a client can actually perform.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use beam402_http::{serve_with, Limits, Method, Request, Response};

/// Start a server on a free port and give back its address.
fn start(handler: impl Fn(&Request) -> Response + Send + Sync + 'static, limits: Limits) -> String {
    // Bind first to learn a free port, then drop it and let the server take it.
    // A fixed port would make these tests fight each other and any developer
    // who happens to be running the real thing.
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let listen = addr.to_string();
    let serving = listen.clone();
    std::thread::spawn(move || {
        let _ = serve_with(serving.as_str(), handler, limits);
    });
    for _ in 0..200 {
        if TcpStream::connect(&listen).is_ok() {
            return listen;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the server never came up on {listen}");
}

fn ask(addr: &str, raw: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = String::new();
    // The server closes after every response, so a read to EOF is the whole of
    // it — which is also the assertion that it *does* close.
    s.read_to_string(&mut out).unwrap();
    out
}

fn routes(r: &Request) -> Response {
    match (r.method, r.path.as_str()) {
        (Method::Get | Method::Head, "/") => Response::html("<h1>beam402</h1>"),
        (Method::Get, "/api/round") => Response::json(r#"{"et":12.34}"#),
        (Method::Post, "/api/arm") => {
            Response::text(200, format!("armed with {} bytes", r.body.len()))
        }
        // A route that exists under another method is 405, not 404 — the
        // difference is the whole of what a client can do about it.
        (_, "/api/arm") | (_, "/api/round") | (_, "/") => {
            Response::text(405, "method not allowed\n")
        }
        _ => Response::text(404, "no such thing\n"),
    }
}

#[test]
fn it_serves_a_page_and_closes() {
    let addr = start(routes, Limits::default());
    let out = ask(&addr, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(out.starts_with("HTTP/1.1 200 OK\r\n"), "{out}");
    assert!(
        out.contains("Content-Type: text/html; charset=utf-8"),
        "{out}"
    );
    assert!(out.contains("Content-Length: 16"), "{out}");
    assert!(out.contains("Connection: close"), "{out}");
    assert!(out.ends_with("<h1>beam402</h1>"), "{out}");
}

#[test]
fn a_head_request_gets_the_headers_and_no_body() {
    // Worth pinning: the length must still describe the body that a GET would
    // have returned, or a client sizing a download is misled.
    let addr = start(routes, Limits::default());
    let out = ask(&addr, "HEAD / HTTP/1.1\r\n\r\n");
    assert!(out.contains("Content-Length: 16"), "{out}");
    assert!(!out.contains("<h1>"), "{out}");
}

#[test]
fn a_post_carries_its_body_through() {
    let addr = start(routes, Limits::default());
    let out = ask(
        &addr,
        "POST /api/arm HTTP/1.1\r\nContent-Length: 4\r\n\r\ngogo",
    );
    assert!(out.contains("armed with 4 bytes"), "{out}");
}

#[test]
fn an_unknown_route_is_a_404_and_a_wrong_method_is_a_405() {
    let addr = start(routes, Limits::default());
    assert!(ask(&addr, "GET /nope HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
    assert!(ask(&addr, "GET /api/arm HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 405"));
}

#[test]
fn a_request_that_never_ends_is_refused_rather_than_buffered() {
    // The shape of the attack this bounds: one socket, no newline, forever.
    let addr = start(routes, Limits::default());
    let mut s = TcpStream::connect(&addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /").unwrap();
    s.write_all(&vec![b'a'; 20_000]).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 431"), "{out}");
}

#[test]
fn a_handler_that_panics_does_not_take_the_server_with_it() {
    // A panic in one request must not end the event. The connection dies, the
    // listener does not, and the next client is served.
    let addr = start(
        |r: &Request| {
            if r.path == "/boom" {
                panic!("deliberate");
            }
            Response::text(200, "fine")
        },
        Limits::default(),
    );
    let mut s = TcpStream::connect(&addr).unwrap();
    s.write_all(b"GET /boom HTTP/1.1\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);

    assert!(
        ask(&addr, "GET /ok HTTP/1.1\r\n\r\n").contains("fine"),
        "the server survived the panic"
    );
}

#[test]
fn the_connection_limit_answers_rather_than_spawning() {
    // Over the limit the reply is a status and a closed socket, not a thread —
    // the machine at the other end of this may be a tree with 512 KB of SRAM.
    let limits = Limits {
        connections: 1,
        timeout: Duration::from_secs(5),
        ..Limits::default()
    };
    let addr = start(
        |_: &Request| {
            std::thread::sleep(Duration::from_millis(400));
            Response::text(200, "slow")
        },
        limits,
    );

    let mut held = TcpStream::connect(&addr).unwrap();
    held.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let out = ask(&addr, "GET / HTTP/1.1\r\n\r\n");
    assert!(out.starts_with("HTTP/1.1 503"), "{out}");

    let mut first = String::new();
    let _ = held.read_to_string(&mut first);
    assert!(first.contains("slow"), "and the held one still finished");
}
