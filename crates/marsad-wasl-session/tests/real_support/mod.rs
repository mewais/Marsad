//! real_support/mod.rs — loopback HTTP/1.1 mock server + vendored-fixture
//! helpers for the `RealWaslSessionClient`/`RealWaslSessionProvider` black-box
//! suite (design `docs/design/wasl-backend-plan.md` §3, §11: the real,
//! hand-rolled HTTP client is the "separate, later phase" these tests specify).
//!
//! This is the caller-side analogue of `marsad-wasl/tests/support/mod.rs`'s
//! `MockServer` — a real `std::net::TcpListener` on `127.0.0.1:0` speaking just
//! enough hand-rolled HTTP/1.1 to stand in for a Wasl runtime-control node
//! (`Wasl/crates/wasl-runtime-control/src/lib.rs` routes):
//!
//!   GET  /v1/info
//!   GET  /v1/targets
//!   POST /v1/targets
//!   GET  /v1/targets/{id}
//!   GET  /v1/targets/{id}/events   (SSE: hello + turn_event + truncated)
//!   POST /v1/targets/{id}/turns
//!   POST /v1/targets/{id}/cancel
//!   POST /v1/targets/{id}/gate
//!   POST /v1/targets/{id}/shutdown
//!
//! It is scripted (each endpoint has a FIFO of canned responses) and records
//! every request so tests can assert the client sent the right method / path /
//! body. Canned response bodies are built from the VENDORED FIXTURES
//! (`../../fixtures/runtime-control-v1/`) — NOT hand-typed JSON — so drift
//! against the real Wasl wire contract is caught here, exactly as
//! `marsad-wasl`'s harness loads `fixtures/operator-v1/`.
//!
//! Kept intentionally separate from the frozen fake-only `tests/support/mod.rs`
//! (which has no wire at all): this file is only pulled in by the NEW
//! `tests/real_*.rs` files via `mod real_support;`.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Fixture helpers — resolve/read/compact the vendored runtime-control-v1 files.
// ---------------------------------------------------------------------------

/// Path to a file under the vendored `fixtures/runtime-control-v1/` directory,
/// resolved relative to `CARGO_MANIFEST_DIR` (this crate lives at
/// `crates/marsad-wasl-session`, so the fixtures dir is two levels up). Mirrors
/// `marsad-wasl/tests/support::fixture_path`.
pub fn fixture_path(filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/runtime-control-v1")
        .join(filename)
}

/// Read a vendored fixture file's raw (pretty-printed) text.
pub fn read_fixture(filename: &str) -> String {
    let path = fixture_path(filename);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"))
}

/// Read a vendored fixture and re-serialize it compactly (single line, no
/// whitespace) — real Wasl SSE `data:` payloads and HTTP bodies are compact,
/// while the vendored copies are pretty-printed for readability.
pub fn compact_fixture(filename: &str) -> String {
    let raw = read_fixture(filename);
    let v: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{filename} must be valid JSON: {e}"));
    serde_json::to_string(&v).expect("re-serialize fixture")
}

/// The parsed JSON `Value` of a vendored fixture, for order-insensitive body
/// comparisons (key order is irrelevant per the wire contract).
pub fn fixture_value(filename: &str) -> serde_json::Value {
    serde_json::from_str(&read_fixture(filename))
        .unwrap_or_else(|e| panic!("{filename} must be valid JSON: {e}"))
}

/// Parse a JSON literal into a `Value`.
pub fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("must be valid JSON")
}

/// Compact an arbitrary JSON literal (single line, no whitespace).
pub fn compact_json(raw: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(raw).expect("literal must be valid JSON");
    serde_json::to_string(&v).expect("re-serialize literal")
}

// ---------------------------------------------------------------------------
// SSE line builders — compose a raw `/v1/targets/{id}/events` byte stream.
// ---------------------------------------------------------------------------

pub fn sse_event_line(event: &str) -> String {
    format!("event: {event}\n")
}
pub fn sse_id_line(id: u64) -> String {
    format!("id: {id}\n")
}
pub fn sse_data_line(data: &str) -> String {
    format!("data: {data}\n")
}
pub fn sse_comment_line(text: &str) -> String {
    format!(": {text}\n")
}
pub const SSE_BLANK: &str = "\n";

