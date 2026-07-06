/// fakes.rs — exercise the SshRunner trait contract via FakeSshRunner.
///
/// Covers:
///   - FakeSshRunner::new()
///   - FakeSshRunner::respond(target, Ok(output)) — scripted success
///   - FakeSshRunner::respond(target, Err(HostError::Unreachable)) — scripted hard error
///   - SshRunner trait: run(&target, &cmd) -> Result<String, HostError>
///   - health_check maps SshRunner::Ok  -> HostStatus::Connected
///   - health_check maps SshRunner::Err(Unreachable) -> HostStatus::Unreachable
///   - Multiple scripted responses consumed in registration order
///   - FakeSshRunner usable directly as a SshRunner (trait object coercion)
use marsad_hosts::{FakeSshRunner, HostError, HostRegistry, HostStatus, SshRunner};

// ---------------------------------------------------------------------------
// FakeSshRunner basics
// ---------------------------------------------------------------------------

#[test]
fn fake_ssh_runner_new_does_not_panic() {
    let _fake = FakeSshRunner::new();
}

#[test]
fn fake_ssh_runner_implements_ssh_runner_trait() {
    // If this compiles, FakeSshRunner implements SshRunner.
    let fake = FakeSshRunner::new();
    let _boxed: Box<dyn SshRunner> = Box::new(fake);
}

// ---------------------------------------------------------------------------
// SshRunner::run — scripted Ok response
// ---------------------------------------------------------------------------

#[test]
fn fake_runner_returns_scripted_ok_output() {
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "user@target.example.com",
        Ok("hello from target".to_string()),
    );
    let result = fake.run("user@target.example.com", "echo hello");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "hello from target");
}

// ---------------------------------------------------------------------------
// SshRunner::run — scripted Err response
// ---------------------------------------------------------------------------

#[test]
fn fake_runner_returns_scripted_err_unreachable() {
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "user@down.example.com",
        Err(HostError::Unreachable("down.example.com".to_string())),
    );
    let result = fake.run("user@down.example.com", "echo hello");
    assert!(result.is_err());
    match result.unwrap_err() {
        HostError::Unreachable(h) => assert_eq!(h, "down.example.com"),
        other => panic!("expected Unreachable, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// SshRunner::run — scripted responses consumed in order per target
// ---------------------------------------------------------------------------

#[test]
fn fake_runner_consumes_responses_in_order() {
    let mut fake = FakeSshRunner::new();
    fake.respond("user@seq.example.com", Ok("first".to_string()));
    fake.respond("user@seq.example.com", Ok("second".to_string()));

    let r1 = fake.run("user@seq.example.com", "cmd").unwrap();
    let r2 = fake.run("user@seq.example.com", "cmd").unwrap();
    assert_eq!(r1, "first");
    assert_eq!(r2, "second");
}

// ---------------------------------------------------------------------------
// health_check integration with FakeSshRunner — Connected path
// ---------------------------------------------------------------------------

#[test]
fn health_check_connected_via_fake_ok() {
    let mut fake = FakeSshRunner::new();
    fake.respond("user@alive.example.com", Ok("alive".to_string()));
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = reg
        .add("alive", "user@alive.example.com", None, None, Vec::new(), Vec::new())
        .unwrap()
        .id
        .clone();
    assert_eq!(reg.health_check(&id), HostStatus::Connected);
}

// ---------------------------------------------------------------------------
// health_check integration with FakeSshRunner — Unreachable path
// ---------------------------------------------------------------------------

#[test]
fn health_check_unreachable_via_fake_err_unreachable() {
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "user@gone.example.com",
        Err(HostError::Unreachable("gone.example.com".to_string())),
    );
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = reg
        .add("gone", "user@gone.example.com", None, None, Vec::new(), Vec::new())
        .unwrap()
        .id
        .clone();
    assert_eq!(reg.health_check(&id), HostStatus::Unreachable);
}

// ---------------------------------------------------------------------------
// health_check integration with FakeSshRunner — Reconnecting path (transient error)
// ---------------------------------------------------------------------------

#[test]
fn health_check_reconnecting_via_fake_err_non_unreachable() {
    // Spec-pinned: HostError::Unreachable(_) -> hard -> HostStatus::Unreachable;
    // any other HostError -> transient -> HostStatus::Reconnecting.
    // NotFound is used here as the representative non-Unreachable error.
    let mut fake = FakeSshRunner::new();
    fake.respond(
        "user@flaky.example.com",
        Err(HostError::NotFound("flaky".to_string())),
    );
    let runner = Box::new(fake);
    let mut reg = HostRegistry::new(runner);
    let id = reg
        .add("flaky", "user@flaky.example.com", None, None, Vec::new(), Vec::new())
        .unwrap()
        .id
        .clone();
    assert_eq!(reg.health_check(&id), HostStatus::Reconnecting);
}

// ---------------------------------------------------------------------------
// FakeSshRunner scripted for multiple distinct targets
// ---------------------------------------------------------------------------

#[test]
fn fake_runner_handles_multiple_targets_independently() {
    let mut fake = FakeSshRunner::new();
    fake.respond("user@a.example.com", Ok("from-a".to_string()));
    fake.respond("user@b.example.com", Ok("from-b".to_string()));

    let ra = fake.run("user@a.example.com", "cmd").unwrap();
    let rb = fake.run("user@b.example.com", "cmd").unwrap();
    assert_eq!(ra, "from-a");
    assert_eq!(rb, "from-b");
}

// ---------------------------------------------------------------------------
// FakeSshRunner can be used as a Box<dyn SshRunner> passed to HostRegistry
// ---------------------------------------------------------------------------

#[test]
fn fake_as_trait_object_in_registry() {
    let fake = FakeSshRunner::new();
    let _reg = HostRegistry::new(Box::new(fake) as Box<dyn SshRunner>);
}

// ---------------------------------------------------------------------------
// FakeSshRunner scripted mixed Ok and Err for same target
// ---------------------------------------------------------------------------

#[test]
fn fake_runner_mixed_ok_and_err_in_sequence() {
    let mut fake = FakeSshRunner::new();
    fake.respond("user@mix.example.com", Ok("up".to_string()));
    fake.respond(
        "user@mix.example.com",
        Err(HostError::Unreachable("mix.example.com".to_string())),
    );
    fake.respond("user@mix.example.com", Ok("up-again".to_string()));

    assert!(fake.run("user@mix.example.com", "probe").is_ok());
    assert!(fake.run("user@mix.example.com", "probe").is_err());
    assert!(fake.run("user@mix.example.com", "probe").is_ok());
}
