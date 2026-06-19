//! Integration scenario i07_core_facade — see ./AGENTS.md.
//!
//! Live scenario: N≥3 concurrent real `claude` sessions in tmux + FIFO pairs,
//! driven via stream-json, asserting independent advance and no cross-routing.
//!
//! Tests live in `tests/concurrency.rs`.
