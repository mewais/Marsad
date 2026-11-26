#![cfg(target_os = "linux")]
//! The sign-in step of a host's shared ssh connection ([`Masters::sign_in`]), against a real
//! sshd on localhost. The command the hub builds runs through a small wrapper, which can make
//! ssh take its time, stand for an ssh that fails or never gets anywhere, and say its pid; every
//! connection is the real ssh's, and so is every `-O check` / `-O exit` the hub runs.
//!
//! Every wait in here is for an event (a process's end, a line in a FIFO): nothing is timed.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::{boot_target, isolated_home_env, master_pid, Sshd, TempDir};
use marsad_agent::hub::master::{Check, Masters, SignInCommand, SignInStep};
use marsad_agent::hub::transport::{bootstrap_argv_via, SshRoute};
use marsad_proto::diag::MasterExitWhy;
use marsad_proto::hosts::{HostEntry, HostKind};
use marsad_proto::HostId;
use wasit_control::ExitInfo;
use wasit_diag::Ident;

/// The wrapper ssh. Environment:
/// * `WRAP_PIDFIFO`: a FIFO it writes its pid to first (the pid ssh then has: it execs).
/// * `WRAP_DELAY`: seconds it takes before it starts ssh (a slow host).
/// * `WRAP_MODE`: `ssh` (default) runs the real ssh with the hub's arguments; `hang` never gets
///   anywhere; `fail` exits 255 saying why on stderr; `exit0` exits 0 and does nothing.
const WRAPPER: &str = r#"#!/bin/sh
if [ -n "$WRAP_PIDFIFO" ]; then echo $$ > "$WRAP_PIDFIFO"; fi
sleep "${WRAP_DELAY:-0}"
case ${WRAP_MODE:-ssh} in
  ssh) exec "$WRAP_SSH" "$@" ;;
  hang) exec sleep 3600 ;;
  fail) echo "wrapper: refused" >&2; exit 255 ;;
  exit0) exit 0 ;;
esac
exit 2
"#;

/// The tests' own home, which every ssh the hub side runs here (and the hub's `-O` commands)
/// reads instead of the owner's: set once for the whole process, the same for every test.
fn isolate_home() {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join(format!("mrs-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        for (k, v) in isolated_home_env(&home) {
            std::env::set_var(k, v);
        }
        std::env::set_var("SSH_AUTH_SOCK", "");
        home
    });
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn run<F: std::future::Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Whether `pid` is running: there and not a zombie (an exited child not reaped yet).
fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        let state = stat.rsplit_once(')').map(|(_, rest)| rest.trim_start());
        !matches!(state.and_then(|s| s.chars().next()), None | Some('Z' | 'X'))
    })
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn syscall(num: std::ffi::c_long, ...) -> std::ffi::c_long;
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
    fn close(fd: i32) -> i32;
}

/// `pidfd_open(2)` (the same number on every Linux architecture).
const SYS_PIDFD_OPEN: std::ffi::c_long = 434;

/// Returns once `pid` (not a child of this process: a detached master) has ended. A pidfd
/// becomes readable when its process ends; `poll` without a timeout waits for exactly that.
fn wait_gone(pid: u32) {
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1.
    let fd = unsafe {
        syscall(
            SYS_PIDFD_OPEN,
            pid as std::ffi::c_int,
            0 as std::ffi::c_uint,
        )
    };
    if fd < 0 {
        // ESRCH: it is gone already.
        assert!(!alive(pid), "pidfd_open failed for a live pid {pid}");
        return;
    }
    let fd = i32::try_from(fd).unwrap();
    let mut p = PollFd {
        fd,
        events: 1, // POLLIN
        revents: 0,
    };
    // SAFETY: one valid pollfd; the descriptor is ours and closed once.
    unsafe {
        while poll(&mut p, 1, -1) < 0 {}
        close(fd);
    }
}

/// Reads the first line written to the FIFO at `path` (blocks until a writer writes it).
async fn line_of(path: &Path) -> String {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || std::fs::read_to_string(path).unwrap())
        .await
        .unwrap()
        .trim()
        .to_owned()
}

fn mkfifo(path: &Path) {
    assert!(std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap()
        .success());
}

fn real_ssh() -> PathBuf {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("ssh"))
        .find(|p| p.is_file())
        .expect("ssh on PATH")
}

