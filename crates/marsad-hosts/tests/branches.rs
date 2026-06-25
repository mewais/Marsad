/// branches.rs — table-driven cases closing remaining regions to 100%.
///
/// Covers:
///   - parse_ssh_config: comments, wildcard host patterns, missing optional fields
///   - HostStatus transitions: Connected -> Reconnecting -> Unreachable
///   - Localhost protection flags: is_local=true, removable=false, seeded by new
///   - helper_install_descriptor: all fields inspected
///   - HostEntry with partial fields (no IdentityFile, no User, etc.)
///   - Multiple health_check calls updating status in-place
use marsad_hosts::{
    parse_ssh_config, FakeSshRunner, HelperDescriptor, HostError, HostRegistry, HostStatus,
};

// ---------------------------------------------------------------------------
// parse_ssh_config — comments are ignored
// ---------------------------------------------------------------------------

#[test]
fn parse_ignores_comment_lines() {
    let config = r#"
# This is a top-level comment
Host server1
    # inline comment in block
    HostName 10.0.0.1
    User admin
    # another comment
    IdentityFile ~/.ssh/id_rsa
"#;
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].alias, "server1");
    assert_eq!(entries[0].hostname.as_deref(), Some("10.0.0.1"));
    assert_eq!(entries[0].user.as_deref(), Some("admin"));
}

// ---------------------------------------------------------------------------
// parse_ssh_config — wildcard host patterns are kept as-is
// ---------------------------------------------------------------------------

#[test]
fn parse_wildcard_host_kept_as_alias() {
    let config = r#"
Host *.internal
    User svc

Host 10.0.*
    IdentityFile ~/.ssh/corp_rsa
"#;
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 2, "both wildcard blocks must be parsed");
    assert!(
        entries.iter().any(|e| e.alias == "*.internal"),
        "wildcard alias kept"
    );
    assert!(
        entries.iter().any(|e| e.alias == "10.0.*"),
        "wildcard alias kept"
    );
}

// ---------------------------------------------------------------------------
// parse_ssh_config — missing optional fields return None
// ---------------------------------------------------------------------------

#[test]
fn parse_host_with_no_optional_fields_returns_none() {
    let config = "Host barebones\n";
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].alias, "barebones");
    assert_eq!(entries[0].hostname, None);
    assert_eq!(entries[0].user, None);
    assert_eq!(entries[0].identity_file, None);
}

#[test]
fn parse_host_with_only_hostname_has_none_user_and_identity_file() {
    let config = "Host partial\n    HostName partial.example.com\n";
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hostname.as_deref(), Some("partial.example.com"));
    assert_eq!(entries[0].user, None);
    assert_eq!(entries[0].identity_file, None);
}

#[test]
fn parse_host_with_only_user_has_none_hostname_and_identity_file() {
    let config = "Host withuser\n    User carol\n";
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hostname, None);
    assert_eq!(entries[0].user.as_deref(), Some("carol"));
    assert_eq!(entries[0].identity_file, None);
}

// ---------------------------------------------------------------------------
// parse_ssh_config — empty config returns empty vec
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_config_returns_empty_vec() {
    let entries = parse_ssh_config("");
    assert!(entries.is_empty());
}

#[test]
fn parse_only_comments_returns_empty_vec() {
    let config = "# just a comment\n# another one\n";
    let entries = parse_ssh_config(config);
    assert!(entries.is_empty());
}

// ---------------------------------------------------------------------------
// HostStatus transitions
// ---------------------------------------------------------------------------

#[test]
fn new_remote_host_starts_connecting() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let host = reg.add("newhost", "user@newhost.example.com", None, None, Vec::new()).unwrap();
    assert_eq!(host.status, HostStatus::Connecting);
}

#[test]
fn set_status_never_connected_keeps_connecting_on_transient() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let id = reg.add("nc", "user@nc.example.com", None, None, Vec::new()).unwrap().id.clone();

    // A transient failure before the first successful connect stays Connecting.
    reg.set_status(&id, HostStatus::Reconnecting);
    assert_eq!(reg.list().iter().find(|h| h.id == id).unwrap().status, HostStatus::Connecting);

    // A hard failure still flips it (the host is clearly down).
    reg.set_status(&id, HostStatus::Unreachable);
    assert_eq!(reg.list().iter().find(|h| h.id == id).unwrap().status, HostStatus::Unreachable);

    // Once connected, later transient failures become Reconnecting as usual.
    reg.set_status(&id, HostStatus::Connected);
    reg.set_status(&id, HostStatus::Reconnecting);
    assert_eq!(reg.list().iter().find(|h| h.id == id).unwrap().status, HostStatus::Reconnecting);
}

#[test]
fn health_check_transitions_reconnecting_to_connected() {
    let mut fake = FakeSshRunner::new();
    fake.respond("user@ok.example.com", Ok("ok".to_string()));
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = {
        let h = reg.add("ok", "user@ok.example.com", None, None, Vec::new()).unwrap();
        assert_eq!(
            h.status,
            HostStatus::Connecting,
            "must start Connecting"
        );
        h.id.clone()
    };
    let status = reg.health_check(&id);
    assert_eq!(status, HostStatus::Connected);
    // The stored Host should also reflect the new status
    let stored = reg.list().iter().find(|h| h.id == id).unwrap();
    assert_eq!(stored.status, HostStatus::Connected);
}

