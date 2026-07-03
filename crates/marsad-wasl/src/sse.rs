//! Incremental SSE parser (`docs/specs/wasl-connector.md` §2.2) — feed lines,
//! get frames at blank-line dispatch. Dependency-free: no sockets involved,
//! `tests/branches.rs` exercises this module directly line-by-line.

/// One parsed SSE frame: `event:` type, optional `id:` (absent on `hello`),
/// raw `data:` payload (multiple `data:` lines join with `'\n'`).
#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    pub event: String,
    pub id: Option<u64>,
    pub data: String,
}

/// Incremental SSE parser. `event: X` sets the type; `data: Y` appends the
/// payload; `id: N` parses as `u64` (unparsable → `None`). Comment lines
/// (leading `':'`) and unknown field names are ignored. A dispatched frame
/// with no `event:` is dropped (`push_line` returns `None`).
#[derive(Debug, Default)]
pub struct SseParser {
    event: Option<String>,
    id: Option<u64>,
    data_lines: Vec<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one raw line (no trailing `'\n'`). Returns `Some(frame)` when the
    /// line was the blank line that dispatches a complete (`event:`-bearing)
    /// frame.
    pub fn push_line(&mut self, line: &str) -> Option<SseFrame> {
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
        // Unknown field names (e.g. "retry: 3000") are silently ignored.
        None
    }
}