struct Rig {
    d: TempDir,
    _sshd: Sshd,
    masters: Masters,
    entry: HostEntry,
    control: PathBuf,
    wrapper: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        isolate_home();
        let d = TempDir::new();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let sshd = Sshd::start(d.path());
        let wrapper = d.path().join("wrap-ssh");
        std::fs::write(&wrapper, WRAPPER).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let masters = Masters::new(d.path().join("ctl"));
        run(masters.prepare()).unwrap();
        let entry = HostEntry {
            id: HostId::new("box"),
            name: "box".into(),
            kind: sshd.kind(&sshd.key),
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            ask_before_connecting: false,
            local_files: false,
            shared_connection: true,
            signed_out: false,
            machine: None,
        };
        let control = masters.path(&entry).unwrap();
        Rig {
            d,
            _sshd: sshd,
            masters,
            entry,
            control,
            wrapper,
        }
    }

    /// The sign-in command as the hub builds it, run through the wrapper with `env`, and
    /// `extra` ssh options.
    fn command(&self, env: &[(&str, &str)], extra: &[String]) -> SignInCommand {
        let mut argv = bootstrap_argv_via(
            &self.entry,
            boot_target("box", "0123456789abcdef"),
            SshRoute::SignIn {
                control: &self.control,
            },
        )
        .unwrap();
        argv[0] = self.wrapper.display().to_string();
        for (i, o) in extra.iter().enumerate() {
            argv.insert(1 + 2 * i, "-o".into());
            argv.insert(2 + 2 * i, o.clone());
        }
        let mut env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        env.push(("WRAP_SSH".into(), real_ssh().display().to_string()));
        SignInCommand {
            argv,
            env,
            link: Ident::new("hosts:box"),
            attempt: 1,
        }
    }

    /// A sign-in as the hub runs it: the generation it began under, then the step.
    async fn sign_in(&self, cmd: SignInCommand) -> SignInStep {
        let generation = self.masters.begin_signin(&self.entry, &self.control).await;
        self.masters
            .sign_in(&self.entry, &self.control, generation, cmd)
            .await
    }

    fn fifo(&self, name: &str) -> PathBuf {
        let p = self.d.path().join(name);
        mkfifo(&p);
        p
    }
}

#[test]
fn a_signed_in_connection_outlives_the_hub_side_that_started_it() {
    let rig = Rig::new();
    let rt = runtime();
    let step = rt.block_on(rig.sign_in(rig.command(&[], &[])));
    assert_eq!(step, SignInStep::Open);
    let master = master_pid(&rig.control).expect("a master behind the socket");
    // The runtime ends with its tasks, the one that reaped the sign-in command among them.
    drop(rt);
    assert!(
        alive(master),
        "the connection must stay open after the hub side is gone"
    );
    assert_eq!(
        run(rig.masters.check(&rig.entry, &rig.control, None)),
        Check::Alive
    );
    run(rig.masters.exit(&rig.entry, MasterExitWhy::SignedOut));
    wait_gone(master);
    assert!(!rig.control.exists(), "no socket is left");
}

#[test]
fn the_master_is_detached_from_the_hub_with_null_stdio() {
    let rig = Rig::new();
    let step = run(rig.sign_in(rig.command(&[], &[])));
    assert_eq!(step, SignInStep::Open);
    let pid = master_pid(&rig.control).expect("a master behind the socket");
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    // Fields after the command name: state, ppid, pgrp, session.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    let own = std::fs::read_to_string("/proc/self/stat").unwrap();
    let own: Vec<&str> = own.rsplit_once(')').unwrap().1.split_whitespace().collect();
    assert_ne!(fields[1], std::process::id().to_string(), "not our child");
    assert_ne!(fields[2], own[2], "not in our process group: {stat}");
    assert_ne!(fields[3], own[3], "not in our session: {stat}");
    for fd in 0..3 {
        let target = std::fs::read_link(format!("/proc/{pid}/fd/{fd}")).unwrap();
        assert_eq!(target, Path::new("/dev/null"), "fd {fd} is null");
    }
    run(rig.masters.exit(&rig.entry, MasterExitWhy::SignedOut));
    wait_gone(pid);
}

#[test]
fn a_slow_host_is_waited_for_and_not_timed_out() {
    let rig = Rig::new();
    let began = Instant::now();
    let step = run(rig.sign_in(rig.command(&[("WRAP_DELAY", "4")], &[])));
    assert_eq!(
        step,
        SignInStep::Open,
        "authenticating takes several seconds"
    );
    assert!(began.elapsed() >= Duration::from_secs(4));
    let pid = master_pid(&rig.control).expect("a master behind the socket");
    run(rig.masters.exit(&rig.entry, MasterExitWhy::SignedOut));
    wait_gone(pid);
}

#[test]
fn a_command_that_fails_is_ended_with_its_stderr_and_nothing_is_left() {
    let rig = Rig::new();
    let fifo = rig.fifo("pid");
    let f = fifo.display().to_string();
    run(async {
        let cmd = rig.command(&[("WRAP_MODE", "fail"), ("WRAP_PIDFIFO", &f)], &[]);
        let (step, pid) = tokio::join!(rig.sign_in(cmd), line_of(&fifo));
        assert_eq!(
            step,
            SignInStep::Ended {
                exit: ExitInfo {
                    code: Some(255),
                    signal: None
                },
                stderr: "wrapper: refused".into(),
            }
        );
        // Reaped by the time the step ended.
        assert!(!alive(pid.parse().unwrap()), "the command is not running");
    });
    assert!(!rig.control.exists(), "no socket was left");
}

