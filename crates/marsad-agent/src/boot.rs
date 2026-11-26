//! `marsad-agent boot`: the host's end of a hub's bootstrap, once a helper of the hub's build is
//! installed (see [`crate::bootwire`] for the protocol, `hub::transport` for how it is started).
//!
//! It reports what the host has — this helper's build and hash, and the daemon of this account:
//! whether one answers, its build, host id, pid and the hub it serves — reads the hub's one
//! decision, carries it out on this connection (a restart stops the running daemon and waits
//! for its exit, however long that takes), makes sure a daemon runs (a start waits for its
//! readiness line, however long the host's login environment takes), says `MARSAD-OK` with what
//! it did, and then is the bridge on the same stdio. Nothing here retries or waits for a time.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use marsad_proto::daemon::DaemonInfo;
use marsad_proto::methods;
use tokio::io::AsyncReadExt;
use wasit_control::{ClientOptions, ControlClient, Hello};

use crate::bootwire::{self, DaemonState, Decision, Report};
use crate::cli::Options;
use crate::serve::{self, Started};
use crate::version;

/// The OS word the scripts report (`uname -s`, or `Windows`).
fn os_word() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "Darwin",
        "windows" => "Windows",
        other => other,
    }
}

/// One protocol line on stdout (stdout is the hub's connection: nothing else is printed there).
fn say(line: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(format!("{line}\n").as_bytes());
    let _ = out.flush();
}

fn failed(stage: &str, reason: &str) -> ExitCode {
    eprintln!("marsad-agent boot: {stage}: {reason}");
    say(&bootwire::fail_line(stage, reason));
    ExitCode::from(1)
}

/// One line from the hub, read byte by byte (nothing past it is taken from the connection);
/// `None`: the hub closed it (it refused to go on).
async fn read_line() -> Option<String> {
    let mut stdin = tokio::io::stdin();
    let mut line = Vec::new();
    loop {
        let mut b = [0u8; 1];
        match stdin.read(&mut b).await {
            Ok(0) | Err(_) => return None,
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => line.push(b[0]),
        }
        if line.len() > 4096 {
            return None;
        }
    }
    Some(
        String::from_utf8_lossy(&line)
            .trim_end_matches('\r')
            .to_owned(),
    )
}

/// The daemon of this account, as its endpoint answers.
async fn look_at_daemon(o: &Options, report: &mut Report) {
    let opts = ClientOptions::new(Hello::new(version::build_id(), "marsad-agent-boot", "cli"));
    match ControlClient::connect_endpoint(&o.endpoint(), opts).await {
        Ok(client) => {
            report.daemon = DaemonState::Running;
            let hello = client.connection().remote_hello().clone();
            report.daemon_build = Some(hello.build.clone());
            report.daemon_host = Some(hello.host_id.to_string());
            if let Ok(info) = client
                .request::<DaemonInfo>(methods::DAEMON_INFO, (), std::time::Duration::from_secs(30))
                .await
            {
                report.daemon_pid = Some(info.pid);
                report.daemon_hub = info.hub_id;
            }
            client.close();
        }
        Err(wasit_control::ConnectError::Io(_)) => report.daemon = DaemonState::Absent,
        Err(_) => report.daemon = DaemonState::Unreachable,
    }
}

pub async fn boot(mut o: Options) -> ExitCode {
    // What the daemon this starts is given: everything but this run's own flag.
    o.raw.retain(|a| a != "--installed");
    let exe = std::env::current_exe().ok();
    let hash = match exe.clone() {
        Some(p) => tokio::task::spawn_blocking(move || bootwire::sha256_file(&p).ok())
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let mut report = Report {
        os: os_word().to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        helper_build: Some(version::build_id()),
        helper_hash: hash,
        hasher: None,
        daemon: DaemonState::Absent,
        daemon_build: None,
        daemon_host: None,
        daemon_pid: None,
        daemon_hub: None,
        machine: Some(o.layout().machine.clone()),
        installed: o.installed,
    };
    look_at_daemon(&o, &mut report).await;
    say(&report.line());
    let Some(line) = read_line().await else {
        // The hub refused to go on (it said why on its side) or the connection ended.
        return ExitCode::from(1);
    };
    let stop = match Decision::parse(&line) {
        Some(Decision::Run) => "none".to_owned(),
        Some(Decision::Restart(_)) => {
            let stopped = serve::stop_daemon(&o).await;
            if !stopped.done() {
                return failed("stop", &stopped.words.join("; "));
            }
            "stopped".to_owned()
        }
        Some(Decision::Upload { .. }) => {
            return failed("decision", "this helper is installed; nothing to upload")
        }
        None => return failed("decision", &format!("unknown decision {line:?}")),
    };
    // The daemon is ready before the connection becomes the bridge's: a running one answered
    // above; one started here printed its readiness line. No deadline: a slow login environment
    // is waited for: the wait ends with the daemon's readiness line or its exit. (Closing the
    // hub's connection does not end it: nothing reads the connection before the OK line, and
    // the daemon is detached on purpose.) Progress goes to stderr, as `stop`'s does.
    let running = report.daemon == DaemonState::Running && stop == "none";
    let daemon = if running {
        "running"
    } else {
        eprintln!("waiting for the daemon to be ready");
        match serve::start_daemon_process(o.raw.clone(), o.layout()).await {
            Ok(Started::Ready) => "started",
            // Another start won the lock meanwhile: it serves (the bridge connects to it).
            Ok(Started::AnotherWon) => "running",
            Err(f) => return failed("daemon", &f.cause.to_string()),
        }
    };
    if let Some(exe) = exe {
        cleanup(&exe);
    }
    say(&bootwire::format(
        bootwire::OK,
        &[
            ("stop", Some(&stop)),
            ("install", Some(if o.installed { "ok" } else { "none" })),
            ("daemon", Some(daemon)),
        ],
    ));
    serve::bridge(o).await
}

/// The helper directories of the account: `<bin>/<build>/<os>-<arch>/<exe>`, or `None` when this
/// executable is not in that shape (a development or test build run from elsewhere: nothing is
/// cleaned there).
fn helper_bin(exe: &Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let platform = exe.parent()?;
    let build = platform.parent()?;
    let bin = build.parent()?;
    let named = |p: &Path, n: &str| p.file_name().and_then(|f| f.to_str()) == Some(n);
    let home = bin.parent()?;
    (named(bin, "bin") && (named(home, ".marsad") || named(home, "Marsad"))).then(|| {
        (
            platform.to_path_buf(),
            build.to_path_buf(),
            bin.to_path_buf(),
        )
    })
}

/// Removes what earlier bootstraps left: unfinished uploads next to this helper, every build's
/// directory but this one and the most recent other one (two builds are kept: a second hub
/// version, or a machine of a shared home still running the previous one), the Windows boot
/// programs (compiled again when an upload needs one) and the flat layout of builds before
/// per-build directories. Best effort: what cannot be removed (a running program on Windows)
/// stays for the next time; nothing waits or retries.
fn cleanup(exe: &Path) {
    let Some((platform, build, bin)) = helper_bin(exe) else {
        return;
    };
    for e in std::fs::read_dir(&platform).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if abandoned_upload(&name) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let mut others: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(&bin).into_iter().flatten().flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            if path != build {
                let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                others.push((at, path));
            }
        } else if name.starts_with("marsad-boot-") || name.starts_with("marsad-agent") {
            let _ = std::fs::remove_file(&path);
        }
    }
    others.sort_by_key(|o| std::cmp::Reverse(o.0));
    for (_, dir) in others.into_iter().skip(1) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Whether `name` is an upload that nobody is writing any more. A home may be shared by
