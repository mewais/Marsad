/// happy.rs — nominal paths for every public fn/variant.
///
/// Covers:
///   - parse_ssh_config: multi-Host block with HostName, User, IdentityFile
///   - HostRegistry::new: localhost entry is seeded automatically
///   - HostRegistry::add: returns reference to newly created Host
///   - HostRegistry::list: returns all entries including localhost
///   - HostRegistry::remove: removes a remote host by id
///   - HostRegistry::health_check -> Connected when SshRunner returns Ok
///   - HostRegistry::helper_install_descriptor: returns a valid HelperDescriptor
use marsad_hosts::{
    list_wsl_distros, parse_ssh_config, FakeSshRunner, FakeWslRunner, HelperDescriptor, HostEntry,
    HostRegistry, HostStatus, RealWslRunner, WslRunner,
};

// ---------------------------------------------------------------------------
// parse_ssh_config — multi-Host nominal path
// ---------------------------------------------------------------------------

#[test]
fn parse_multi_host_all_fields() {
    let config = r#"
Host webserver
    HostName 192.168.1.10
    User deploy
    IdentityFile ~/.ssh/deploy_rsa

Host devbox
    HostName devbox.example.com
    User alice
    IdentityFile ~/.ssh/id_ed25519
"#;
    let entries = parse_ssh_config(config);
    assert_eq!(entries.len(), 2, "expected 2 entries");

    let web = entries
        .iter()
        .find(|e| e.alias == "webserver")
        .expect("webserver not found");
    assert_eq!(web.hostname.as_deref(), Some("192.168.1.10"));
    assert_eq!(web.user.as_deref(), Some("deploy"));
    assert_eq!(web.identity_file.as_deref(), Some("~/.ssh/deploy_rsa"));

    let dev = entries
        .iter()
        .find(|e| e.alias == "devbox")
        .expect("devbox not found");
    assert_eq!(dev.hostname.as_deref(), Some("devbox.example.com"));
    assert_eq!(dev.user.as_deref(), Some("alice"));
    assert_eq!(dev.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
}

#[test]
fn parse_returns_vec_of_host_entries() {
    let config = "Host myhost\n    HostName 10.0.0.1\n    User bob\n";
    let entries: Vec<HostEntry> = parse_ssh_config(config);
    assert!(!entries.is_empty());
    // Verify public fields exist and have expected values
    let e = &entries[0];
    let _alias: &str = &e.alias;
    let _hostname: &Option<String> = &e.hostname;
    let _user: &Option<String> = &e.user;
    let _identity_file: &Option<String> = &e.identity_file;
}

// ---------------------------------------------------------------------------
// HostRegistry::new — seeds localhost
// ---------------------------------------------------------------------------

#[test]
fn new_seeds_localhost_entry() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let hosts = reg.list();
    assert!(!hosts.is_empty(), "localhost must be seeded on new");
    let local = hosts.iter().find(|h| h.is_local).expect("no local entry");
    assert!(!local.removable, "localhost must not be removable");
    assert!(local.label.to_lowercase().contains("local") || local.is_local);
}

// ---------------------------------------------------------------------------
// HostRegistry::add — nominal add of a remote
// ---------------------------------------------------------------------------

#[test]
fn add_remote_host_returns_host_ref() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let result = reg.add("myserver", "alice@myserver.example.com", None, None, Vec::new());
    let host = result.expect("add should succeed");
    assert_eq!(host.label, "myserver");
    assert_eq!(host.target, "alice@myserver.example.com");
    assert!(!host.is_local);
    assert!(host.removable);
    // New hosts start in Connecting until health_check
    assert_eq!(host.status, HostStatus::Connecting);
}

// ---------------------------------------------------------------------------
// HostRegistry::list — shows all including localhost
// ---------------------------------------------------------------------------

#[test]
fn list_includes_localhost_and_added_remote() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    reg.add("remote1", "bob@remote1.example.com", None, None, Vec::new()).unwrap();
    let hosts = reg.list();
    assert!(hosts.len() >= 2);
    assert!(
        hosts.iter().any(|h| h.is_local),
        "localhost must be in list"
    );
    assert!(
        hosts.iter().any(|h| h.label == "remote1"),
        "remote1 must be in list"
    );
}

