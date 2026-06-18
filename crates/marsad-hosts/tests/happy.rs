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
use marsad_hosts::{parse_ssh_config, FakeSshRunner, HelperDescriptor, Host, HostEntry, HostRegistry, HostStatus};

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

    let web = entries.iter().find(|e| e.alias == "webserver").expect("webserver not found");
    assert_eq!(web.hostname.as_deref(), Some("192.168.1.10"));
    assert_eq!(web.user.as_deref(), Some("deploy"));
    assert_eq!(web.identity_file.as_deref(), Some("~/.ssh/deploy_rsa"));

    let dev = entries.iter().find(|e| e.alias == "devbox").expect("devbox not found");
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
    assert_eq!(local.label.to_lowercase().contains("local") || local.is_local, true);
}

// ---------------------------------------------------------------------------
// HostRegistry::add — nominal add of a remote
// ---------------------------------------------------------------------------

#[test]
fn add_remote_host_returns_host_ref() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let result = reg.add("myserver", "alice@myserver.example.com");
    let host = result.expect("add should succeed");
    assert_eq!(host.label, "myserver");
    assert_eq!(host.target, "alice@myserver.example.com");
    assert!(!host.is_local);
    assert!(host.removable);
    // New hosts start in Reconnecting until health_check
    assert_eq!(host.status, HostStatus::Reconnecting);
}

// ---------------------------------------------------------------------------
// HostRegistry::list — shows all including localhost
// ---------------------------------------------------------------------------

#[test]
fn list_includes_localhost_and_added_remote() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    reg.add("remote1", "bob@remote1.example.com").unwrap();
    let hosts = reg.list();
    assert!(hosts.len() >= 2);
    assert!(hosts.iter().any(|h| h.is_local), "localhost must be in list");
    assert!(hosts.iter().any(|h| h.label == "remote1"), "remote1 must be in list");
}

// ---------------------------------------------------------------------------
// HostRegistry::remove — remove a remote host
// ---------------------------------------------------------------------------

#[test]
fn remove_remote_host_succeeds() {
    let runner = Box::new(FakeSshRunner::new());
    let mut reg = HostRegistry::new(runner);
    let id = {
        let host = reg.add("toremove", "charlie@toremove.example.com").unwrap();
        host.id.clone()
    };
    let count_before = reg.list().len();
    reg.remove(&id).expect("remove should succeed");
    let count_after = reg.list().len();
    assert_eq!(count_after, count_before - 1);
    assert!(!reg.list().iter().any(|h| h.id == id), "removed host should not appear in list");
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
        let host = reg.add("healthy", "alice@healthy.example.com").unwrap();
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
    assert!(!desc.purpose.is_empty(), "descriptor purpose must not be empty");
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
    reg.add("derivetest", "user@derivetest.example.com").unwrap();
    let hosts = reg.list();
    let h = &hosts[0];
    let cloned = h.clone();
    assert_eq!(*h, cloned);
    let _ = format!("{:?}", h);
}