/// ssh warns about an identity file it cannot read on its stderr, before `-E` applies: that is
/// what the sign-in reports along with the failure.
#[test]
fn what_ssh_says_before_its_log_opens_is_reported() {
    let mut rig = Rig::new();
    let missing = rig.d.path().join("no-such-key").display().to_string();
    let HostKind::Ssh { identity_file, .. } = &mut rig.entry.kind else {
        unreachable!()
    };
    *identity_file = Some(missing.clone());
    let step = run(rig.sign_in(rig.command(&[], &["BatchMode=yes".into()])));
    let SignInStep::Ended { exit, stderr } = step else {
        panic!("expected a failed sign-in, got {step:?}")
    };
    assert_eq!(exit.code, Some(255));
    assert!(stderr.contains(&missing), "{stderr:?}");
}

#[test]
fn a_command_that_exits_zero_without_a_connection_is_no_connection() {
    let rig = Rig::new();
    let step = run(rig.sign_in(rig.command(&[("WRAP_MODE", "exit0")], &[])));
    assert_eq!(step, SignInStep::NoConnection { code: Some(0) });
}

#[test]
fn signing_out_during_a_sign_in_kills_its_command() {
    let rig = Rig::new();
    let fifo = rig.fifo("pid");
    let f = fifo.display().to_string();
    run(async {
        let cmd = rig.command(&[("WRAP_MODE", "hang"), ("WRAP_PIDFIFO", &f)], &[]);
        let generation = rig.masters.begin_signin(&rig.entry, &rig.control).await;
        let signing_in = rig
            .masters
            .sign_in(&rig.entry, &rig.control, generation, cmd);
        let sign_out = async {
            let pid: u32 = line_of(&fifo).await.parse().unwrap();
            rig.masters.mark_signed_out(&rig.entry.id);
            pid
        };
        let (step, pid) = tokio::join!(signing_in, sign_out);
        assert_eq!(step, SignInStep::SignedOut);
        assert!(
            !alive(pid),
            "the command is killed (and reaped) by the sign-out"
        );
    });
}

/// The leak this design exists to close: ssh authenticated but could not create the control
/// socket (something is in its place), so it shares nothing and goes on as a plain `-N`
/// connection. It must stay the hub's child, waited for without a timer, and end with the
/// sign-out; under `-f` it forked into the background with nothing to end it (and the step
/// ended at once, with no connection).
#[test]
fn an_authenticated_ssh_without_its_socket_stays_ours_until_sign_out() {
    let rig = Rig::new();
    let authed = rig.fifo("authed");
    let gate = rig.fifo("gate");
    run(async {
        // Not a forwarding: the hub's ssh commands ignore the configuration's forwardings
        // (`ClearAllForwardings`, which ssh applies to command-line `-L` too), so the connected
        // state is shown by the `LocalCommand` alone. It runs once ssh authenticated, before it
        // would detach or fork, and holds ssh until the test opens the gate.
        let options = [
            "PermitLocalCommand=yes".to_owned(),
            format!(
                "LocalCommand=echo $PPID > {} && read go < {}",
                authed.display(),
                gate.display()
            ),
        ];
        let generation = rig.masters.begin_signin(&rig.entry, &rig.control).await;
        // Something that is not a socket where the socket goes: ssh cannot share the connection.
        std::fs::write(&rig.control, "in the way").unwrap();
        let cmd = rig.command(&[], &options);
        let signing_in = rig
            .masters
            .sign_in(&rig.entry, &rig.control, generation, cmd);
        let user = async {
            let pid: u32 = line_of(&authed).await.parse().unwrap();
            tokio::fs::write(&gate, "go\n").await.unwrap();
            // ssh is authenticated, connected, with no shared connection. The process that
            // signed in is that one, still running in the foreground (under `-f` it exited
            // right after its fork, long before a forwarded connection gets this far).
            assert!(
                running(pid),
                "the ssh that signed in is still running, ours"
            );
            // The user signs out.
            rig.masters.mark_signed_out(&rig.entry.id);
            pid
        };
        let (step, pid) = tokio::join!(signing_in, user);
        assert_eq!(
            step,
            SignInStep::SignedOut,
            "the sign-in waited for the user"
        );
        assert!(!alive(pid), "the ssh that signed in is ended (and reaped)");
    });
    let _ = std::fs::remove_file(&rig.control);
}

#[test]
fn a_command_that_cannot_start_is_reported_as_such() {
    let rig = Rig::new();
    let mut cmd = rig.command(&[], &[]);
    cmd.argv[0] = "/nonexistent/marsad-test-ssh".into();
    match run(rig.sign_in(cmd)) {
        SignInStep::Unstarted(why) => assert!(why.contains("marsad-test-ssh"), "{why}"),
        other => panic!("expected Unstarted, got {other:?}"),
    }
}