/// several machines whose bootstraps install into the same directory at once, so a temporary
/// file is only known abandoned when it is this machine's (`marsad-agent.new.<host>.<pid>`, the
/// POSIX script's name) and its writer is gone. Linux answers that from `/proc`; elsewhere a
/// POSIX leftover is kept (its build directory goes with the keep-two rule), and on Windows the
/// boot program's file (`marsad-agent.exe.new.<pid>`) cannot be removed while it is written, so
/// trying is safe.
fn abandoned_upload(name: &str) -> bool {
    if cfg!(windows) {
        return name.starts_with("marsad-agent.exe.new.");
    }
    let Some(rest) = name.strip_prefix("marsad-agent.new.") else {
        return false;
    };
    let Some((host, pid)) = rest.rsplit_once('.') else {
        return false;
    };
    if !cfg!(target_os = "linux") || pid.parse::<u32>().is_err() {
        return false;
    }
    let ours = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
    host == ours.trim() && !Path::new("/proc").join(pid).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_helper_layout_is_cleaned() {
        let root = std::env::temp_dir().join(format!("mrs-boot-clean-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join(".marsad").join("bin");
        let mk = |rel: &str, file: &str| {
            let d = bin.join(rel);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), b"x").unwrap();
            d.join(file)
        };
        let old1 = mk("b1/Linux-x86_64", "marsad-agent");
        // Ordered by modification time: b2 newer than b1, b3 the newest other.
        let set = |p: &Path, s: u64| {
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(s);
            std::fs::File::options()
                .write(true)
                .open(p)
                .unwrap()
                .set_modified(t)
                .unwrap();
        };
        let old2 = mk("b2/Linux-x86_64", "marsad-agent");
        let legacy = mk("Linux-x86_64", "marsad-agent");
        let me = mk("cur/Linux-x86_64", "marsad-agent");
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();
        // A writer that is gone (pid beyond the kernel's maximum), and one that is alive (us).
        let leftover = mk(
            "cur/Linux-x86_64",
            &format!("marsad-agent.new.{}.4194305", host.trim()),
        );
        let live = mk(
            "cur/Linux-x86_64",
            &format!("marsad-agent.new.{}.{}", host.trim(), std::process::id()),
        );
        let other_machine = mk("cur/Linux-x86_64", "marsad-agent.new.elsewhere.4194305");
        std::fs::write(bin.join("marsad-boot-abc.exe"), b"x").unwrap();
        for (d, s) in [("b1", 10), ("b2", 30), ("Linux-x86_64", 20)] {
            let dir = std::fs::File::open(bin.join(d)).unwrap();
            dir.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(s))
                .unwrap();
        }
        set(&old1, 1);
        cleanup(&me);
        assert!(me.exists(), "this helper stays");
        if cfg!(target_os = "linux") {
            assert!(!leftover.exists(), "an abandoned upload goes");
        }
        assert!(live.exists(), "an upload being written stays");
        assert!(other_machine.exists(), "another machine's upload stays");
        assert!(old2.exists(), "the most recent other build stays");
        assert!(!old1.exists() && !legacy.exists(), "older ones go");
        assert!(!bin.join("marsad-boot-abc.exe").exists());
        // Not in the helper layout: nothing is touched.
        let elsewhere = root
            .join("target")
            .join("debug")
            .join("x")
            .join("marsad-agent");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        assert!(helper_bin(&elsewhere).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
