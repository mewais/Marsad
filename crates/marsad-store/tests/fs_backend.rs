//! On-disk `FsBackend` round-trip — the real persistence path the desktop uses.
//!
//! The rest of the store suite runs over `MemoryBackend`, so this disk-backed test
//! is what guards `FsBackend`'s key↔path mapping. It specifically catches the bug
//! where keys like `host:host-1` are written into a `host/` subdirectory but
//! `restore()`'s `list("")` didn't recurse (so nothing came back on restart), and
//! the Windows variant where the path separator is `\` rather than `/`.
//!
//! `FsBackend` is `#[cfg(not(coverage))]`, so this whole file is gated to match —
//! it runs under plain `cargo test`, not under `cargo llvm-cov`.
#![cfg(not(coverage))]

use marsad_store::{FsBackend, PersistedHost, PersistedSession, Store, Workspace};

fn temp_root(tag: &str) -> std::path::PathBuf {
    // Unique-enough without Date/rand: pid + a monotonic counter.
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("marsad-fsbackend-{tag}-{}-{n}", std::process::id()))
}

#[test]
fn host_session_workspace_round_trip_across_restart() {
    let root = temp_root("rt");
    let _ = std::fs::remove_dir_all(&root);

    // ── Write through one Store (the "first run") ──────────────────────────────
    {
        let mut store = Store::new(Box::new(FsBackend::new(&root)));
        store
            .upsert_host(&PersistedHost {
                id: "host-1".to_string(),
                label: "box".to_string(),
                target: "user@box.example.com".to_string(),
                is_local: false,
                claude_bin: None,
                setup: None,
                env: Vec::new(),
            })
            .expect("persist host");
        store
            .upsert_session(&PersistedSession {
                id: "session-1".to_string(),
                session_id: None,
                name: "s1".to_string(),
                host_id: "host-1".to_string(),
                host_target: Some("user@box.example.com".to_string()),
                claude_bin: None,
                setup: None,
                env: Vec::new(),
                project: "/proj".to_string(),
                branch: "main".to_string(),
                model: "claude-haiku-4-5-20251001".to_string(),
                last_phase: "Starting".to_string(),
                posture: None,
            disallowed_tools: vec![],
            ws: None,            })
            .expect("persist session");
        store
            .snapshot_workspace(&Workspace {
                id: "ws-1".to_string(),
                name: "Work".to_string(),
                session_ids: vec!["session-1".to_string()],
                saved_at: "2026-06-24T00:00:00Z".to_string(),
            })
            .expect("persist workspace");
    }

    // Foreign files in data_root (on Windows the install dir IS the data root: app
    // binaries, agent.sock, agent.log, a stray `~` tree all land here). Restore must
    // ignore them, NOT abort — earlier, reading agent.sock errored and dropped every
    // host/session. Simulate the worst offenders.
    std::fs::write(root.join("marsad-agent.exe"), b"MZ\x00\x00binary").unwrap();
    std::fs::write(root.join("agent.log"), b"listening\n").unwrap();
    std::fs::create_dir_all(root.join("~/.marsad/sessions/marsad-session-1")).unwrap();

    // ── Restore through a fresh Store (the "second run") ───────────────────────
    let store2 = Store::new(Box::new(FsBackend::new(&root)));
    let restored = store2.restore().expect("restore from disk");

    assert_eq!(restored.hosts.len(), 1, "host must survive restart");
    assert_eq!(restored.hosts[0].target, "user@box.example.com");
    assert_eq!(restored.sessions.len(), 1, "session must survive restart");
    assert_eq!(
        restored.sessions[0].host_target.as_deref(),
        Some("user@box.example.com")
    );
    assert_eq!(restored.workspaces.len(), 1, "workspace must survive restart");

    let _ = std::fs::remove_dir_all(&root);
}
