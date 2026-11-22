#![allow(dead_code)]
//! Shared harness: short temp dirs, the real `marsad-agent` and `wasit-fake-cli` binaries,
//! clients, bounded waits, git repos, and a real sshd on localhost for remote hosts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use marsad_client::{ClientConfig, MarsadClient, Subscription};
use serde::de::DeserializeOwned;
use wasit_control::Hello;
use wasit_fake_cli::scenario::Scenario;

pub const T: Duration = Duration::from_secs(60);
pub const DRIVERS: [&str; 4] = ["claude", "codex", "opencode", "kilo"];

/// Short temp dir (Unix socket paths are limited to ~104 bytes).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("mrs-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

/// Kill every process whose command line mentions `needle` (test cleanup). On Windows the
/// needle is matched as a plain substring (PowerShell `-like`), not a regex.
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
/// it is killed there). A missing process is fine.
pub fn stop_pid(pid: u32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .stderr(Stdio::null())
        .status();
    #[cfg(windows)]
    kill_pid(pid);
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Daemons, shims and fake CLIs started under this dir.
        pkill(&self.0.to_string_lossy());
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

pub fn client(data: &Path) -> MarsadClient {
    let mut cfg = ClientConfig::new(data, Hello::new("test", "gui-test", "gui"));
    cfg.agent_bin = Some(agent());
    cfg.serve_args = serve_args();
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    cfg.supervise.backoff.max = Duration::from_millis(500);
    MarsadClient::connect(cfg)
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
        child: std::process::Child,
    }

    impl Drop for Sshd {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
            let (hostkey, key, wrong) = (dir.join("hostkey"), dir.join("id"), dir.join("wrong"));
            keygen(&hostkey);
            keygen(&key);
            keygen(&wrong);
            let pubkey = std::fs::read_to_string(key.with_extension("pub")).unwrap();
            let path = format!("{}:/usr/local/bin:/usr/bin:/bin", fakebin.display());
            let ak = if env_home {
                format!(
                    "environment=\"HOME={}\",environment=\"PATH={path}\" {pubkey}",
                    remote_home.display()
                )
            } else {
                pubkey.clone()
            };
            std::fs::write(dir.join("authorized_keys"), ak).unwrap();
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
            let _ = std::fs::create_dir_all("/run/sshd");
            let child = std::process::Command::new(&sshd)
                .args(["-D", "-e", "-f"])
                .arg(dir.join("sshd_config"))
                .stdout(Stdio::null())
                .stderr(std::fs::File::create(dir.join("sshd.log")).unwrap())
                .spawn()
                .unwrap();
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
            // Readiness: connect with a backoff schedule until sshd listens.
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
                child,
            }
        }

        /// One "machine" of several that share the home directory `home`: its own machine key
        /// and runtime directory (the overrides Marsad honours), the same `HOME`.
        pub fn start_machine(dir: &Path, name: &str, home: &Path, machine: &str) -> Sshd {
            let mut s = Self::start_custom(dir, name, "", true);
            let run = s.dir.join("run");
            let pubkey = std::fs::read_to_string(s.key.with_extension("pub")).unwrap();
            let path = format!(
                "{}:/usr/local/bin:/usr/bin:/bin",
                s.dir.join("bin").display()
            );
            std::fs::write(
                s.dir.join("authorized_keys"),
                format!(
                    "environment=\"HOME={}\",environment=\"PATH={path}\",\
                     environment=\"MARSAD_MACHINE_KEY={machine}\",\
                     environment=\"MARSAD_RUNTIME_DIR={}\" {pubkey}",
                    home.display(),
                    run.display()
                ),
            )
            .unwrap();
            s.remote_home = home.to_path_buf();
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
