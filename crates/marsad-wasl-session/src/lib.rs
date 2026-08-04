//! `marsad-wasl-session` — caller-side Wasl **session-control** client (X6,
//! crate bucket L1).
//!
//! Authoritative design: `docs/design/wasl-backend-plan.md` (v3). This crate is
//! Marsad's thin-client view of a **Wasl-owned** live session (Wasl's "Model A"
//! managed runtime): Marsad prompts it, observes it, resolves its native gates,
//! interrupts, and stops it — it never owns the process. It is the structural
//! analogue of X1 `marsad-wasl` (operator gates) for the *control* surface, and
//! is **distinct** from it.
//!
//! The public surface:
//! * [`WaslSessionClient`] — caller-side mirror of Wasl's `DeliveryTarget` (§4.2).
//! * [`WaslSessionProvider`] — the construct/reattach factory seam (§4.3), with
//!   the idempotent [`WaslSessionProvider::get_or_start`].
//! * [`WaslEventStream`] — the pull-style observe stream.
//! * The caller-mirror domain types (§4.4): [`WaslCapabilities`],
//!   [`WaslTargetStatus`], [`WaslTurnReceipt`], [`WaslBusyPolicy`],
//!   [`WaslTurnRequest`], [`WaslNativeGateDetail`], [`WaslSessionEvent`], …
//! * The pure helpers: [`project_status_to_phase`] / [`status_is_live`] (§6.3),
//!   [`native_gate_key`] / [`native_gate_command`] (§8), and
//!   [`new_idempotency_key`] (§12 Q5).
//! * [`FakeWaslSessionClient`] / [`FakeWaslSessionProvider`] — the in-memory test
//!   double the whole phase runs against (§5).
//! * [`WaslSessionRuntimesConfig`] / [`WaslSessionRuntimeEntry`] — the agent's
//!   production `wasl.json` `session_runtimes` config shape (§6.1/§7), the
//!   structural analogue of X1 `marsad_wasl::WaslOperatorsConfig`.
//! * [`RealWaslSessionClient`] / [`RealWaslSessionProvider`] /
//!   [`WaslSessionClientConfig`] — the real, hand-rolled HTTP/1.1 + SSE client
//!   against a live Wasl runtime-control node (§3, §11). It hand-rolls a FRESH
//!   HTTP/SSE stack in this crate (mirroring `marsad-wasl`'s discipline, NOT
//!   importing it — those primitives are `pub(crate)`) and translates every public
//!   type to/from its real internally-tagged snake_case wire shape via a private
//!   DTO layer. Its genuinely-untestable IO error branches carry `cfg(coverage)`
//!   residuals so the crate holds 100% line+region; see `AGENTS.md`.
//!
//! It depends on no other Marsad crate except `marsad-events` (for the shared
//! `Phase` the status projection returns, §6.3 / Q3) — plus `serde`,
//! `serde_json`, `thiserror`. It never depends on a Wasl crate: the fake is the
//! contract's executable form until Wasl ships fixtures (§3).

mod client;
mod config;
mod error;
mod fake;
mod projection;
mod types;

// The real, hand-rolled HTTP/1.1 + SSE client and its wire primitives (§3, §11).
// Always compiled (the frozen `real_*.rs` tests reference its symbols
// unconditionally); the genuinely-untestable IO error/retry branches that this
// crate's own tests structurally cannot drive get per-site `cfg(coverage)`
// residuals so the crate holds 100% line+region under `cargo llvm-cov`, exactly
// the `marsad-wasl` real-client regime. See `AGENTS.md`.
mod real;
mod sse;
mod wire;
mod wire_dto;

pub use client::{WaslEventStream, WaslSessionClient, WaslSessionProvider};
pub use config::{WaslSessionRuntimeEntry, WaslSessionRuntimesConfig};
pub use error::WaslSessionError;
pub use fake::{FakeWaslSessionClient, FakeWaslSessionProvider};
pub use projection::{
    native_gate_command, native_gate_key, new_idempotency_key, project_status_to_phase,
    status_is_live, wasl_a2a_bind,
};
pub use real::{RealWaslSessionClient, RealWaslSessionProvider, WaslSessionClientConfig};
pub use types::{
    // Rich-event payload types (re-exported from marsad_proto via types.rs so
    // callers don't need a separate marsad-proto dependency to pattern-match).
    CompactionInfo, PlanEntry, SessionMode, SlashCommandInfo,
    // Core session vocabulary.
    WaslAgentCard, WaslBusyPolicy, WaslCancelReason, WaslCapabilities, WaslCausation, WaslEventSeq,
    WaslGateDecision, WaslNativeGateDetail, WaslSessionEvent, WaslSessionEventKind,
    WaslShutdownMode, WaslStreamItem, WaslTargetKey, WaslTargetSnapshot, WaslTargetStatus,
    WaslTurnId, WaslTurnReceipt, WaslTurnRequest,
};
