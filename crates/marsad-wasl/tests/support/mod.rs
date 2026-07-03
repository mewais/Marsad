//! tests/support/mod.rs — shared black-box test harness for `marsad-wasl`.
//!
//! Not a cargo test binary itself (subdirectory files aren't auto-registered
//! as integration tests); pulled in via `mod support;` from the top-level
//! `tests/{happy,errors,branches}.rs` files, mirroring the house pattern of
//! self-contained `tests/*.rs` helpers (see `crates/marsad-proto/tests/fixtures.rs`
//! for the `fixture_path`-style precedent this borrows).
//!
//! Provides:
//! - `MockServer`: a real `std::net::TcpListener` on `127.0.0.1:0` speaking
//!   just enough hand-rolled HTTP/1.1 to stand in for a Wasl node's Operator
//!   API — `GET /v1/events` (SSE, chunked or raw-until-close), `GET /v1/gates`,
//!   `GET /v1/info`, `POST /v1/gates/{id}/decision`. This is the
//!   "loopback-real tier" the spec calls for (§7.1): the real
//!   `WaslOperatorClient` HTTP/SSE code path runs end to end against it.
//! - Fixture helpers: `fixture_path`/`read_fixture`/`compact_fixture` for the
//!   vendored `fixtures/operator-v1/` files, and SSE line-building helpers.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

/// Path to a file under the vendored `fixtures/operator-v1/` directory,
/// resolved relative to `CARGO_MANIFEST_DIR` (this crate lives at
/// `crates/marsad-wasl`, so the fixtures dir is two levels up).
pub fn fixture_path(filename: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/operator-v1")
        .join(filename)
}

/// Read a vendored fixture file's raw (pretty-printed) text.
pub fn read_fixture(filename: &str) -> String {
    let path = fixture_path(filename);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("could not read {path:?}: {e}"))
}

/// Read a vendored fixture and re-serialize it compactly (single line, no
/// whitespace) — real Wasl `data:` payloads are single-line in practice
/// (spec §2.2), while the vendored copies are pretty-printed for readability.
pub fn compact_fixture(filename: &str) -> String {
    let raw = read_fixture(filename);
    let v: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{filename} must be valid JSON: {e}"));
    serde_json::to_string(&v).expect("re-serialize fixture")
}

/// Compact an arbitrary JSON literal (single line, no whitespace).
pub fn compact_json(raw: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(raw).expect("literal must be valid JSON");
    serde_json::to_string(&v).expect("re-serialize literal")
}

/// Parse a JSON literal into a `Value`, for order-insensitive body comparisons.
pub fn val(s: &str) -> serde_json::Value {
    serde_json::from_str(s).expect("must be valid JSON")
}

// ---------------------------------------------------------------------------
// SSE line builders — compose a raw `/v1/events` byte stream by hand.
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

/// One full SSE record: `event:`, optional `id:`, `data:`, then the blank
/// line that dispatches it.
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
// MockServer — scripted HTTP/1.1 responder over a real loopback TCP socket.
// ---------------------------------------------------------------------------

/// One HTTP request the mock server observed, kept for test-side assertions.
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

    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("recorded body must be JSON: {e}\n{:?}", self.body))
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A scripted response for one `GET /v1/events` connection.
#[derive(Debug, Clone, Default)]
pub struct EventsScript {
    /// Raw lines to write, in order (each already newline-terminated: use
    /// `sse_frame`/`sse_comment_line`/`SSE_BLANK` to build these).
    pub lines: Vec<String>,
    /// If true, the server closes the TCP connection abruptly right after
    /// writing `lines`, WITHOUT a graceful chunk terminator — simulates a
    /// node crash / network drop mid-stream for reconnect tests. If false,
    /// the connection is held open (bounded by an internal timeout) so the
    /// client's `stop` flag can be observed without a forced disconnect.
    pub drop_after: bool,
    /// If true (the default/typical case, matching a real chunked SSE
    /// response), frames are wrapped in HTTP/1.1 chunked-transfer framing.
    /// If false, the response has neither `Transfer-Encoding` nor
    /// `Content-Length` and is written raw, relying on `Connection: close`
    /// (or `drop_after`) to terminate — exercises the "raw/Content-Length-less
    /// streaming body" path from spec §2.3.
    pub chunked: bool,
}