#[test]
fn health_check_transitions_connected_to_unreachable_on_hard_failure() {
    let mut fake = FakeSshRunner::new();
    // First call succeeds, second call returns hard failure
    fake.respond("user@flip.example.com", Ok("ok".to_string()));
    fake.respond(
        "user@flip.example.com",
        Err(HostError::Unreachable("flip.example.com".to_string())),
    );
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = {
        let h = reg.add("flip", "user@flip.example.com", None, None, Vec::new()).unwrap();
        h.id.clone()
    };
    // First health_check -> Connected
    let s1 = reg.health_check(&id);
    assert_eq!(s1, HostStatus::Connected);
    // Second health_check -> Unreachable
    let s2 = reg.health_check(&id);
    assert_eq!(s2, HostStatus::Unreachable);
    let stored = reg.list().iter().find(|h| h.id == id).unwrap();
    assert_eq!(stored.status, HostStatus::Unreachable);
}

#[test]
fn health_check_reconnecting_status_on_transient_failure() {
    // Spec-pinned: HostError::Unreachable(_) -> hard -> HostStatus::Unreachable;
    // any other HostError -> transient -> HostStatus::Reconnecting.
    // We script a non-Unreachable error (NotFound) to exercise the transient path.
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "user@transient.example.com",
        Err(HostError::NotFound("transient".to_string())),
    );
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = {
        let h = reg.add("transient", "user@transient.example.com", None, None, Vec::new()).unwrap();
        h.id.clone()
    };
    let status = reg.health_check(&id);
    assert_eq!(status, HostStatus::Reconnecting);
}

// ---------------------------------------------------------------------------
// Localhost protection flags
// ---------------------------------------------------------------------------

#[test]
fn localhost_is_local_true() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let local = reg
        .list()
        .iter()
        .find(|h| h.is_local)
        .expect("localhost must exist");
    assert!(local.is_local);
}

#[test]
fn localhost_removable_false() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let local = reg
        .list()
        .iter()
        .find(|h| h.is_local)
        .expect("localhost must exist");
    assert!(!local.removable);
}

#[test]
fn remote_host_is_local_false_and_removable_true() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let host = reg.add("remote", "user@remote.example.com", None, None, Vec::new()).unwrap();
    assert!(!host.is_local);
    assert!(host.removable);
}

#[test]
fn only_one_localhost_seeded_by_new() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let local_count = reg.list().iter().filter(|h| h.is_local).count();
    assert_eq!(local_count, 1, "exactly one localhost must be seeded");
}

// ---------------------------------------------------------------------------
// helper_install_descriptor fields
// ---------------------------------------------------------------------------

#[test]
fn helper_descriptor_path_non_empty() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let desc = reg.helper_install_descriptor();
    assert!(!desc.path.is_empty());
}

#[test]
fn helper_descriptor_purpose_non_empty() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let desc = reg.helper_install_descriptor();
    assert!(!desc.purpose.is_empty());
}

#[test]
fn helper_descriptor_removable_true() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let desc: HelperDescriptor = reg.helper_install_descriptor();
    assert!(
        desc.removable,
        "helper must be marked removable (user can uninstall)"
    );
}

#[test]
fn helper_descriptor_derives_debug_clone_partialeq() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let desc = reg.helper_install_descriptor();
    let cloned = desc.clone();
    assert_eq!(desc, cloned);
    let _ = format!("{:?}", &desc);
}

// ---------------------------------------------------------------------------
// HostStatus derives PartialEq, Debug, Clone
// ---------------------------------------------------------------------------

#[test]
fn host_status_derives_partialeq_debug_clone() {
    let s = HostStatus::Connected;
    let c = s.clone();
    assert_eq!(s, c);
    let _ = format!("{:?}", &s);

    let s2 = HostStatus::Reconnecting;
    assert_ne!(s, s2);

    let s3 = HostStatus::Unreachable;
    let _ = format!("{:?}", s3);
}

// ---------------------------------------------------------------------------
// session_count field is present and starts at 0 for new remote hosts
// ---------------------------------------------------------------------------

#[test]
fn new_remote_host_session_count_starts_at_zero() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let host = reg.add("fresh", "user@fresh.example.com", None, None, Vec::new()).unwrap();
    assert_eq!(host.session_count, 0);
}

// ---------------------------------------------------------------------------
// Host.id is unique across multiple adds
// ---------------------------------------------------------------------------

#[test]
fn host_ids_are_unique() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let id1 = reg.add("h1", "user@h1.example.com", None, None, Vec::new()).unwrap().id.clone();
    let id2 = reg.add("h2", "user@h2.example.com", None, None, Vec::new()).unwrap().id.clone();
    assert_ne!(id1, id2);
}
