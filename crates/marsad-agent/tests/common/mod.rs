#![allow(dead_code)]
//! Shared harness: short temp dirs, the real `marsad-agent` and `wasit-fake-cli` binaries,
//! clients, bounded waits, git repos, and a real sshd on localhost for remote hosts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use marsad_client::{ClientConfig, MarsadClient, Subscription};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::AuthPrompt;
use marsad_proto::layout::Layout;
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, LinkState};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::AsyncBufReadExt;
use wasit_control::Hello;
use wasit_fake_cli::scenario::Scenario;

pub const T: Duration = Duration::from_secs(60);
pub const DRIVERS: [&str; 4] = ["claude", "codex", "opencode", "kilo"];

/// Short temp dir (Unix socket paths are limited to ~104 bytes). Everything a test starts lives
/// below it: the data roots, the daemons' runtime directories ([`run_dir`]), the sshd of the
/// "remote" machines. Dropping it ends what is still running below it and removes it.
pub struct TempDir(pub PathBuf);

/// The prefix of a [`TempDir`]'s name.
const TEMP_PREFIX: &str = "mrs-";

impl TempDir {
    pub fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{n:03}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Kill every process whose command line matches `needle`. Only for a test that names a process
/// by a command line that is its own (the ssh to its sshd's port): ending what a test started is
/// `TempDir`'s drop, never this. On Windows the needle is matched as a plain substring
/// (PowerShell `-like`), not a regex.
pub fn pkill(needle: &str) {
    #[cfg(unix)]
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-f", needle])
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    {
        let needle = needle.replace('\'', "''");
        let script = format!(
            "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{needle}*' -and $_.ProcessId -ne $PID }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Kill a process outright (`kill -9` / `taskkill /F`); a missing process is fine.
pub fn kill_pid(pid: u32) {
    #[cfg(unix)]
    let mut cmd = {
        let mut c = std::process::Command::new("kill");
        c.args(["-9", &pid.to_string()]);
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("taskkill");
        c.args(["/F", "/PID", &pid.to_string()]);
        c
    };
    let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Ask a daemon to stop (`SIGTERM`; Windows has no polite signal for a detached process, so
/// it is killed there). A missing process is fine. Tests use [`stop_daemon`], which also waits
/// for the end.
fn stop_pid(pid: u32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    kill_pid(pid);
}

/// Stops the daemon `pid` and returns when it is GONE: its endpoint and lock are released and
/// nothing can reach it any more, so what a test does next (a client that starts the next hub,
/// a call) can only meet its successor. The wait is the process's own end, a handle opened
/// before the signal (a pidfd: it cannot be fooled by a recycled pid), bounded only to turn a
/// hang into a failure. Where no such handle exists (not Linux) it is the kill of
/// [`stop_pid`] that has to be complete when it returns. The one way tests stop a daemon: the
/// hub ([`stop_hub`]), a host's daemon or a daemon the test started itself.
pub async fn stop_daemon(pid: u32) {
    let watch = wasit_fake_cli::harness::PidWatch::open(pid);
    stop_pid(pid);
    if let Some(w) = watch {
        assert!(
            w.exited(T).await,
            "the daemon {pid} did not exit after it was asked to stop"
        );
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if std::env::var_os("MARSAD_TEST_KEEP").is_some() {
            return;
        }
        // Daemons and session shims (with their CLIs) started anywhere below this directory
        // outlive their starters on purpose: end them before their files go.
        wasit_fake_cli::harness::stop_all_under(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn agent() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_marsad-agent"))
}

/// The fake CLI binary, building it once if this test run did not.
pub fn fake() -> PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        if let Some(p) = wasit_fake_cli::fake_cli_path() {
            return p;
        }
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = std::process::Command::new(cargo)
            .args(["build", "-q", "-p", "marsad-fake-cli"])
            .status()
            .expect("run cargo build for the fake CLI");
        assert!(st.success(), "building wasit-fake-cli failed");
        wasit_fake_cli::fake_cli_path().expect("fake CLI built")
    })
    .clone()
}

/// `serve` options pointing every driver at the fake CLI.
pub fn serve_args() -> Vec<String> {
    let mut v = vec![
        "--stop-grace-ms".to_string(),
        "500".into(),
        "--host-id".into(),
        "hub".into(),
    ];
    for d in DRIVERS {
        v.push("--driver-bin".into());
        v.push(format!("{d}={}", fake().display()));
        v.push("--driver-env".into());
        v.push(format!("{d}=WASIT_FAKE_PERSONA={d}"));
    }
    v
}

/// The runtime directory of the daemon of `data`: inside the test's own [`TempDir`], so it goes
/// with it, its socket paths stay short, and nothing lands in the machine's runtime directory,
/// where the real installation's daemon lives. The tests give it to the clients that start a hub
/// ([`client_config`]), to the daemons they start (`--run-dir`, [`data_args`]) and to the layouts
/// they resolve ([`layout`]). (The "remote" machines get theirs from `MARSAD_RUNTIME_DIR` in
/// their own login environment; a variable of the test process would be shared by parallel
/// tests.)
pub fn run_dir(data: &Path) -> PathBuf {
    let temp = data
        .ancestors()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(TEMP_PREFIX))
        })
        .unwrap_or_else(|| panic!("{} is not inside a TempDir", data.display()));
    let rel = data.strip_prefix(temp).unwrap();
    let name = rel.to_string_lossy().replace(['/', '\\'], "_");
    temp.join("run")
        .join(if name.is_empty() { "root" } else { &name })
}

/// The options that say where the daemon of `data` keeps its files (`serve`, `bridge`, `stop`,
/// `diagnostics export`): `--data <data>` and `--run-dir`.
pub fn data_args(data: &Path) -> Vec<String> {
    vec![
        "--data".to_string(),
        data.display().to_string(),
        "--run-dir".into(),
        run_dir(data).display().to_string(),
    ]
}

/// A client that starts, on demand, the hub daemon of `data` with the daemon binary of this
/// build, its runtime directory [`run_dir`]. The caller adds the daemon's options to
/// `serve_args`.
pub fn client_config(data: impl AsRef<Path>, hello: Hello) -> ClientConfig {
    let data = data.as_ref();
    let mut cfg = ClientConfig::new(data, hello);
    cfg.run_dir = Some(run_dir(data));
    cfg.agent_bin = Some(agent());
    cfg
}

pub fn client(data: &Path) -> MarsadClient {
    client_with(data, true)
}

/// A client of a daemon the test started itself: it never starts one, so ending that daemon does
/// not bring up a successor behind the test's back. Otherwise the same as [`client`].
pub fn client_of(data: &Path) -> MarsadClient {
    client_with(data, false)
}

fn client_with(data: &Path, start_daemon: bool) -> MarsadClient {
    let mut cfg = client_config(data, Hello::new("test", "gui-test", "gui"));
    cfg.start_daemon = start_daemon;
    cfg.serve_args = serve_args();
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    cfg.supervise.backoff.max = Duration::from_millis(500);
    MarsadClient::connect(cfg)
}

/// A hub daemon (started on demand by its client) under `dir` with `extra` serve options, whose
/// client retries quickly.
pub fn hub_client(dir: &Path, extra: &[&str]) -> MarsadClient {
    let mut cfg = client_config(dir, Hello::new("test", "gui", "gui"));
    cfg.serve_args = serve_args();
    cfg.serve_args.extend(extra.iter().map(|s| s.to_string()));
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    MarsadClient::connect(cfg)
}

/// A hub daemon of `data` that the test starts itself (so its exit can be awaited and it is
/// ended by [`end_own_daemon`]), with the options of [`data_args`] and [`serve_args`]. Its
/// clients are [`client_of`]s.
pub async fn own_daemon(data: &Path) -> tokio::process::Child {
    let mut child = tokio::process::Command::new(agent())
        .arg("serve")
        .arg("--hub")
        .args(data_args(data))
        .args(serve_args())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut line = String::new();
    tokio::time::timeout(
        T,
        tokio::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
    )
    .await
    .expect("the daemon said it is ready")
    .unwrap();
    assert!(line.contains("ready"), "{line}");
    child
}

/// Ends a daemon of [`own_daemon`] and returns when it is gone and reaped.
pub async fn end_own_daemon(mut daemon: tokio::process::Child) {
    stop_daemon(daemon.id().unwrap()).await;
    daemon.wait().await.unwrap();
}

/// Wait on the fleet until `pred` matches an event.
pub async fn until(
    f: &mut Subscription<FleetEvent>,
    what: &str,
    pred: impl Fn(&FleetEvent) -> bool,
) -> FleetEvent {
    let fut = async {
        loop {
            let (_, e) = next(f).await;
            if pred(&e) {
                return e;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(120), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Whether `e` is a link event of `host` whose state satisfies `p`.
pub fn link_is(e: &FleetEvent, host: &str, p: impl Fn(&LinkState) -> bool) -> bool {
    matches!(e, FleetEvent::Link { host: h, state } if h.as_str() == host && p(state))
}

/// The next sign-in prompt of `host`.
pub async fn prompt(f: &mut Subscription<FleetEvent>, host: &str) -> AuthPrompt {
    match until(
        f,
        "a sign-in prompt",
        |e| matches!(e, FleetEvent::AuthPrompt { host: h, prompt: Some(_) } if h.as_str() == host),
    )
    .await
    {
        FleetEvent::AuthPrompt {
            prompt: Some(p), ..
        } => p,
        _ => unreachable!(),
    }
}

/// Stops the hub daemon `c` talks to and returns when it is gone ([`stop_daemon`]): the next
/// call of `c` (or of a client of the same data) reaches the hub its client starts after it,
/// never this one on its way out. The single way a test restarts or ends a hub.
pub async fn stop_hub(c: &MarsadClient) {
    let pid = c
        .daemon_info(None)
        .await
        .expect("the hub to stop answers")
        .pid;
    stop_daemon(pid).await;
}

/// Write a scenario next to `dir` and return the env that makes the fake CLI play it.
pub fn scenario_env(dir: &Path, sc: &Scenario) -> Vec<(String, String)> {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = dir.join(format!(
        "scenario-{}.json",
        N.fetch_add(1, Ordering::Relaxed)
    ));
    sc.write_to(&p).unwrap();
    vec![(
        "WASIT_FAKE_SCENARIO".into(),
        p.to_string_lossy().into_owned(),
    )]
}

/// Next item of a subscription (bounded).
pub async fn next<T: DeserializeOwned>(s: &mut Subscription<T>) -> (wasit_control::Cursor, T) {
    match tokio::time::timeout(T, s.next()).await {
        Ok(Some(Ok(x))) => x,
        Ok(Some(Err(e))) => panic!("subscription error: {e}"),
        Ok(None) => panic!("subscription ended"),
        Err(_) => panic!("timed out waiting for an event"),
    }
}

/// Run a command, panicking on failure.
pub fn sh(dir: &Path, argv: &[&str]) -> String {
    let out = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{argv:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A git repo with one commit (`a.txt`, `b.txt`).
pub fn git_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    sh(dir, &["git", "init", "-q", "-b", "main"]);
    sh(dir, &["git", "config", "user.email", "t@example.com"]);
    sh(dir, &["git", "config", "user.name", "T"]);
    // Byte-exact contents on every platform (Git for Windows defaults to autocrlf).
    sh(dir, &["git", "config", "core.autocrlf", "false"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("b.txt"), "keep\n").unwrap();
    sh(dir, &["git", "add", "."]);
    sh(dir, &["git", "commit", "-q", "-m", "init"]);
}

// ── a real sshd on localhost (Unix: the remote-host tests) ─────────────────

#[cfg(unix)]
mod sshd {
    use super::*;

    /// An sshd on a free localhost port with a throwaway host key; the authorized key logs in
    /// with a separate `HOME` (the "remote" machine) and a `PATH` whose `claude` is the fake CLI.
    pub struct Sshd {
        pub dir: PathBuf,
        pub port: u16,
        pub user: String,
        pub key: PathBuf,
        pub wrong_key: PathBuf,
        pub known_hosts: PathBuf,
        pub remote_home: PathBuf,
        /// What an `authorized_keys` line starts with for a key that logs in as the "remote"
        /// machine (its `HOME`, `PATH`, shell and runtime directory), or nothing.
        authorized_prefix: String,
        sshd_bin: PathBuf,
        /// The listener; `None` while [`Sshd::stop`] has it down.
        child: Option<std::process::Child>,
    }

    impl Drop for Sshd {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn keygen(path: &Path) {
        let st = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(path)
            .status()
            .expect("ssh-keygen (install openssh-client)");
        assert!(st.success());
    }

    impl Sshd {
        pub fn start(dir: &Path) -> Sshd {
            Self::start_custom(dir, "ssh", "", true)
        }

        /// `extra` sshd_config lines take precedence (sshd uses the first value of a keyword);
        /// `env_home`: the authorized key logs in with the separate remote HOME and fake PATH.
        pub fn start_custom(dir: &Path, name: &str, extra: &str, env_home: bool) -> Sshd {
            let sshd = ["/usr/sbin/sshd", "/usr/local/sbin/sshd"]
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
                .expect("remote-host tests need sshd (apt-get install openssh-server)");
            let dir = dir.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let remote_home = dir.parent().unwrap().join(format!("rh-{name}"));
            std::fs::create_dir_all(&remote_home).unwrap();
            let fakebin = dir.join("bin");
            std::fs::create_dir_all(&fakebin).unwrap();
            std::os::unix::fs::symlink(fake(), fakebin.join("claude")).unwrap();
            // The "remote" user's login shell: runs the command without any profile, so the
            // environment stage keeps this test's PATH (the machine's /etc/profile would put a
            // real `claude` first).
            std::fs::write(
                fakebin.join("loginsh"),
                "#!/bin/sh\nwhile [ $# -gt 1 ]; do shift; done\nexec /bin/sh -c \"$1\"\n",
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    fakebin.join("loginsh"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
            let (hostkey, key, wrong) = (dir.join("hostkey"), dir.join("id"), dir.join("wrong"));
            keygen(&hostkey);
            keygen(&key);
            keygen(&wrong);
            let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
            let path = format!("{}:/usr/local/bin:/usr/bin:/bin", fakebin.display());
            let authorized_prefix = if env_home {
                // The "remote" machine's daemons keep their sockets in the test's own directory.
                format!(
                    "environment=\"HOME={}\",environment=\"PATH={path}\",\
                     environment=\"SHELL={}\",environment=\"MARSAD_RUNTIME_DIR={}\" ",
                    remote_home.display(),
                    fakebin.join("loginsh").display(),
                    dir.join("run").display()
                )
            } else {
                String::new()
            };
            std::fs::write(
                dir.join("authorized_keys"),
                format!("{authorized_prefix}{pubkey}"),
            )
            .unwrap();
            let port = {
                let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port()
            };
            std::fs::write(
                dir.join("sshd_config"),
                format!(
                    "{extra}\nPort {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\n\
                     PermitUserEnvironment yes\nStrictModes no\nPasswordAuthentication no\n\
                     KbdInteractiveAuthentication no\nUsePAM no\nPidFile {}\nLogLevel ERROR\n",
                    hostkey.display(),
                    dir.join("authorized_keys").display(),
                    dir.join("sshd.pid").display()
                ),
            )
            .unwrap();
            let child = Self::spawn(&sshd, &dir);
            let hostpub = std::fs::read_to_string(hostkey.with_extension("pub")).unwrap();
            let mut parts = hostpub.split_whitespace();
            let known_hosts = dir.join("known_hosts");
            std::fs::write(
                &known_hosts,
                format!(
                    "[127.0.0.1]:{port} {} {}\n",
                    parts.next().unwrap(),
                    parts.next().unwrap()
                ),
            )
            .unwrap();
            Self::await_listening(port, &dir);
            let user = String::from_utf8(
                std::process::Command::new("id")
                    .arg("-un")
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_string();
            Sshd {
                dir,
                port,
                user,
                key,
                wrong_key: wrong,
                known_hosts,
                remote_home,
                authorized_prefix,
                sshd_bin: sshd,
                child: Some(child),
            }
        }

        fn spawn(sshd: &Path, dir: &Path) -> std::process::Child {
            let _ = std::fs::create_dir_all("/run/sshd");
            std::process::Command::new(sshd)
                .args(["-D", "-e", "-f"])
                .arg(dir.join("sshd_config"))
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(dir.join("sshd.log")).unwrap())
                .spawn()
                .unwrap()
        }

        /// Readiness: connect with a backoff schedule until sshd listens.
        fn await_listening(port: u16, dir: &Path) {
            let mut delay = Duration::from_millis(5);
            let mut up = false;
            for _ in 0..12 {
                if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                    up = true;
                    break;
                }
                std::thread::sleep(delay);
                delay *= 2;
            }
            assert!(
                up,
                "sshd did not start: {}",
                std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default()
            );
        }

        /// The runtime directory base of the "remote" machine's daemons (what its logins get as
        /// `MARSAD_RUNTIME_DIR`): inside the test's own directory.
        pub fn runtime_dir(&self) -> PathBuf {
            self.dir.join("run")
        }

        /// The pid of the listener (it must be running).
        pub fn pid(&self) -> u32 {
            self.child.as_ref().expect("sshd is running").id()
        }

        /// Takes the listener down (this test's own sshd, by its child handle): nothing listens
        /// on the port until [`Sshd::start_again`]. Sessions it already has are its children's.
        pub fn stop(&mut self) {
            if let Some(mut c) = self.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }

        /// Brings the listener back on the same port with the same keys.
        pub fn start_again(&mut self) {
            assert!(self.child.is_none(), "sshd is running");
            self.child = Some(Self::spawn(&self.sshd_bin, &self.dir));
            Self::await_listening(self.port, &self.dir);
        }

        /// A new key with `passphrase` that logs in as the "remote" machine: a host that uses
        /// it must ask the user for the passphrase at every sign-in (the host's ssh options
        /// should keep ssh from using or filling an agent: `IdentityAgent=none`,
        /// `AddKeysToAgent=no`).
        pub fn passphrase_key(&self, passphrase: &str) -> PathBuf {
            let key = self.dir.join("id-passphrase");
            let st = std::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
                .arg(&key)
                .status()
                .expect("ssh-keygen");
            assert!(st.success());
            let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
            let path = self.dir.join("authorized_keys");
            let mut lines = std::fs::read_to_string(&path).unwrap();
            lines.push_str(&format!("{}{pubkey}", self.authorized_prefix));
            std::fs::write(&path, lines).unwrap();
            key
        }

        /// One "machine" of several that share the home directory `home`: its own machine key
        /// and runtime directory (the overrides Marsad honours), the same `HOME`.
        pub fn start_machine(dir: &Path, name: &str, home: &Path, machine: &str) -> Sshd {
            let mut s = Self::start_custom(dir, name, "", true);
            let run = s.runtime_dir();
            let pubkey = std::fs::read_to_string(s.key.with_extension("pub")).unwrap();
            let path = format!(
                "{}:/usr/local/bin:/usr/bin:/bin",
                s.dir.join("bin").display()
            );
            std::fs::write(
                s.dir.join("authorized_keys"),
                format!(
                    "environment=\"HOME={}\",environment=\"PATH={path}\",\
                     environment=\"SHELL={}/bin/loginsh\",\
                     environment=\"MARSAD_MACHINE_KEY={machine}\",\
                     environment=\"MARSAD_RUNTIME_DIR={}\" {pubkey}",
                    home.display(),
                    s.dir.display(),
                    run.display()
                ),
            )
            .unwrap();
            s.remote_home = home.to_path_buf();
            s.authorized_prefix = format!(
                "environment=\"HOME={}\",environment=\"PATH={path}\",\
                 environment=\"SHELL={}/bin/loginsh\",\
                 environment=\"MARSAD_MACHINE_KEY={machine}\",\
                 environment=\"MARSAD_RUNTIME_DIR={}\" ",
                home.display(),
                s.dir.display(),
                run.display()
            );
            s
        }

        /// The host kind for this sshd, logging in with `key`.
        pub fn kind(&self, key: &Path) -> marsad_proto::hosts::HostKind {
            marsad_proto::hosts::HostKind::Ssh {
                target: format!("{}@127.0.0.1", self.user),
                port: Some(self.port),
                identity_file: Some(key.display().to_string()),
                options: vec![
                    format!("UserKnownHostsFile={}", self.known_hosts.display()),
                    "StrictHostKeyChecking=yes".into(),
                    "GlobalKnownHostsFile=/dev/null".into(),
                ],
            }
        }

        pub fn installed_agent(&self) -> PathBuf {
            self.remote_home.join(common_bin_rel())
        }
    }
}
#[cfg(unix)]
#[allow(unused_imports)]
pub use sshd::*;

/// Where the bootstrap installs the helper under the remote home (POSIX hosts).
#[allow(dead_code)]
pub fn common_bin_rel() -> String {
    let os = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    format!(".marsad/bin/{os}-{}/marsad-agent", std::env::consts::ARCH)
}

/// The layout of `data` with its daemon's runtime directory ([`run_dir`]).
pub fn layout(data: &Path) -> Layout {
    Layout::resolve(data.to_path_buf(), Some(run_dir(data))).unwrap()
}

/// Where `Layout::resolve(root, None)` puts the runtime directory of the data root `root`: the
/// place a daemon nobody told its runtime directory keeps its sockets, next to the real
/// installation's. Only computed (the base directory is the machine's, which exists), never
/// created, listed or touched; tests assert that it does not exist. The tag formula is checked
/// against the product in `tests/no_leftovers.rs`.
pub fn default_run_dir(root: &Path) -> PathBuf {
    let base_env = std::env::var_os(marsad_proto::layout::RUNTIME_DIR_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let base = wasit_runtime::machine::runtime_dir("marsad", base_env.as_deref()).unwrap();
    let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let tag = wasit_runtime::machine::key_from_raw(&format!("root:{}", abs.display()));
    base.join(&tag[..8])
}

/// Every line of an NDJSON diagnostics file, parsed (a line that is not JSON fails the test).
pub fn records(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|l| {
            serde_json::from_str(l)
                .unwrap_or_else(|e| panic!("{}: not JSON ({e}): {l:?}", path.display()))
        })
        .collect()
}

pub fn of<'a>(recs: &'a [Value], ev: &str) -> Vec<&'a Value> {
    recs.iter().filter(|r| r["ev"] == ev).collect()
}

pub async fn stop(data: &Path) -> std::process::Output {
    let out = tokio::time::timeout(
        T,
        tokio::process::Command::new(agent())
            .arg("stop")
            .args(data_args(data))
            .output(),
    )
    .await
    .expect("stop ended")
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Every file under `dir`, with its bytes.
pub fn files(dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if let Ok(b) = std::fs::read(&p) {
            out.push((p, b));
        }
    }
}

/// A marker is raw, JSON-escaped once and JSON-escaped twice (a CLI's JSON line travels inside
/// the journal's JSON).
pub fn forms(marker: &str) -> Vec<String> {
    let esc = |s: &str| {
        let q = serde_json::to_string(s).unwrap();
        q[1..q.len() - 1].to_string()
    };
    let once = esc(marker);
    let twice = esc(&once);
    vec![marker.to_string(), once, twice]
}

/// How `marker` can show in bytes: raw, JSON-escaped once or twice, base64 (at each of the three
/// alignments it can have inside a longer base64 text) and UTF-16LE. `Some(form)` of the first
/// that is in `bytes`.
pub fn found_in(bytes: &[u8], marker: &str) -> Option<&'static str> {
    use wasit_core::content::b64_encode;
    let text = String::from_utf8_lossy(bytes);
    let f = forms(marker);
    for (form, needle) in [
        ("raw", &f[0]),
        ("json-escaped", &f[1]),
        ("json-escaped twice", &f[2]),
    ] {
        if text.contains(needle.as_str()) {
            return Some(form);
        }
    }
    for k in 0..3 {
        let mut padded = vec![0u8; k];
        padded.extend_from_slice(marker.as_bytes());
        let enc = b64_encode(&padded);
        let part = &enc[[0, 2, 3][k]..enc.len() - 4];
        assert!(
            part.len() >= 8,
            "a marker too short to look for in base64: {marker}"
        );
        if text.contains(part) {
            return Some("base64");
        }
    }
    let wide: Vec<u8> = marker.encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes
        .windows(wide.len())
        .any(|w| w == wide.as_slice())
        .then_some("utf-16le")
}

pub fn contains_any(haystack: &str, marker: &str) -> bool {
    forms(marker).iter().any(|f| haystack.contains(f.as_str()))
}

pub fn is_turn_completed(e: &SessionEvent) -> bool {
    matches!(
        &e.kind,
        SessionEventKind::Agent {
            event: AgentEvent::TurnCompleted { .. }
        }
    )
}

/// The files that must never hold a marker: everything under `<data>/diag/`, the daemon's
/// stderr file and every shim log, each marker raw, JSON-escaped once and twice. `at_least`
/// diagnostics files must exist (so the scan cannot pass on nothing).
pub fn assert_no_markers(l: &Layout, markers: &[&str], at_least: usize) {
    let mut scanned = Vec::new();
    files(&l.diag(), &mut scanned);
    // An export is a gzip file: the text it holds is what must be free of the markers.
    for (path, bytes) in &mut scanned {
        if path.to_string_lossy().ends_with(".ndjson.gz") {
            *bytes =
                marsad_agent::hub::export::gzip::gunzip(&*bytes).expect("a well-formed gzip file");
        }
    }
    assert!(scanned.len() >= at_least, "{scanned:?}");
    if let Ok(b) = std::fs::read(l.data.join("marsad-agent.log")) {
        scanned.push((l.data.join("marsad-agent.log"), b));
    }
    if let Ok(rd) = std::fs::read_dir(l.data.join("sessions")) {
        for e in rd.flatten() {
            for f in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                let n = f.file_name().to_string_lossy().into_owned();
                if n.starts_with("shim.log") || n == "shim.stderr" {
                    scanned.push((f.path(), std::fs::read(f.path()).unwrap()));
                }
            }
        }
    }
    for (path, bytes) in &scanned {
        let text = String::from_utf8_lossy(bytes);
        for m in markers {
            assert!(
                !contains_any(&text, m),
                "marker {m} found in {}:\n{text}",
                path.display()
            );
        }
    }
}

/// An export as the hub reported it and as its file holds it.
pub mod exporting {
    use marsad_client::{ClientError, MarsadClient};
    use marsad_proto::export::{
        ExportRequest, ExportStarted, ExportState, ExportStatus, ExportsEvent,
    };
    use marsad_proto::{methods, topics};

    use super::*;

    pub struct Export {
        /// The last status the topic showed.
        pub status: ExportStatus,
        /// Every status of this export the topic showed, in order.
        pub seen: Vec<ExportStatus>,
        pub path: PathBuf,
        /// The gunzipped file.
        pub text: String,
        /// Its lines (the exporter's own have a `line` key, the records of a section do not).
        pub lines: Vec<Value>,
    }

    impl Export {
        pub fn own(&self, kind: &str) -> Vec<&Value> {
            self.lines.iter().filter(|l| l["line"] == kind).collect()
        }

        pub fn manifest(&self) -> &Value {
            assert_eq!(self.lines[0]["line"], "manifest", "the first line");
            &self.lines[0]
        }

        pub fn host(&self, id: &str) -> &Value {
            self.manifest()["hosts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|h| h["id"] == id)
                .unwrap_or_else(|| panic!("no host {id} in the manifest"))
        }

        pub fn end(&self) -> &Value {
            let last = self.lines.last().unwrap();
            assert_eq!(last["line"], "end", "the last line");
            last
        }

        /// The result of `source` in the end line.
        pub fn result(&self, source: &str) -> &Value {
            self.end()["sources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["source"] == source)
                .unwrap_or_else(|| panic!("no result for {source}"))
        }

        /// The records that follow the section line `pred` selects, up to the next line of the
        /// exporter's own.
        pub fn records_of(&self, pred: impl Fn(&Value) -> bool) -> Vec<&Value> {
            let Some(i) = self
                .lines
                .iter()
                .position(|l| l.get("line").is_some() && pred(l))
            else {
                return Vec::new();
            };
            self.lines[i + 1..]
                .iter()
                .take_while(|l| l.get("line").is_none())
                .collect()
        }

        /// The records of the section of `file` of `source` (one generation).
        pub fn section(&self, source: &str, file: &str) -> Vec<&Value> {
            self.records_of(|l| {
                l["line"] == "section" && l["source"] == source && l["file"] == file
            })
        }

        /// The entries of the trail of `session` on `source`.
        pub fn trail(&self, source: &str, session: &str) -> Vec<&Value> {
            self.records_of(|l| {
                l["line"] == "trail" && l["source"] == source && l["session"] == session
            })
        }
    }

    pub async fn start(c: &MarsadClient, req: ExportRequest) -> Result<ExportStarted, ClientError> {
        c.call_traced(None, methods::DIAGNOSTICS_EXPORT_START, req, None)
            .await
    }

    /// Follows `id` on the topic until it ends, then reads its file.
    pub async fn finish(
        sub: &mut marsad_client::Subscription<Value>,
        data: &Path,
        id: &str,
    ) -> Export {
        let mut seen: Vec<ExportStatus> = Vec::new();
        let status = loop {
            let (_, v) = next(sub).await;
            let statuses = match serde_json::from_value::<ExportsEvent>(v).unwrap() {
                ExportsEvent::Reset { exports } => exports,
                ExportsEvent::Upsert { export } => vec![export],
            };
            let mut ended = None;
            for s in statuses.into_iter().filter(|s| s.id == id) {
                if s.state != ExportState::Running {
                    ended = Some(s.clone());
                }
                seen.push(s);
            }
            if let Some(s) = ended {
                break s;
            }
        };
        assert_eq!(status.state, ExportState::Done, "{status:#?}");
        let file = status
            .file
            .clone()
            .expect("a finished export names its file");
        assert!(!file.contains(['/', '\\']), "{file}");
        let path = layout(data).diag().join("exports").join(&file);
        let gz = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(status.bytes, Some(gz.len() as u64));
        let text = String::from_utf8(
            marsad_agent::hub::export::gzip::gunzip(&gz).expect("a well-formed gzip file"),
        )
        .unwrap();
        let lines = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
            .collect();
        Export {
            status,
            seen,
            path,
            text,
            lines,
        }
    }

    /// Starts an export and waits for its end.
    pub async fn export(c: &MarsadClient, data: &Path, req: ExportRequest) -> Export {
        let mut sub = c.subscribe_raw(None, topics::DIAGNOSTICS_EXPORTS, None);
        let started = start(c, req).await.unwrap();
        finish(&mut sub, data, &started.id).await
    }
}

/// The panic message of an opt-in live test that was run without `MARSAD_LIVE=1`. Such a test
/// fails with it: an ignored test that passes without doing its work would pass silently.
pub fn live_hint(test_bin: &str) -> String {
    format!(
        "{test_bin} is a live test (it needs a logged-in claude CLI and spends model quota) and \
         did not run: set MARSAD_LIVE=1 and run `MARSAD_LIVE=1 cargo test -p marsad-agent --test \
         {test_bin} -- --ignored --nocapture`"
    )
}