// ---------------------------------------------------------------------------
// HostRegistry::remove — remove a remote host
// ---------------------------------------------------------------------------

#[test]
fn remove_remote_host_succeeds() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let id = {
        let host = reg.add("toremove", "charlie@toremove.example.com", None, None, Vec::new()).unwrap();
        host.id.clone()
    };
    let count_before = reg.list().len();
    reg.remove(&id).expect("remove should succeed");
    let count_after = reg.list().len();
    assert_eq!(count_after, count_before - 1);
    assert!(
        !reg.list().iter().any(|h| h.id == id),
        "removed host should not appear in list"
    );
}

// ---------------------------------------------------------------------------
// HostRegistry::health_check — Connected when SshRunner returns Ok
// ---------------------------------------------------------------------------

#[test]
fn health_check_returns_connected_on_ok_response() {
    let mut fake = FakeSshRunner::new();
    fake.respond("alice@healthy.example.com", Ok("pong".to_string()));
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = {
        let host = reg.add("healthy", "alice@healthy.example.com", None, None, Vec::new()).unwrap();
        host.id.clone()
    };
    let status = reg.health_check(&id);
    assert_eq!(status, HostStatus::Connected);
}

// ---------------------------------------------------------------------------
// HostRegistry::helper_install_descriptor — fields are present and non-empty
// ---------------------------------------------------------------------------

#[test]
fn helper_install_descriptor_fields_are_populated() {
    let runner = Box::new(FakeSshRunner::new());
    let reg = HostRegistry::new(runner);
    let desc: HelperDescriptor = reg.helper_install_descriptor();
    assert!(!desc.path.is_empty(), "descriptor path must not be empty");
    assert!(
        !desc.purpose.is_empty(),
        "descriptor purpose must not be empty"
    );
    // removable must be true: users can uninstall the helper
    assert!(desc.removable, "helper must be marked as removable");
}

// ---------------------------------------------------------------------------
// HostEntry derives Debug, Clone, PartialEq
// ---------------------------------------------------------------------------

#[test]
fn host_entry_derives_debug_clone_partialeq() {
    let entry = parse_ssh_config("Host foo\n    HostName bar.com\n    User bob\n");
    assert!(!entry.is_empty());
    let cloned = entry[0].clone();
    assert_eq!(entry[0], cloned);
    let _ = format!("{:?}", &entry[0]);
}

// ---------------------------------------------------------------------------
// Host derives Debug, Clone, PartialEq
// ---------------------------------------------------------------------------

#[test]
fn host_derives_debug_clone_partialeq() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    reg.add("derivetest", "user@derivetest.example.com", None, None, Vec::new())
        .unwrap();
    let hosts = reg.list();
    let h = &hosts[0];
    let cloned = h.clone();
    assert_eq!(*h, cloned);
    let _ = format!("{:?}", h);
}

// ---------------------------------------------------------------------------
// HostRegistry::set_status — apply an external health result; unknown id no-op
// ---------------------------------------------------------------------------

#[test]
fn set_status_updates_existing_host() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let id = reg.add("s", "u@h", None, None, Vec::new()).unwrap().id.clone();
    reg.set_status(&id, HostStatus::Connected);
    let h = reg.list().iter().find(|h| h.id == id).unwrap();
    assert_eq!(h.status, HostStatus::Connected);
}

#[test]
fn set_status_unknown_id_is_noop() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let before = reg.list().len();
    reg.set_status("host-999", HostStatus::Unreachable); // no panic, no change
    assert_eq!(reg.list().len(), before);
}

// ---------------------------------------------------------------------------
// HostRegistry::load_remote — re-insert persisted host, preserve id, bump next_id
// ---------------------------------------------------------------------------

#[test]
fn load_remote_preserves_id_and_starts_reconnecting() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    reg.load_remote("host-7", "saved", "u@saved.example.com", None, None, Vec::new());
    let h = reg.list().iter().find(|h| h.id == "host-7").unwrap();
    assert_eq!(h.label, "saved");
    assert!(!h.is_local && h.removable);
    assert_eq!(h.status, HostStatus::Connecting);
    // next_id must advance past 7 so a fresh add does not collide.
    let added = reg.add("fresh", "u@fresh", None, None, Vec::new()).unwrap();
    assert_eq!(added.id, "host-8");
}