/// One full SSE record: `event:`, optional `id:`, `data:`, then the blank line
/// that dispatches it. `hello`/`truncated` carry no `id:` per the wire spec;
/// `turn_event` frames do.
pub fn sse_frame(event: &str, id: Option<u64>, data: &str) -> Vec<String> {
    let mut lines = vec![sse_event_line(event)];
    if let Some(id) = id {
        lines.push(sse_id_line(id));
    }
    lines.push(sse_data_line(data));
    lines.push(SSE_BLANK.to_string());
    lines
}

// ---------------------------------------------------------------------------
// Recorded request — kept for test-side assertions on what the client sent.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The request body parsed as JSON (panics if it isn't) — for asserting the
    /// client sent a `create_request`/`submit_request`/`gate_request`/… shaped
    /// body.
    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("recorded body must be JSON: {e}\n{:?}", self.body_str()))
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

// ---------------------------------------------------------------------------
// Scripted responses.
// ---------------------------------------------------------------------------

/// A scripted buffered (non-SSE) response for a JSON endpoint.
#[derive(Debug, Clone)]
pub struct BufferedResponse {
    pub status: u16,
    pub body: String,
    pub chunked: bool,
    /// If true: accept the connection, read the request, then never write a byte
    /// back and never close — models a wedged/dead node that still holds the TCP
    /// accept open. Proves the CLIENT-side read deadline (not a server close) is
    /// what bounds a blocking call. Mirrors `marsad-wasl`'s `BufferedResponse::hang`.
    pub hang: bool,
}

impl BufferedResponse {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            chunked: false,
            hang: false,
        }
    }
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
            chunked: false,
            hang: false,
        }
    }
    pub fn chunked_json(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            chunked: true,
            hang: false,
        }
    }
    /// A peer that accepts, reads the request, then never responds.
    pub fn hang() -> Self {
        Self {
            status: 0,
            body: String::new(),
            chunked: false,
            hang: true,
        }
    }
}

/// A scripted response for one `GET /v1/targets/{id}/events` (SSE) connection.
#[derive(Debug, Clone, Default)]
pub struct EventsScript {
    /// Raw lines to write in order (each already newline-terminated: build with
    /// `sse_frame`/`sse_comment_line`/`SSE_BLANK`).
    pub lines: Vec<String>,
    /// If true, the server drops the TCP connection abruptly after writing
    /// `lines` (models a node crash / network drop mid-stream). If false, the
    /// connection is held open (bounded by an internal timeout).
    pub drop_after: bool,
    /// If true (typical, matching a real chunked SSE response), frames are
    /// wrapped in HTTP/1.1 chunked-transfer framing; if false, raw bytes relying
    /// on `Connection: close`/`drop_after` to terminate.
    pub chunked: bool,
}

impl EventsScript {
    pub fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            drop_after: false,
            chunked: true,
        }
    }
    pub fn drop_after(mut self, v: bool) -> Self {
        self.drop_after = v;
        self
    }
    pub fn chunked(mut self, v: bool) -> Self {
        self.chunked = v;
        self
    }
}

#[derive(Default)]
struct ServerState {
    info_responses: VecDeque<BufferedResponse>,
    list_responses: VecDeque<BufferedResponse>,
    create_responses: VecDeque<BufferedResponse>,
    target_responses: VecDeque<BufferedResponse>,
    turns_responses: VecDeque<BufferedResponse>,
    cancel_responses: VecDeque<BufferedResponse>,
    gate_responses: VecDeque<BufferedResponse>,
    shutdown_responses: VecDeque<BufferedResponse>,
    events_scripts: VecDeque<EventsScript>,
    requests: Vec<RecordedRequest>,
    events_connections: usize,
}

/// A scripted HTTP/1.1 responder over a real loopback TCP socket — the caller
/// side's loopback-real tier. The real `RealWaslSessionClient` HTTP code path
/// runs end to end against it.
pub struct MockRuntimeServer {
    pub addr: SocketAddr,
    state: Arc<Mutex<ServerState>>,
    stop: Arc<AtomicBool>,
}