impl EventsScript {
    pub fn new(lines: Vec<String>) -> Self {
        EventsScript {
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

/// A scripted buffered response (`GET /v1/gates`, `GET /v1/info`,
/// `POST .../decision`).
#[derive(Debug, Clone)]
pub struct BufferedResponse {
    pub status: u16,
    pub body: String,
    pub chunked: bool,
}

impl BufferedResponse {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        BufferedResponse {
            status,
            body: body.into(),
            chunked: false,
        }
    }
    pub fn empty(status: u16) -> Self {
        BufferedResponse {
            status,
            body: String::new(),
            chunked: false,
        }
    }
    pub fn chunked_json(status: u16, body: impl Into<String>) -> Self {
        BufferedResponse {
            status,
            body: body.into(),
            chunked: true,
        }
    }
}

#[derive(Default)]
struct ServerState {
    events_scripts: VecDeque<EventsScript>,
    gates_responses: VecDeque<BufferedResponse>,
    info_responses: VecDeque<BufferedResponse>,
    decision_responses: VecDeque<BufferedResponse>,
    requests: Vec<RecordedRequest>,
    events_connections: usize,
}

pub struct MockServer {
    pub addr: SocketAddr,
    state: Arc<Mutex<ServerState>>,
    stop: Arc<AtomicBool>,
}

impl MockServer {
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

        MockServer { addr, state, stop }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn push_events_script(&self, script: EventsScript) {
        self.state.lock().unwrap().events_scripts.push_back(script);
    }
    pub fn push_gates_response(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().gates_responses.push_back(resp);
    }
    pub fn push_info_response(&self, resp: BufferedResponse) {
        self.state.lock().unwrap().info_responses.push_back(resp);
    }
    pub fn push_decision_response(&self, resp: BufferedResponse) {
        self.state
            .lock()
            .unwrap()
            .decision_responses
            .push_back(resp);
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn requests_to(&self, path: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|r| r.path == path)
            .collect()
    }
    pub fn events_connection_count(&self) -> usize {
        self.state.lock().unwrap().events_connections
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// An address nothing is listening on (bind then immediately drop the
/// listener), for "connection refused" tests.
pub fn unused_addr() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind throwaway listener");
    listener.local_addr().expect("local_addr")
}

// ---------------------------------------------------------------------------
// Wire-level plumbing
// ---------------------------------------------------------------------------

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

/// (method, path, headers, body) of one parsed HTTP request.
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

    // Hold the connection open (a live SSE stream never ends on its own in
    // v1) so the client's `stop` flag can be observed without forcing a
    // disconnect. Bounded so the handler thread doesn't outlive the test
    // process indefinitely.
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

fn write_buffered_response(stream: &mut TcpStream, resp: &BufferedResponse) {
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
        write_chunk(stream, resp.body.as_bytes());
        let _ = stream.write_all(b"0\r\n\r\n");
    } else {
        let _ = stream.write_all(resp.body.as_bytes());
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
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

    if method == "GET" && path == "/v1/events" {
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

    if method == "GET" && path == "/v1/gates" {
        let resp = {
            let mut st = state.lock().unwrap();
            st.gates_responses.pop_front()
        }
        .unwrap_or_else(|| BufferedResponse::json(200, r#"{"last_seq":0,"gates":[]}"#));
        write_buffered_response(&mut stream, &resp);
        return;
    }

    if method == "GET" && path == "/v1/info" {
        let resp = {
            let mut st = state.lock().unwrap();
            st.info_responses.pop_front()
        }
        .unwrap_or_else(|| BufferedResponse::json(200, "{}"));
        write_buffered_response(&mut stream, &resp);
        return;
    }

    if method == "POST" && path.starts_with("/v1/gates/") && path.ends_with("/decision") {
        let resp = {
            let mut st = state.lock().unwrap();
            st.decision_responses.pop_front()
        }
        .unwrap_or(BufferedResponse {
            status: 204,
            body: String::new(),
            chunked: false,
        });
        write_buffered_response(&mut stream, &resp);
        return;
    }

    write_buffered_response(
        &mut stream,
        &BufferedResponse::json(404, r#"{"error":"not_found"}"#),
    );
}