// ---------------------------------------------------------------------------
// WSL host targets — add, is_wsl(), wsl_distro()
// ---------------------------------------------------------------------------

#[test]
fn add_wsl_target_is_accepted_and_flagged() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let host = reg
        .add("ubuntu", "wsl:Ubuntu", None, None, Vec::new())
        .expect("wsl target must be accepted");
    assert_eq!(host.target, "wsl:Ubuntu");
    assert!(host.is_wsl(), "wsl:<distro> host must report is_wsl()");
    assert_eq!(host.wsl_distro(), Some("Ubuntu"));
}

#[test]
fn non_wsl_hosts_are_not_flagged_wsl() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let ssh = reg.add("box", "user@box.example.com", None, None, Vec::new()).unwrap();
    assert!(!ssh.is_wsl());
    assert_eq!(ssh.wsl_distro(), None);
    // The seeded localhost ("this machine") is also not WSL.
    let local = reg.list().iter().find(|h| h.is_local).unwrap();
    assert!(!local.is_wsl());
    assert_eq!(local.wsl_distro(), None);
}

// ---------------------------------------------------------------------------
// WSL distro discovery — list_wsl_distros via FakeWslRunner
// ---------------------------------------------------------------------------

#[test]
fn list_wsl_distros_parses_distro_lines() {
    let mut fake = FakeWslRunner::new();
    // `wsl.exe -l -q` emits one distro per line, CRLF-terminated; a trailing
    // blank line is common.
    fake.respond(Ok("Ubuntu\r\nDebian\r\n\r\n".to_string()));
    let targets = list_wsl_distros(&fake).expect("discovery must succeed");
    assert_eq!(targets, vec!["wsl:Ubuntu", "wsl:Debian"]);
}

#[test]
fn list_wsl_distros_empty_when_none_installed() {
    let mut fake = FakeWslRunner::new();
    fake.respond(Ok(String::new()));
    let targets = list_wsl_distros(&fake).expect("discovery must succeed");
    assert!(targets.is_empty());
}

#[test]
fn fake_wsl_runner_implements_trait_and_real_runner_constructs() {
    let fake = FakeWslRunner::new();
    let _boxed: Box<dyn WslRunner> = Box::new(fake);
    // RealWslRunner::list() is a no-op stub off-Windows / under coverage.
    let real = RealWslRunner;
    let out = real.list().expect("stub list must succeed");
    assert!(out.is_empty());
}

#[test]
fn load_remote_non_numeric_id_leaves_next_id() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    reg.load_remote("custom-id", "x", "u@x", None, None, Vec::new()); // unparseable suffix
    reg.load_remote("host-0", "y", "u@y", None, None, Vec::new()); // n (0) < next_id (1)
    let added = reg.add("z", "u@z", None, None, Vec::new()).unwrap();
    assert_eq!(added.id, "host-1"); // next_id untouched by either load
}

// ---------------------------------------------------------------------------
// HostRegistry::update — launch config on a remote host, and on localhost
// ---------------------------------------------------------------------------

#[test]
fn update_sets_launch_config_on_remote_host() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    let id = reg.add("box", "u@h", None, None, Vec::new()).unwrap().id.clone();
    let updated = reg
        .update(
            &id,
            Some("genie".to_string()),
            Some("module load x".to_string()),
            vec![("A".to_string(), "b".to_string())],
            Some("u@h2".to_string()),
        )
        .expect("update existing host");
    assert_eq!(updated.claude_bin.as_deref(), Some("genie"));
    assert_eq!(updated.setup.as_deref(), Some("module load x"));
    assert_eq!(updated.env, vec![("A".to_string(), "b".to_string())]);
    assert_eq!(updated.target, "u@h2"); // target changed
}

#[test]
fn update_localhost_keeps_target_when_none() {
    let mut reg = HostRegistry::new(Box::new(FakeSshRunner::new()));
    // localhost is seeded and protected; update must still work and leave its
    // target ("this machine") unchanged when target is None.
    let updated = reg
        .update(
            "localhost",
            Some("/opt/claude".to_string()),
            None,
            Vec::new(),
            None,
        )
        .expect("update localhost");
    assert_eq!(updated.claude_bin.as_deref(), Some("/opt/claude"));
    assert_eq!(updated.target, "this machine");
    assert!(updated.is_local);
}
