/// errors.rs — every HostError variant reachable from the public API.
///
/// Covers:
///   - HostError::DuplicateLabel     — add a host whose label already exists
///   - HostError::CannotRemoveLocalhost — remove the localhost entry
///   - HostError::MalformedTarget    — add with a target that doesn't look like user@host or alias
///   - HostError::NotFound           — remove or health_check with an unknown id
///   - HostError::Unreachable        — health_check when SshRunner signals a hard failure
use marsad_hosts::{
    list_wsl_distros, FakeSshRunner, FakeWslRunner, HostError, HostRegistry, HostStatus,
};

// ---------------------------------------------------------------------------
// DuplicateLabel
// ---------------------------------------------------------------------------

#[test]
fn add_duplicate_label_returns_duplicate_label_error() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    reg.add(
        "server1",
        "alice@server1.example.com",
        None,
        None,
        Vec::new(),
        Vec::new(),
    )
    .expect("first add must succeed");
    let err = reg
        .add("server1", "bob@server2.example.com", None, None, Vec::new(), Vec::new())
        .expect_err("duplicate add must fail");
    match err {
        HostError::DuplicateLabel(label) => assert_eq!(label, "server1"),
        other => panic!("expected DuplicateLabel, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// CannotRemoveLocalhost
// ---------------------------------------------------------------------------

#[test]
fn remove_localhost_returns_cannot_remove_localhost() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    // localhost is always the first entry seeded; find it by is_local flag
    let local_id = reg
        .list()
        .iter()
        .find(|h| h.is_local)
        .map(|h| h.id.clone())
        .expect("localhost must be seeded");
    let err = reg
        .remove(&local_id)
        .expect_err("remove localhost must fail");
    match err {
        HostError::CannotRemoveLocalhost => {}
        other => panic!("expected CannotRemoveLocalhost, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// MalformedTarget
// ---------------------------------------------------------------------------

#[test]
fn add_empty_target_returns_malformed_target() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let err = reg
        .add("badhost", "", None, None, Vec::new(), Vec::new())
        .expect_err("empty target must fail");
    match err {
        HostError::MalformedTarget(t) => assert!(t.is_empty() || t.contains("")),
        other => panic!("expected MalformedTarget, got {:?}", other),
    }
}

#[test]
fn add_bare_ssh_alias_is_accepted() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    // A bare host / ~/.ssh/config alias (e.g. "etx") is valid — `ssh etx` resolves it.
    let host = reg
        .add("box", "etx", None, None, Vec::new(), Vec::new())
        .expect("bare alias must be accepted");
    assert_eq!(host.target, "etx");
}

#[test]
fn add_half_formed_or_spaced_target_returns_malformed_target() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    // "user@" / "@host" (half of user@host) and whitespace are still rejected.
    for (i, bad) in ["user@", "@host", "a b", "  "].iter().enumerate() {
        match reg.add(&format!("bad{i}"), bad, None, None, Vec::new(), Vec::new()) {
            Err(HostError::MalformedTarget(_)) => {}
            other => panic!("expected MalformedTarget for {bad:?}, got {other:?}"),
        }
    }
}

#[test]
fn add_target_at_only_returns_malformed_target() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let err = reg
        .add("badhost3", "@", None, None, Vec::new(), Vec::new())
        .expect_err("@ only must fail");
    match err {
        HostError::MalformedTarget(_) => {}
        other => panic!("expected MalformedTarget, got {:?}", other),
    }
}

#[test]
fn add_target_missing_user_returns_malformed_target() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let err = reg
        .add("badhost4", "@somehost.example.com", None, None, Vec::new(), Vec::new())
        .expect_err("missing user must fail");
    match err {
        HostError::MalformedTarget(_) => {}
        other => panic!("expected MalformedTarget, got {:?}", other),
    }
}

#[test]
fn add_target_missing_host_returns_malformed_target() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let err = reg
        .add("badhost5", "user@", None, None, Vec::new(), Vec::new())
        .expect_err("missing host must fail");
    match err {
        HostError::MalformedTarget(_) => {}
        other => panic!("expected MalformedTarget, got {:?}", other),
    }
}

#[test]
fn add_empty_wsl_distro_returns_malformed_target() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    // `wsl:` with no distro name is malformed (the distro is required).
    let err = reg
        .add("nodist", "wsl:", None, None, Vec::new(), Vec::new())
        .expect_err("empty wsl distro must fail");
    match err {
        HostError::MalformedTarget(t) => assert_eq!(t, "wsl:"),
        other => panic!("expected MalformedTarget, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// WSL discovery — runner error propagates
// ---------------------------------------------------------------------------

#[test]
fn list_wsl_distros_propagates_runner_error() {
    let mut fake = FakeWslRunner::new();
    fake.respond(Err(HostError::Unreachable("wsl.exe missing".to_string())));
    let err = list_wsl_distros(&fake).expect_err("runner error must propagate");
    match err {
        HostError::Unreachable(m) => assert!(m.contains("wsl.exe")),
        other => panic!("expected Unreachable, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// NotFound — remove with unknown id
// ---------------------------------------------------------------------------

#[test]
fn remove_unknown_id_returns_not_found() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let err = reg
        .remove("nonexistent-id-xyz")
        .expect_err("remove unknown id must fail");
    match err {
        HostError::NotFound(id) => assert_eq!(id, "nonexistent-id-xyz"),
        other => panic!("expected NotFound, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// NotFound — health_check with unknown id
// ---------------------------------------------------------------------------

#[test]
fn health_check_unknown_id_returns_unreachable_status() {
    // Spec-pinned: health_check returns HostStatus (not Result); unknown id -> HostStatus::Unreachable.
    // Callers that need to distinguish "no such host" use list()/lookup first.
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let status = reg.health_check("nonexistent-id-xyz");
    assert_eq!(status, HostStatus::Unreachable);
}

// ---------------------------------------------------------------------------
// Unreachable — health_check when SshRunner returns Err(HostError::Unreachable)
// ---------------------------------------------------------------------------

#[test]
fn health_check_maps_runner_unreachable_to_unreachable_status() {
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "alice@dead.example.com",
        Err(HostError::Unreachable("dead.example.com".to_string())),
    );
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = {
        let host = reg
            .add("dead", "alice@dead.example.com", None, None, Vec::new(), Vec::new())
            .unwrap();
        host.id.clone()
    };
    let status = reg.health_check(&id);
    assert_eq!(status, HostStatus::Unreachable);
}

// ---------------------------------------------------------------------------
// HostError derives Debug (required by the spec — serde on data types)
// ---------------------------------------------------------------------------

#[test]
fn host_error_variants_are_debug() {
    let _ = format!("{:?}", HostError::DuplicateLabel("x".to_string()));
    let _ = format!("{:?}", HostError::CannotRemoveLocalhost);
    let _ = format!("{:?}", HostError::MalformedTarget("bad".to_string()));
    let _ = format!("{:?}", HostError::Unreachable("host".to_string()));
    let _ = format!("{:?}", HostError::NotFound("id".to_string()));
}

#[test]
fn update_unknown_id_is_not_found() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let err = reg
        .update("nope", None, None, Vec::new(), None, Vec::new())
        .expect_err("unknown id must be NotFound");
    assert!(matches!(err, HostError::NotFound(id) if id == "nope"));
}
