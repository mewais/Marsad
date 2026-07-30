//! Incremental SSE parser for the real `RealWaslSessionClient` observe stream
//! (`GET /v1/targets/{id}/events`; design §3, §4.2). Feed lines, get frames at
//! blank-line dispatch. Dependency-free (no sockets involved) and hand-rolled in
//! this crate — a FRESH copy of the discipline in `marsad-wasl/src/sse.rs`, NOT
//! an import (that crate's SSE primitives are `pub(crate)`; §3, §5 of the task).
//!
//! Always compiled; the `real_*.rs` tests drive it through `attach`. This crate's
//! own tests exercise every line of this parser (hello with no `id:`, `id:`
//! parsing, comment skip, unknown-field skip, blank-line dispatch), so no
//! `cfg(coverage)` residual is needed here.

/// One parsed SSE frame: `event:` type, optional `id:` (absent on `hello` /
/// `truncated`), and the raw `data:` payload (multiple `data:` lines join with
/// `'\n'`, per the SSE spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseFrame {
    pub event: String,
    pub id: Option<u64>,
    pub data: String,
}

/// Incremental SSE parser. `event: X` sets the frame type; `data: Y` appends the
/// payload; `id: N` parses as `u64` (unparsable → `None`). Comment lines (leading
/// `':'`) and unknown field names are ignored. A dispatched frame with no
/// `event:` is dropped (`push_line` returns `None`).
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    event: Option<String>,
    id: Option<u64>,
    data_lines: Vec<String>,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feed one raw line (no trailing `'\n'`). Returns `Some(frame)` when the
    /// line was the blank line that dispatches a complete (`event:`-bearing)
    /// frame.
    pub(crate) fn push_line(&mut self, line: &str) -> Option<SseFrame> {
        if line.is_empty() {
            let event = self.event.take();
            let id = self.id.take();
            let data = self.data_lines.join("\n");
            self.data_lines.clear();
            return event.map(|event| SseFrame { event, id, data });
        }
        if line.starts_with(':') {
            return None;
        }
        if let Some(rest) = line.strip_prefix("event:") {
            self.event = Some(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data_lines.push(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("id:") {
            self.id = rest.trim().parse::<u64>().ok();
        }
        // Any other field name Marsad doesn't model (e.g. `retry:`) falls through
        // and is silently ignored — SSE's tolerant-skip. Exercised by the coverage-
        // completion suite `tests/wire_translation_completeness.rs` (the frozen
        // `real_attach.rs` fixtures emit only event:/id:/data:/comment/blank lines).
        None
    }
}