impl MockRuntimeServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        listener.set_nonblocking(true).expect("set_nonblocking");
        let addr = listener.local_addr().expect("local_addr");
        let state = Arc::new(Mutex::new(ServerState::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let state2 = state.clone();
        let stop2 = stop.clone();
        thread::spawn(move || loop {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let st = state2.clone();
                    thread::spawn(move || handle_connection(stream, st));
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        });

        Self { addr, state, stop }
    }

    /// `http://ip:port` — the form the real client's config takes (§3: v1 has no
    /// TLS/auth/hostnames; the node binds a loopback/trusted interface).
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn push_info(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().info_responses.push_back(resp);
    }
    pub fn push_list(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().list_responses.push_back(resp);
    }
    pub fn push_create(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().create_responses.push_back(resp);
    }
    pub fn push_target(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().target_responses.push_back(resp);
    }
    pub fn push_turns(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().turns_responses.push_back(resp);
    }
    pub fn push_cancel(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().cancel_responses.push_back(resp);
    }
    pub fn push_gate(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().gate_responses.push_back(resp);
    }
    pub fn push_shutdown(&self, resp: BufferedResponse) {
        self.state
            .lock()
            .unwrap()
            .shutdown_responses
            .push_back(resp);
    }
    pub fn push_events(&self, script: EventsScript) {
        self.state.lock().unwrap().events_scripts.push_back(script);
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && r.path == path)
            .collect()
    }
    /// All requests whose path starts with `prefix` (query-string tolerant, and
    /// useful for the per-target `.../events?after_seq=N` route).
    pub fn requests_with_path_prefix(&self, method: &str, prefix: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && r.path.starts_with(prefix))
            .collect()
    }
    pub fn events_connection_count(&self) -> usize {
        self.state.lock().unwrap().events_connections
    }
}

impl Drop for MockRuntimeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// An address nothing is listening on (bind then immediately drop the listener),
/// for "connection refused" tests.
pub fn unused_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind throwaway listener");
    listener.local_addr().expect("local_addr")
}

/// A `base_url` (`http://ip:port`) pointing at a port nothing listens on.
pub fn unused_base_url() -> String {
    format!("http://{}", unused_addr())
}

// ---------------------------------------------------------------------------
// Wire-level plumbing.
// ---------------------------------------------------------------------------

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

type ParsedRequest = (String, String, Vec<(String, String)>, Vec<u8>);

fn read_request(stream: &TcpStream) -> Option<ParsedRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok()?;
    }

    Some((method, path, headers, body))
}

fn write_chunk(stream: &mut TcpStream, data: &[u8]) {
    let _ = write!(stream, "{:x}\r\n", data.len());
    let _ = stream.write_all(data);
    let _ = stream.write_all(b"\r\n");
}

