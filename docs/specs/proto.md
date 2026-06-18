# B01 — `marsad-proto`

**Tier:** L0 (pure data/logic). **Mode:** one-shot (de)serialization. **Coverage:** 100% line+region.

## Purpose

Wire types and serde (de)serialization for the two protocols Marsad speaks:

1. **Claude Code stream-json** — the NDJSON the `claude` CLI emits/consumes under
   `--input-format stream-json --output-format stream-json --verbose`, plus the control protocol
   that surfaces `can_use_tool` permission requests.
2. **Marsad internal IPC** — the GUI⇄`marsad-agent` protocol (one NDJSON line per message).

This crate is the bottom of the dependency DAG. It does pure parsing/encoding + validation; it holds
no state and performs no IO.

## Inputs / Outputs

- In: `&str` NDJSON lines (one JSON object per line); Rust value structs for encoding.
- Out: typed `StreamMsg` / `IpcMsg` enums; `String` JSON lines.

## Public API (authoritative)

```rust
pub enum StreamMsg {
    SystemInit(SystemInit),       // { type:"system", subtype:"init", session_id, model, cwd, tools, mcp_servers, permissionMode }
    Assistant(AssistantMsg),      // { type:"assistant", message:{ id, content:[ContentBlock], usage } }
    User(UserMsg),                // { type:"user", message:{ content:[ContentBlock] } }
    Result(ResultMsg),            // { type:"result", subtype, total_cost_usd, usage, num_turns, is_error, permission_denials }
    ControlRequest(ControlRequest),   // control protocol: can_use_tool request { tool_name, input, request_id }
    ControlResponse(ControlResponse), // control protocol: allow/deny response
    Unknown(serde_json::Value),   // forward-compat: any well-formed JSON line we don't model yet
}

pub enum ContentBlock { Text { text: String }, ToolUse { id: String, name: String, input: serde_json::Value }, ToolResult { tool_use_id: String, content: serde_json::Value, is_error: bool } }

pub struct Usage { pub input_tokens: u64, pub output_tokens: u64, pub cache_read_input_tokens: u64, pub cache_creation_input_tokens: u64 }

pub enum ResultSubtype { Success, ErrorMaxTurns, ErrorDuringExecution }

pub fn parse_stream_line(line: &str) -> Result<StreamMsg, ProtoError>;
pub fn encode_user_input(input: &UserInput) -> String;          // encodes a stream-json user turn
pub fn encode_control_response(resp: &ControlResponse) -> String;

pub enum IpcMsg { /* GUI<->agent: LaunchSession, ResumeSession, AttachSession, StopSession, SendPrompt, AnswerGate, Subscribe, VmUpdate, AgentEvent, ... */ }
pub fn parse_ipc(line: &str) -> Result<IpcMsg, ProtoError>;
pub fn encode_ipc(msg: &IpcMsg) -> String;

pub enum ProtoError { MalformedJson(String), MissingDiscriminant, UnknownDiscriminant(String), SchemaMismatch { field: &'static str, detail: String } }
```

All public types derive `serde::{Serialize, Deserialize}`, `Debug`, `Clone`, `PartialEq`.

## Behavior notes

- `Unknown` is the forward-compat escape hatch: a syntactically valid JSON object with an
  unrecognized `type` parses to `Unknown`, **not** an error. A *missing* `type` is
  `MissingDiscriminant`. Non-JSON is `MalformedJson`.
- Round-trip: `parse_stream_line` then re-encode must preserve semantically meaningful fields
  (test as value equality on the typed struct, not byte equality).
- `Usage` fields default to 0 when absent.

## Test buckets (`tests/`)

- `happy.rs` — one parse test per `StreamMsg` and `IpcMsg` variant; encode tests for user input +
  control response + every IPC message.
- `errors.rs` — `MalformedJson` (truncated/invalid JSON), `MissingDiscriminant`, `UnknownDiscriminant`
  for stream + IPC, `SchemaMismatch` (wrong field type).
- `branches.rs` — `Unknown` passthrough; absent-usage defaults; each `ResultSubtype`; each
  `ContentBlock` variant incl. `is_error` tool results; empty content arrays.
- `fixtures.rs` — parse every line of `fixtures/streams/*.ndjson` (recorded real run) with zero
  errors and assert the expected variant counts; parse `fixtures/hooks/*.json`.

## Edit scope

Code-agent: write `crates/marsad-proto/src/**` + `Cargo.toml`; read-only `tests/`, this spec.