fn write_buffered_response(stream: &mut TcpStream, resp: &BufferedResponse) {
    if resp.hang {
        // Never write, never close: park this handler thread for the rest of the
        // test process — harmless (a background thread the test binary drops on
        // exit) and genuinely distinct from a server-side close/EOF.
        loop {
            thread::sleep(Duration::from_secs(3600));
        }
    }
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\n",
        resp.status,
        status_text(resp.status)
    );
    if resp.chunked {
        headers.push_str("Transfer-Encoding: chunked\r\n");
    } else {
        headers.push_str(&format!("Content-Length: {}\r\n", resp.body.len()));
    }
    headers.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(headers.as_bytes());

    if resp.chunked {
        if !resp.body.is_empty() {
            write_chunk(stream, resp.body.as_bytes());
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    } else {
        let _ = stream.write_all(resp.body.as_bytes());
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn write_events_response(stream: &mut TcpStream, script: &EventsScript) {
    let mut headers = String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n");
    if script.chunked {
        headers.push_str("Transfer-Encoding: chunked\r\n");
    }
    headers.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(headers.as_bytes());

    for line in &script.lines {
        if script.chunked {
            write_chunk(stream, line.as_bytes());
        } else {
            let _ = stream.write_all(line.as_bytes());
        }
    }

    if script.drop_after {
        let _ = stream.shutdown(std::net::Shutdown::Both);
        return;
    }

    // Hold the connection open (a live SSE stream never ends on its own in v1)
    // so the client's read-deadline / stop discipline can be observed without a
    // forced disconnect. Bounded so the handler thread can't outlive the test.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = [0u8; 256];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
}

/// The `{id}` segment of `/v1/targets/{id}[/sub]`, if `path` is a per-target
/// route. Query strings (`?after_seq=N`) are stripped first.
fn per_target_suffix(path: &str) -> Option<(&str, &str)> {
    let path = path.split('?').next().unwrap_or(path);
    let rest = path.strip_prefix("/v1/targets/")?;
    // rest is `{id}` or `{id}/sub`
    match rest.split_once('/') {
        Some((id, sub)) => Some((id, sub)),
        None => Some((rest, "")),
    }
}

fn pop_or(q: &mut VecDeque<BufferedResponse>, default: BufferedResponse) -> BufferedResponse {
    q.pop_front().unwrap_or(default)
}

fn handle_connection(mut stream: TcpStream, state: Arc<Mutex<ServerState>>) {
    let Some((method, path, headers, body)) = read_request(&stream) else {
        return;
    };
    {
        let mut st = state.lock().unwrap();
        st.requests.push(RecordedRequest {
            method: method.clone(),
            path: path.clone(),
            headers: headers.clone(),
            body: body.clone(),
        });
    }

    let bare = path.split('?').next().unwrap_or(&path);

    // Collection routes.
    if method == "GET" && bare == "/v1/info" {
        let resp = {
            let mut st = state.lock().unwrap();
            pop_or(&mut st.info_responses, BufferedResponse::json(200, "{}"))
        };
        write_buffered_response(&mut stream, &resp);
        return;
    }
    if method == "GET" && bare == "/v1/targets" {
        let resp = {
            let mut st = state.lock().unwrap();
            pop_or(
                &mut st.list_responses,
                BufferedResponse::json(200, r#"{"targets":[]}"#),
            )
        };
        write_buffered_response(&mut stream, &resp);
        return;
    }
    if method == "POST" && bare == "/v1/targets" {
        let resp = {
            let mut st = state.lock().unwrap();
            pop_or(&mut st.create_responses, BufferedResponse::json(200, "{}"))
        };
        write_buffered_response(&mut stream, &resp);
        return;
    }

    // Per-target routes.
    if let Some((_id, sub)) = per_target_suffix(bare) {
        match (method.as_str(), sub) {
            ("GET", "") => {
                let resp = {
                    let mut st = state.lock().unwrap();
                    pop_or(&mut st.target_responses, BufferedResponse::json(200, "{}"))
                };
                write_buffered_response(&mut stream, &resp);
                return;
            }
            ("GET", "events") => {
                let script = {
                    let mut st = state.lock().unwrap();
                    st.events_connections += 1;
                    st.events_scripts.pop_front()
                }
                .unwrap_or(EventsScript {
                    lines: vec![],
                    drop_after: true,
                    chunked: true,
                });
                write_events_response(&mut stream, &script);
                return;
            }
            ("POST", "turns") => {
                let resp = {
                    let mut st = state.lock().unwrap();
                    pop_or(&mut st.turns_responses, BufferedResponse::json(200, "{}"))
                };
                write_buffered_response(&mut stream, &resp);
                return;
            }
            ("POST", "cancel") => {
                let resp = {
                    let mut st = state.lock().unwrap();
                    pop_or(&mut st.cancel_responses, BufferedResponse::empty(204))
                };
                write_buffered_response(&mut stream, &resp);
                return;
            }
            ("POST", "gate") => {
                let resp = {
                    let mut st = state.lock().unwrap();
                    pop_or(&mut st.gate_responses, BufferedResponse::empty(204))
                };
                write_buffered_response(&mut stream, &resp);
                return;
            }
            ("POST", "shutdown") => {
                let resp = {
                    let mut st = state.lock().unwrap();
                    pop_or(&mut st.shutdown_responses, BufferedResponse::empty(204))
                };
                write_buffered_response(&mut stream, &resp);
                return;
            }
            _ => {}
        }
    }

    write_buffered_response(
        &mut stream,
        &BufferedResponse::json(404, r#"{"error":"unknown_target","detail":"no route"}"#),
    );
}
