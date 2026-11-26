//! Connecting to and provisioning a host over ONE command per attempt (ARCHITECTURE §4.1,
//! with the owner's interactive-auth rule). An ssh host on a Linux or macOS hub has a shared
//! connection ([`super::master`]): that command is the link step, riding the connection, and
//! the attempt first looks for it and, if none is open and signing in is allowed, opens it in
//! a sign-in step where every prompt happens ([`BootstrapConnector::shared_connection`]).
//!
//! [`BootstrapConnector`] (the hub's `LinkConnector`) runs `<transport> <bootstrap>` (see
//! [`super::transport`]: one command line for every default shell) with `SSH_ASKPASS` pointing
//! at this binary and speaks the bootstrap's protocol ([`crate::bootwire`]) on that one
//! connection: the host's report (platform, installed helper, its daemon: build, host id, hub),
//! one decision ([`decide`]: run, restart, upload with SHA-256, or a refusal in words), the
//! host's `MARSAD-OK` once its daemon is ready, then the control protocol. A restart, an id
//! fix, an upgrade and Windows all happen on this connection: there is no second one, so a
//! user who signs in does so once. A failed attempt is classified from its structure
//! ([`failure_kind`]: the transport's exit status, what the user was asked, WSL's own
//! distribution list); the words the transport wrote are shown, and read only for ssh's two
//! fixed refusals that need a person ([`ssh_refusal`]: a changed host key, refused
//! authentication). So anything needing a human parks the link in `auth_needed` (or as a host
//! key problem); a refusal (another hub's daemon, no checksum tool, a failed install) parks it
//! as failed with its words.
//!
//! A host that needs the user at login (a connect asked them for something other than a
//! host-key confirmation, see [`marsad_proto::hosts::PromptKind::needs_user`]) is never tried
//! on the connector's own account: the link's supervisor asks [`BootstrapConnector::hold`]
//! before every attempt it would start by itself, and a host that needs the user and has not
//! been asked for (`hosts.reconnect` arms the next attempt) is parked in `auth_needed` instead,
//! unless the host's shared connection is still open. The click is spent when an attempt asks
//! the user something or connects (see [`Learned`]).
//!
//! Liveness is progress, not a clock (slow hosts must never fail): every step waits for the
//! host's next line or its end (ssh's keep-alives end a dead connection), the upload reports its
//! bytes, the host's daemon start waits for its readiness line; only sign-in prompts have a
//! deadline, the user's. Every attempt records its steps (`boot.*`, see [`Trail`]).
//!
//! Binaries: the host's (os, arch) picks the shipped helper triple
//! ([`super::transport::helper_target`]: any Linux → the static musl build); that is the
//! running executable when it was built for that triple, else
//! `MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]` (assumed to be the hub's build: a helper
//! that reports another is refused), else — development builds — the running executable when
//! it runs on the same (os, arch). No build for the platform, or a missing file, fails the link
//! (no retry loop) with a message saying what to do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use marsad_proto::daemon::VersionInfo;
use marsad_proto::diag::{BootShell, BootStage, HelperAction, HostsOp, MarsadEvent, MasterExitWhy};
use marsad_proto::hosts::{HostEntry, HostKind, HostPlatform};
use marsad_proto::HostId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use wasit_control::wire::RpcError;
use wasit_control::{
    Attempt, ChildHandle, ClientOptions, CommandFailureKind, CommandSpec, CommandTransport,
    ConnectDiag, ConnectError, ControlClient, HostSpec, LinkConnector, TransportKind,
    CLOSED_EXIT_WAIT,
};
use wasit_diag::{Bytes, Code, Ident, Ms};

use super::auth::{Asking, Prompts, SignInAttempt};
use super::master::{Check, Masters, SignInCommand, SignInStep};
use super::transport::{
    bootstrap_argv_via, helper_target, platform_from, posix_script, triple_platform, BootTarget,
    SshRoute,
};
use crate::bootwire::{self, DaemonState, Decision, Line, Report, RestartWhy};
use crate::diag::{self, emit};

/// Environment variable naming the directory with binaries for other target triples.
pub const BIN_DIR_ENV: &str = "MARSAD_AGENT_BIN_DIR";
/// Set for the askpass helper: which host's link is asking.
pub const ASKPASS_HOST_ENV: &str = "MARSAD_ASKPASS_HOST";
/// Set for the askpass helper: the connect attempt whose ssh runs it (see
/// [`Prompts::begin_attempt`]).
pub const ASKPASS_ATTEMPT_ENV: &str = "MARSAD_ASKPASS_ATTEMPT";
/// Set for the askpass helper: the hub's endpoint path.
pub const ASKPASS_SOCKET_ENV: &str = "MARSAD_ASKPASS_SOCKET";
/// Set for the askpass helper: the hub machine's diagnostics directory (the helper appends its
/// one record to the shared `aux.ndjson` there).
pub const ASKPASS_DIAG_ENV: &str = "MARSAD_ASKPASS_DIAG";

/// Host entries shared between the hosts registry and the connector.
pub type Entries = Arc<RwLock<HashMap<HostId, HostEntry>>>;

/// Something a connect attempt learned, for the hosts registry to persist.
#[derive(Debug, Clone)]
pub enum Learned {
    Platform {
        host: HostId,
        platform: HostPlatform,
    },
    /// A sign-in of the host asked the user for something other than a host-key confirmation.
    Interactive { host: HostId },
    /// A sign-in of a host marked interactive asked the user nothing: the mark is cleared.
    NotInteractive { host: HostId },
    /// The machine key the host's daemon reported in its hello.
    Machine { host: HostId, machine: String },
}

impl Learned {
    /// The host the fact is about.
    pub fn host(&self) -> &HostId {
        match self {
            Learned::Platform { host, .. }
            | Learned::Interactive { host }
            | Learned::NotInteractive { host }
            | Learned::Machine { host, .. } => host,
        }
    }
}

pub struct BootstrapConnector {
    pub entries: Entries,
    pub learned: mpsc::Sender<Learned>,
    pub prompts: Prompts,
    /// The link command of each host's latest attempt (see [`LinkChildren`]).
    pub links: LinkChildren,
    /// This hub's id: the daemon a bootstrap starts serves it, and a daemon serving another hub
    /// is not taken over (see [`decide`]).
    pub hub_id: String,
    /// The shared ssh connections of ssh hosts (a Windows hub has none, see [`Masters`]).
    pub masters: Masters,
    pub own: VersionInfo,
    pub bin_dir: Option<PathBuf>,
    /// The hub's endpoint path (for the askpass helper).
    pub socket: PathBuf,
    /// This machine's diagnostics directory (for the askpass helper).
    pub diag_dir: PathBuf,
}

/// The link command (the transport child that carries a host's bootstrap and then its protocol)
/// of each host's latest attempt. A reconnect starts its command only once the previous one has
/// exited: a server that allows one session at a time (`MaxSessions 1`) refuses the second while
/// the first is open. The previous command is being replaced, so it is ended explicitly
/// (SIGKILL, which a wedged ssh cannot ignore) rather than left to end on its own; the wait for
/// its exit event then has no timer and completes once the kill has been delivered.
#[derive(Default)]
pub struct LinkChildren {
    by_host: Mutex<HashMap<HostId, Arc<ChildHandle>>>,
}

impl LinkChildren {
    /// Records `child` as `host`'s link command (the one a new attempt waits for).
    pub fn remember(&self, host: &HostId, child: Arc<ChildHandle>) {
        locked(&self.by_host).insert(host.clone(), child);
    }

    /// Whether `host`'s previous link command has not been waited for yet.
    pub fn pending(&self, host: &HostId) -> bool {
        locked(&self.by_host).contains_key(host)
    }

    /// Ends `host`'s previous link command and resolves once it has exited (at once when there is
    /// none). The command is taken from the table when this is first polled, so a second attempt
    /// never waits for the same exit twice.
    pub async fn settle(&self, host: &HostId) {
        let previous = locked(&self.by_host).remove(host);
        if let Some(child) = previous {
            end_link(&child).await;
            child.wait_exit_unbounded().await;
        }
    }
}

/// Kills a link command that has not exited yet, by its pid. Only that child's pid is used: the
/// child is not a process-group leader, and no process is found by name. The pid is ours until
/// the child's exit is recorded, because its waiter task reaps the child only then, so the pid
/// cannot have been reused in the meantime.
async fn end_link(child: &ChildHandle) {
    if child.exit().is_some() {
        return;
    }
    if let Some(pid) = child.pid() {
        kill_pid(pid).await;
    }
}

/// Force-kills the process `pid` (SIGKILL on Unix, `taskkill /F` on Windows: the same forced kill
/// Wasit's own tests use). A kill that fails because the process already ended is not an error,
/// and the caller waits for the exit event either way.
async fn kill_pid(pid: u32) {
    let pid_arg = pid.to_string();
    #[cfg(unix)]
    let mut cmd = wasit_process::command("kill");
    #[cfg(unix)]
    cmd.args(["-KILL", &pid_arg]);
    #[cfg(windows)]
    let mut cmd = wasit_process::command("taskkill");
    #[cfg(windows)]
    cmd.args(["/F", "/PID", &pid_arg]);
    let _ = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

fn cmd_err(
    kind: CommandFailureKind,
    exit_code: Option<i32>,
    stderr: impl Into<String>,
) -> ConnectError {
    ConnectError::Command {
        kind,
        exit_code,
        stderr: stderr.into(),
    }
}

/// The longest line read while waiting for a bootstrap line.
const MAX_LINE: usize = 4096;

/// How one line ended.
enum LineEnd {
    /// A `\n` ended it.
    Newline,
    /// The stream ended (what was read is a last, unterminated line, possibly empty).
    Eof,
    /// It passed [`MAX_LINE`] bytes.
    TooLong,
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read one `\n`-terminated line byte by byte (never past it), the line without the `\n`
/// collected in `line` as it arrives: a read that is cancelled or fails leaves what came in
/// `line`, as evidence.
async fn read_raw_line<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    line: &Mutex<Vec<u8>>,
) -> std::io::Result<LineEnd> {
    loop {
        let mut b = [0u8; 1];
        if r.read(&mut b).await? == 0 {
            return Ok(LineEnd::Eof);
        }
        if b[0] == b'\n' {
            return Ok(LineEnd::Newline);
        }
        let mut line = locked(line);
        line.push(b[0]);
        if line.len() > MAX_LINE {
            return Ok(LineEnd::TooLong);
        }
    }
}

/// Lines a host may print before the bootstrap's report (a `.profile` or login banner).
const MAX_PREAMBLE_LINES: usize = 32;

/// What a host printed before the bootstrap's report, or instead of it: the only evidence there
/// is when the report never comes.
#[derive(Debug, Default, PartialEq, Eq)]
struct Preamble {
    /// Lines, a last one without its newline included.
    lines: u32,
    /// Bytes, newlines included.
    bytes: u64,
    /// The bytes themselves (possibly UTF-16LE from wsl.exe; decode with
    /// [`wasit_process::decode_console_output`]).
    raw: Vec<u8>,
}

impl Preamble {
    fn line(&mut self, text: &[u8]) {
        self.lines += 1;
        self.bytes += text.len() as u64 + 1;
        self.raw.extend_from_slice(text);
        self.raw.push(b'\n');
    }

    /// What came after the last newline.
    fn partial(&mut self, text: &[u8]) {
        if !text.is_empty() {
            self.lines += 1;
            self.bytes += text.len() as u64;
            self.raw.extend_from_slice(text);
        }
    }

    /// The preamble as the user would read it.
    fn text(&self) -> String {
        wasit_process::decode_console_output(&self.raw)
    }

    /// How much the host sent, in words.
    fn describe(&self) -> String {
        match (self.lines, self.bytes) {
            (0, _) => "nothing".to_owned(),
            (1, b) => format!("1 line ({b} bytes)"),
            (n, b) => format!("{n} lines ({b} bytes)"),
        }
    }
}

/// What the host has sent while the report is awaited, kept as it arrives (so that an attempt
/// cancelled in the middle of the wait still has it).
#[derive(Default)]
struct Seen {
    /// The lines that ended, before the line in progress.
    preamble: Mutex<Preamble>,
    /// The line in progress.
    line: Mutex<Vec<u8>>,
}

impl Seen {
    /// Everything seen so far, the line in progress counted as a last, unterminated line;
    /// resets for the next read.
    fn take(&self) -> Preamble {
        let mut p = std::mem::take(&mut *locked(&self.preamble));
        p.partial(&std::mem::take(&mut *locked(&self.line)));
        p
    }
}

/// Why no report was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// The stream ended.
    Ended,
    /// [`MAX_PREAMBLE_LINES`] lines passed without the report.
    TooManyLines,
    /// A line passed [`MAX_LINE`] bytes.
    LineTooLong,
    /// Reading failed.
    ReadFailed,
}

impl Missing {
    fn describe(self) -> &'static str {
        match self {
            Missing::Ended => "before the connection ended",
            Missing::TooManyLines => "before the limit of 32 lines was reached",
            Missing::LineTooLong => "before a line longer than 4096 bytes",
            Missing::ReadFailed => "before reading from the connection failed",
        }
    }
}

enum BootRead {
    /// A protocol line (one of the tags asked for).
    Report(String),
    /// It did not come.
    Missing(Missing),
}

/// The tags a host's first protocol line may have.
const FIRST_TAGS: &[&str] = &[bootwire::SH_READY, bootwire::REPORT, bootwire::FAIL];
/// The tags of a report (after the POSIX script was sent, or after an upload).
const REPORT_TAGS: &[&str] = &[bootwire::REPORT, bootwire::FAIL];
/// The tags of the answer to a decision.
const OK_TAGS: &[&str] = &[bootwire::OK, bootwire::FAIL];

/// Read until a line tagged with one of `tags`, keeping what came before it in `seen` as it
/// arrives. No time bound: a host whose login takes minutes (module loads in rc files) is still
/// starting, and a dead connection ends the stream (ssh's keep-alives).
async fn read_boot_line<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    seen: &Seen,
    tags: &[&str],
) -> BootRead {
    for _ in 0..MAX_PREAMBLE_LINES {
        let why = match read_raw_line(r, &seen.line).await {
            Ok(LineEnd::Newline) => {
                let line = std::mem::take(&mut *locked(&seen.line));
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches('\r');
                let tag = text.split_whitespace().next().unwrap_or("");
                if tags.contains(&tag) {
                    return BootRead::Report(text.to_owned());
                }
                locked(&seen.preamble).line(&line);
                continue;
            }
            Ok(LineEnd::Eof) => Missing::Ended,
            Ok(LineEnd::TooLong) => Missing::LineTooLong,
            Err(_) => Missing::ReadFailed,
        };
        return BootRead::Missing(why);
    }
    BootRead::Missing(Missing::TooManyLines)
}

/// How one attempt of the bootstrap is recorded (`boot.*`, design §5.3): every step carries the
/// host and the attempt number the supervisor passed to the connector, which is what joins it
/// to the supervisor's own `link.*` records of `hosts:<host>`.
///
/// An attempt the supervisor cancels (its connect bound ran out, the link was stopped) drops
/// the connector's future without a result: the trail then records, when it is dropped, that
/// it did not finish, where it was, and what the host had printed so far.
struct Trail {
    host: Ident,
    /// The supervisor's attempt: its number joins every record, and the step the attempt has
    /// reached is told to it (see [`Trail::reach`]).
    attempt: Attempt,
    began: Instant,
    /// What the host printed before its report, as it arrives.
    seen: Seen,
    progress: Mutex<Progress>,
}

struct Progress {
    stage: BootStage,
    /// `boot.preamble` was recorded for the read in progress.
    preamble_recorded: bool,
    finished: bool,
}

impl Trail {
    fn new(host: &HostId, attempt: Attempt) -> Trail {
        let trail = Trail {
            host: Ident::new(host.as_str()),
            attempt,
            began: Instant::now(),
            seen: Seen::default(),
            progress: Mutex::new(Progress {
                stage: BootStage::Prepare,
                preamble_recorded: true,
                finished: false,
            }),
        };
        trail.reach(BootStage::Prepare);
        trail
    }

    /// The attempt's number, as every record carries it.
    fn n(&self) -> u64 {
        self.attempt.n()
    }

    /// The attempt is in `stage` now: kept for the record of a cancelled attempt, and told to the
    /// supervisor in the words of the step, which it shows if its time runs out
    /// ("connect timed out while <step>").
    fn reach(&self, stage: BootStage) {
        self.progress().stage = stage;
        self.attempt.reached(match stage {
            BootStage::Prepare => "preparing the connection",
            BootStage::Spawn => "starting the transport command",
            BootStage::SignIn => "signing in to the host (opening the shared ssh connection)",
            BootStage::Report => "waiting for the host's bootstrap report",
            BootStage::Platform => "checking the host's platform",
            BootStage::Helper => "locating the Marsad helper to install",
            BootStage::Upload => "sending the Marsad helper to the host",
            BootStage::Ok => "waiting for the host to carry out the decision (a restart waits for the old daemon to exit, a start for the daemon to be ready)",
            BootStage::Hello => "waiting for the helper's hello",
            BootStage::Check => "checking the host's identity",
        });
    }

    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        locked(&self.progress)
    }

    /// The attempt ended, with a result (a failure is recorded by [`Trail::failed`]).
    fn finish(&self) {
        self.progress().finished = true;
    }

    fn since(&self) -> Ms {
        diag::ms(self.began)
    }

    /// A report is being waited for: what arrives from now on is a new preamble.
    fn expect_report(&self) {
        self.seen.take();
        self.reach(BootStage::Report);
        self.progress().preamble_recorded = false;
    }

    /// The read for the report ended: records and returns what the host printed before it (or
    /// instead of it).
    fn record_preamble(&self) -> Preamble {
        let p = self.seen.take();
        self.progress().preamble_recorded = true;
        self.emit_preamble(&p);
        p
    }

    fn attempt(&self, entry: &HostEntry, armed: bool) {
        emit(MarsadEvent::BootAttempt {
            host: self.host.clone(),
            n: self.n(),
            shell: match entry.kind {
                HostKind::Wsl { .. } => BootShell::Posix,
                _ => BootShell::Universal,
            },
            first_contact: entry.platform.is_none(),
            interactive: entry.interactive_auth,
            armed,
        });
    }

    fn emit_preamble(&self, p: &Preamble) {
        emit(MarsadEvent::BootPreamble {
            host: self.host.clone(),
            n: self.n(),
            lines: p.lines,
            bytes: Bytes(p.bytes),
            shape: (p.lines > 0).then(|| diag::shape(p.text().trim())),
        });
    }

    fn report(&self, r: &Report) {
        emit(MarsadEvent::BootReport {
            host: self.host.clone(),
            n: self.n(),
            os: Code::new(&r.os),
            arch: Code::new(&r.arch),
            installed: r.helper_build.as_deref().map(Ident::new),
            ms: self.since(),
        });
    }

    /// Progress within a step, in words ("sending the Marsad helper (3 of 9 MB)"): what the
    /// link state shows while the attempt runs.
    fn tell(&self, words: String) {
        self.attempt.reached(words);
    }

    fn action(&self, action: HelperAction, bytes: Option<u64>, target: &str) {
        emit(MarsadEvent::BootActionChosen {
            host: self.host.clone(),
            n: self.n(),
            action,
            bytes: bytes.map(Bytes),
            target: Ident::new(target),
        });
    }

    fn uploaded(&self, bytes: u64, took: Duration) {
        emit(MarsadEvent::BootUploaded {
            host: self.host.clone(),
            n: self.n(),
            bytes: Bytes(bytes),
            ms: Ms::of(took),
        });
    }

    fn ok(&self) {
        emit(MarsadEvent::BootOk {
            host: self.host.clone(),
            n: self.n(),
            ms: self.since(),
        });
    }

    fn hello(&self, hello: &wasit_control::Hello, host: &HostId) {
        emit(MarsadEvent::BootHello {
            host: self.host.clone(),
            n: self.n(),
            build: Ident::new(&hello.build),
            host_id_matches: hello.host_id == *host,
            machine: hello.machine.as_deref().map(Ident::new),
            ms: self.since(),
        });
    }

    /// The attempt failed at `stage` with `e`: recorded as wasit-control maps a connect error
    /// to its record ([`ConnectDiag`]).
    fn failed(&self, stage: BootStage, e: &ConnectError) {
        let d = ConnectDiag::of(e);
        emit(MarsadEvent::BootFailed {
            host: self.host.clone(),
            n: self.n(),
            stage,
            kind: d.kind,
            exit: d.exit,
            stderr: d.stderr,
            error: d.error,
        });
    }
}

impl Drop for Trail {
    fn drop(&mut self) {
        let (stage, recorded) = {
            let p = self.progress();
            if p.finished {
                return;
            }
            (p.stage, p.preamble_recorded)
        };
        if !recorded {
            self.emit_preamble(&self.seen.take());
        }
        emit(MarsadEvent::BootFailed {
            host: self.host.clone(),
            n: self.n(),
            stage,
            kind: None,
            exit: None,
            stderr: None,
            error: Some(diag::ours(
                "cancelled",
                &"the attempt ended before it finished (its time ran out, or the link was stopped)",
            )),
        });
    }
}

/// A failed attempt and the step it failed in.
struct Failure {
    stage: BootStage,
    error: ConnectError,
}

fn at(stage: BootStage) -> impl Fn(ConnectError) -> Failure {
    move |error| Failure { stage, error }
}

/// Why sending the helper failed: its file (ours to read) or the host's end (gone).
enum SendError {
    Local(std::io::Error),
    Remote(std::io::Error),
}

/// Copy `from` to `to` in chunks; `sent` counts what the host's end took, and `progress` is
/// told after every chunk.
async fn send_file<R, W>(
    from: &mut R,
    to: &mut W,
    sent: &mut u64,
    progress: impl Fn(u64),
) -> Result<(), SendError>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let n = from.read(&mut chunk).await.map_err(SendError::Local)?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&chunk[..n]).await.map_err(SendError::Remote)?;
        *sent += n as u64;
        progress(*sent);
    }
}

/// Which binary to upload (see the module docs).
#[derive(Debug, PartialEq, Eq)]
pub enum Helper {
    /// The running executable.
    Running,
    File(PathBuf),
}

/// Pick the helper for `platform`, given the hub's own target triple and bin dir.
pub fn pick_helper(
    own_target: &str,
    bin_dir: Option<&std::path::Path>,
    platform: &HostPlatform,
) -> Result<Helper, ConnectError> {
    let Some(triple) = helper_target(&platform.os, &platform.arch) else {
        return Err(cmd_err(
            CommandFailureKind::UnsupportedPlatform,
            None,
            format!(
                "Marsad has no helper for {} on {} (it ships helpers for Linux, macOS and Windows on x86_64 and ARM64). This host cannot run Marsad sessions; remove it.",
                os_word(&platform.os),
                platform.arch
            ),
        ));
    };
    let exe_name = if triple.contains("windows") {
        "marsad-agent.exe"
    } else {
        "marsad-agent"
    };
    if triple == own_target {
        return Ok(Helper::Running);
    }
    let looked = bin_dir.map(|d| d.join(triple).join(exe_name));
    if let Some(p) = looked.as_ref().filter(|p| p.is_file()) {
        return Ok(Helper::File(p.clone()));
    }
    // A development build for another libc of the same OS and architecture.
    let (own_os, own_arch) = triple_platform(own_target);
    if own_os == platform.os && own_arch == platform.arch {
        return Ok(Helper::Running);
    }
    Err(cmd_err(
        CommandFailureKind::HelperMissing,
        None,
        match looked {
            Some(p) => format!(
                "This Marsad installation is missing its helper for {} {} (expected at {}). Reinstall Marsad, then try again.",
                os_word(&platform.os),
                platform.arch,
                wasit_process::plain_path(&p).display()
            ),
            None => format!(
                "This Marsad build has no helpers for other machines ({triple} needed): set {BIN_DIR_ENV} to a directory with {triple}/{exe_name}, or use an installed Marsad."
            ),
        },
    ))
}

fn os_word(os: &str) -> &str {
    match os {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        o => o,
    }
}

async fn send_learned(learned: &mpsc::Sender<Learned>, l: Learned) {
    let host = l.host().clone();
    if learned.send(l).await.is_err() {
        emit(MarsadEvent::HostsFailed {
            host: Some(Ident::new(host.as_str())),
            op: HostsOp::RegistryGone,
            error: diag::ours("registry_gone", &"the hosts registry is gone"),
        });
    }
}

/// Learns that `host` needs the user at login as soon as an attempt asks the user something
/// (its [`Asking`] becomes `Asked`), whatever becomes of the attempt. Learned from the outcome
/// alone, a host would be taken for one that does not need the user for as long as its attempts
/// fail or are cut short: they never reach the place that learns it, and the next one would ask
/// (and push) all over again. The watcher ends with the attempt.
fn learn_when_asked(
    learned: mpsc::Sender<Learned>,
    host: HostId,
    mut asking: watch::Receiver<Asking>,
) {
    tokio::spawn(async move {
        let asked = matches!(
            asking.wait_for(|a| *a != Asking::Waiting).await.as_deref(),
            Ok(Asking::Asked)
        );
        if asked {
            send_learned(&learned, Learned::Interactive { host }).await;
        }
    });
}

impl BootstrapConnector {
    fn entry(&self, id: &HostId) -> Result<HostEntry, ConnectError> {
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| ConnectError::Spawn(format!("unknown host {id}")))
    }

    /// The binary to install on a host running `platform`.
    pub fn artifact(&self, platform: &HostPlatform) -> Result<PathBuf, ConnectError> {
        match pick_helper(&self.own.target, self.bin_dir.as_deref(), platform)? {
            Helper::File(p) => Ok(p),
            Helper::Running => std::env::current_exe()
                .map_err(|e| ConnectError::Spawn(format!("cannot locate the running binary: {e}"))),
        }
    }

    async fn learn(&self, l: Learned) {
        send_learned(&self.learned, l).await;
    }

    /// Classify an attempt that ended before the protocol started.
    async fn failed(
        &self,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
        what: &str,
    ) -> ConnectError {
        let exit = child.wait_exit(CLOSED_EXIT_WAIT).await;
        let stderr = child.stderr_tail();
        let code = exit.and_then(|e| e.code);
        let kind = failure_kind(child.transport(), code, signin, &stderr);
        let detail = if stderr.is_empty() {
            format!("{what} (exit {code:?})")
        } else {
            stderr
        };
        cmd_err(kind, code, detail)
    }

    /// Writing to the host failed: its end is gone, and what the transport child says (its
    /// exit status and stderr) is the reason, not our broken pipe (`what` stands in when the
    /// child said nothing).
    async fn remote_gone(
        &self,
        stage: BootStage,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
        what: &str,
        e: &std::io::Error,
    ) -> Failure {
        Failure {
            stage,
            error: self
                .failed(signin, child, &format!("{what} ({})", e.kind()))
                .await,
        }
    }

    /// The host never printed the bootstrap's report: classify what it printed instead
    /// (stdout and stderr; wsl.exe prints its errors to stdout, in UTF-16LE).
    async fn no_bootstrap(
        &self,
        entry: &HostEntry,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
        stdout: &str,
        preamble: &Preamble,
        why: Missing,
    ) -> ConnectError {
        let exit = child.wait_exit(CLOSED_EXIT_WAIT).await;
        let stderr = child.stderr_tail();
        let code = exit.and_then(|e| e.code);
        let both = sent_words(stdout, &stderr);
        // WSL's own conditions are asked of WSL (its distribution list); the rest of the
        // transport is judged by its structure.
        let listed = match &entry.kind {
            HostKind::Wsl { .. } => Some(super::wsl::distros().await),
            _ => None,
        };
        let mut kind = match (&entry.kind, &listed) {
            (HostKind::Wsl { distro }, Some(list)) => wsl_failure(distro, list)
                .unwrap_or_else(|| failure_kind(child.transport(), code, signin, &both)),
            _ => failure_kind(child.transport(), code, signin, &both),
        };
        if kind == CommandFailureKind::Other && !stdout.is_empty() {
            kind = CommandFailureKind::UnexpectedOutput;
        }
        let detail = match kind {
            CommandFailureKind::UnknownDistro => {
                let distro = match &entry.kind {
                    HostKind::Wsl { distro } => distro.as_str(),
                    _ => "",
                };
                let names: Vec<&str> = listed
                    .iter()
                    .flat_map(|l| l.distros.iter())
                    .map(|d| d.name.as_str())
                    .collect();
                if names.is_empty() {
                    format!("WSL has no distribution named \"{distro}\", and none is installed. Install one (wsl --install), then edit this host.")
                } else {
                    format!(
                        "WSL has no distribution named \"{distro}\". Installed: {}. Edit this host and pick one of them.",
                        names.join(", ")
                    )
                }
            }
            CommandFailureKind::WslUnavailable => format!("WSL is not ready on this PC: {both}"),
            CommandFailureKind::PermissionDenied => permission_denied_detail(&both),
            CommandFailureKind::UnexpectedOutput => format!(
                "The host printed something other than the Marsad bootstrap: {}. Make sure your shell startup files print nothing for non-interactive shells, then try again.",
                truncate(&both, 300)
            ),
            _ if both.is_empty() => format!("the connection closed before the bootstrap ran (exit {code:?})"),
            _ => both,
        };
        // What the host sent before the report would have been is the evidence of the shell
        // startup files; say how much there was, whatever the kind.
        let detail = match kind {
            CommandFailureKind::UnknownDistro | CommandFailureKind::WslUnavailable => detail,
            _ => format!(
                "{detail} [the host sent {} {}, none of it the Marsad bootstrap's report]",
                preamble.describe(),
                why.describe()
            ),
        };
        cmd_err(kind, code, detail)
    }
}

/// The environment of an ssh command the hub runs (pure; see [`BootstrapConnector::ssh_env`]).
pub fn ssh_environment(
    askpass: &Path,
    host: &HostId,
    attempt: u64,
    socket: &Path,
    diag_dir: &Path,
    display: Option<&std::ffi::OsStr>,
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "SSH_ASKPASS".into(),
            wasit_process::plain_path(askpass).display().to_string(),
        ),
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        ("LC_ALL".into(), "C".into()),
        (ASKPASS_HOST_ENV.into(), host.to_string()),
        (ASKPASS_ATTEMPT_ENV.into(), attempt.to_string()),
        (ASKPASS_SOCKET_ENV.into(), socket.display().to_string()),
        (ASKPASS_DIAG_ENV.into(), diag_dir.display().to_string()),
    ];
    env.extend(display_fallback(display));
    env
}

/// The `DISPLAY` an ssh command of the hub is given when the hub has none (U16: a forced `:0`
/// clobbered a real display). `None` when the hub has one, empty counting as none.
pub fn display_fallback(current: Option<&std::ffi::OsStr>) -> Option<(String, String)> {
    match current {
        Some(d) if !d.is_empty() => None,
        _ => Some(("DISPLAY".into(), ":0".into())),
    }
}

/// The kind of a transport command that failed before the protocol started, decided from its
/// structure first, in the order the facts decide: a status that is the remote program's (see
/// [`TransportKind::remote_status`]) proves the sign-in went through, so the failure is the
/// remote program's own; otherwise what the sign-in asked decides; and only then the words the
/// transport wrote, for the few refusals of ssh that need a person and print a fixed message
/// ([`ssh_refusal`]). Anything else is `Other` (retried with backoff). `words` is the transport's
/// stderr (and the sign-in's log): shown to the user, and read here only for those messages.
pub fn failure_kind(
    transport: TransportKind,
    exit: Option<i32>,
    signin: &SignInAttempt,
    words: &str,
) -> CommandFailureKind {
    failure_kind_of(
        transport,
        exit,
        Asks {
            beyond_host_key: signin.asked_beyond_host_key(),
            needs_user: signin.needed_user(),
            host_key_refused: signin.host_key_refused(),
        },
        words,
    )
}

/// What a sign-in asked, as far as the failure decision goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Asks {
    /// A prompt other than a host-key confirmation was asked ([`SignInAttempt::asked_beyond_host_key`]).
    pub beyond_host_key: bool,
    /// A prompt that needs the user at login was asked ([`PromptKind::needs_user`]).
    pub needs_user: bool,
    /// The host key was refused or left unanswered ([`SignInAttempt::host_key_refused`]). A key
    /// the user accepted is not this: a failure after it is judged as any other.
    pub host_key_refused: bool,
}

/// [`failure_kind`] from the facts alone (pure).
pub fn failure_kind_of(
    transport: TransportKind,
    exit: Option<i32>,
    asks: Asks,
    words: &str,
) -> CommandFailureKind {
    if let Some(code) = exit.filter(|c| transport.remote_status(*c)) {
        return match code {
            126 => CommandFailureKind::PermissionDenied,
            127 => CommandFailureKind::CommandNotFound,
            _ => CommandFailureKind::HelperFailed,
        };
    }
    if asks.needs_user {
        CommandFailureKind::AuthNeeded
    } else if asks.host_key_refused {
        // A cancelled or timed-out host-key question makes ssh give up: that needs the user too.
        CommandFailureKind::HostKeyUnknown
    } else if asks.beyond_host_key {
        // A cancelled or timed-out prompt makes ssh give up: that needs the user too.
        CommandFailureKind::AuthNeeded
    } else {
        ssh_refusal(transport, exit, words).unwrap_or(CommandFailureKind::Other)
    }
}

/// The refusals of ssh itself (status 255, nothing asked of the user) that need a person, found
/// by ssh's own fixed messages; `None` for every other failure, which is retried with backoff
/// (network-level failures among them).
///
/// WHY words, and only these two: retrying either of them is wrong. A changed host key is a
/// possible man-in-the-middle and must be shown to the user, never retried. A refused
/// authentication repeated on a timer can lock the account. Nothing else of ssh's can be told
/// apart by its status, so the words are matched for exactly these, and only under `LC_ALL=C`
/// (every hub ssh sets it, so the messages are English and stable):
/// - `WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!` (the banner, OpenSSH's `sshconnect.c`,
///   printed for a host key that differs from the one known; ssh then exits 255);
/// - `Permission denied (` (`user@host: Permission denied (publickey,password).`, OpenSSH's
///   `sshconnect2.c`, once every method is refused) and `Permission denied, please try again.`
///   (a password or passphrase refused and asked again);
/// - `Too many authentication failures` (OpenSSH's `sshconnect2.c` disconnect once the server's
///   `MaxAuthTries` is spent): every further attempt on a timer counts against the account, so
///   it parks like a refused authentication;
/// - `Host key verification failed.` (OpenSSH's `sshconnect.c`, the host key declined or left
///   unanswered, or refused by `StrictHostKeyChecking`): a host-key refusal the user must
///   decide, so it parks as `HostKeyUnknown`. A changed key (the banner above) is checked first.
///
/// The words are read only to classify; they are never shown as the decision, and an unknown
/// message from another ssh version simply falls back to `Other`. Pure.
pub fn ssh_refusal(
    transport: TransportKind,
    exit: Option<i32>,
    words: &str,
) -> Option<CommandFailureKind> {
    if transport != TransportKind::Ssh || exit != Some(255) {
        return None;
    }
    if words.contains("REMOTE HOST IDENTIFICATION HAS CHANGED!") {
        Some(CommandFailureKind::HostKeyChanged)
    } else if words.contains("Host key verification failed.") {
        Some(CommandFailureKind::HostKeyUnknown)
    } else if words.contains("Permission denied (")
        || words.contains("Permission denied, please try again.")
        || words.contains("Too many authentication failures")
    {
        Some(CommandFailureKind::AuthNeeded)
    } else {
        None
    }
}

/// What WSL says about a WSL host whose bootstrap printed nothing: no WSL, no distribution at
/// all, or none by the host's name. `None`: WSL has the distribution, so the failure is the
/// distribution's own (judged by [`failure_kind`]). Asked of WSL's list, not of wsl.exe's words.
pub fn wsl_failure(
    distro: &str,
    list: &marsad_proto::hosts::WslDistros,
) -> Option<CommandFailureKind> {
    if !list.available || list.distros.is_empty() {
        return Some(CommandFailureKind::WslUnavailable);
    }
    (!list.distros.iter().any(|d| d.name == distro)).then_some(CommandFailureKind::UnknownDistro)
}

/// What a host that never printed the report said, for the reason shown to the user: its stdout
/// then its stderr tail (where the transport's own failure and a broken shell startup line
/// land), each left out when empty. WHY both: the stderr is the evidence the user needs when
/// the host's own status ended the attempt (`HelperFailed`), and it must never be dropped. Pure.
pub fn sent_words(stdout: &str, stderr: &str) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, _) => stderr.to_owned(),
        (false, true) => stdout.to_owned(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let t: String = s.chars().take(max).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
}

/// What the supervisor is told when a link must wait for the user (`auth_needed`, `held`), for
/// a host reached by one command per attempt (a Windows hub).
const HELD_DETAIL: &str =
    "this host asks you to sign in each time it connects: connect it to sign in";

/// The same, for a host with a shared connection, when none is open and nothing says why.
const HELD_NO_MASTER: &str =
    "this host asks you to sign in and no shared connection to it is open: connect it to sign in";

/// The user signed out of the host's shared connection.
const SIGNED_OUT_DETAIL: &str = "you signed out of this host: connect it to sign in again";

/// The host is set to be asked before it connects (G4): it waits for the user's click, whatever
/// its connection does.
const HELD_ASKED_DETAIL: &str =
    "this host waits for you: it connects only when you connect it (it is set to be asked before connecting)";

/// Whether an attempt the user did not ask for must not run on its own: the host is set to be
/// asked before it connects, or it needs the user at login (learned, see
/// [`HostEntry::interactive_auth`]). The one rule both the supervisor's question ([`hold`]) and an
/// attempt's own check apply.
///
/// [`hold`]: LinkConnector::hold
fn held_until_asked(entry: &HostEntry) -> bool {
    entry.ask_before_connecting || entry.interactive_auth
}

/// The refusal of an attempt whose host the user signed out of, whether the sign-out came before
/// the attempt or during its sign-in. WHY a refusal and not a `Command` AuthNeeded: the supervisor
/// retries a refused attempt only after asking `hold`, and `hold` parks the host as `held: true`
/// with this detail. A `Command` AuthNeeded would park it as a failed attempt that asked for a
/// person (`held: false`), which the app reads as a sign-in that failed. A sign-out is the user's
/// own word, so both paths must report the same parked state.
fn signed_out_refusal() -> ConnectError {
    ConnectError::Refused(RpcError::new("auth_needed", SIGNED_OUT_DETAIL))
}

impl BootstrapConnector {
    /// The host is held: recorded, and the words the supervisor shows.
    fn held(&self, spec: &HostSpec, entry: &HostEntry) -> String {
        emit(MarsadEvent::HostsHeld {
            host: Ident::new(spec.id.as_str()),
        });
        self.held_detail(entry)
    }

    /// Why `entry` waits for the user, in words.
    fn held_detail(&self, entry: &HostEntry) -> String {
        if entry.ask_before_connecting {
            return HELD_ASKED_DETAIL.to_owned();
        }
        if !Masters::applies(entry) {
            return HELD_DETAIL.to_owned();
        }
        if self.masters.signed_out(&entry.id) {
            return SIGNED_OUT_DETAIL.to_owned();
        }
        match self.masters.ended_words(&entry.id) {
            Some(ended) => format!("{ended}: connect to sign in again"),
            None => HELD_NO_MASTER.to_owned(),
        }
    }
}

#[async_trait]
impl LinkConnector for BootstrapConnector {
    /// The one place that decides a link must not be tried on its own: the host needs the user at
    /// login ([`HostEntry::interactive_auth`]) or they signed out of it, no shared connection is
    /// open that would carry the attempt without a sign-in, and the user has not asked for this
    /// attempt. For a host with a shared connection that is asked of the connection itself (a
    /// local `ssh -O check`): a link that dropped while its connection stayed open reconnects on
    /// its own, whatever the host needs at login.
    async fn hold(&self, spec: &HostSpec) -> Option<String> {
        let entry = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&spec.id)
            .cloned()?;
        if self.prompts.armed(&spec.id) {
            return None;
        }
        // Asked before connecting (G4): held whatever its connection does.
        if entry.ask_before_connecting {
            return Some(self.held(spec, &entry));
        }
        // A signed-out host is held on the user's own word: no look at the connection is needed.
        let signed_out = Masters::applies(&entry) && self.masters.signed_out(&spec.id);
        if Masters::applies(&entry) && !signed_out {
            // A path that cannot be made is the attempt's to report.
            let control = self.masters.path(&entry).ok()?;
            // Only an absent connection lets the host be held: an unanswered one may be open.
            if self
                .masters
                .check(&entry, &control, None)
                .await
                .may_be_open()
            {
                return None;
            }
        }
        if !signed_out && !held_until_asked(&entry) {
            return None;
        }
        Some(self.held(spec, &entry))
    }

    async fn connect(
        &self,
        spec: &HostSpec,
        opts: ClientOptions,
        attempt: Attempt,
    ) -> Result<ControlClient, ConnectError> {
        let trail = Trail::new(&spec.id, attempt);
        let result = self.run(spec, opts, &trail).await;
        trail.finish();
        match result {
            Ok(client) => Ok(client),
            Err(f) => {
                trail.failed(f.stage, &f.error);
                Err(f.error)
            }
        }
    }
}

/// What the hub does with a host's report: one decision, or a refusal in words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Run,
    Restart(RestartWhy),
    Upload,
    Refuse(String),
}

/// The decision for a report (pure; [`BootstrapConnector::run`] carries it out):
///
/// * no helper of this build: install it, if the host can check an upload's SHA-256 (else a
///   refusal saying so), and only once per connection;
/// * a helper that reports another build than the directory it was found in: a refusal (the
///   installation's helper is not this hub's build);
/// * a daemon that serves another hub: a refusal, for as long as it runs (G1: one account's
///   daemon serves one hub; it is never taken over from a hub that uses it);
/// * a daemon of another build, or one that knows itself by another host id (unless that id is
///   a twin of this host, the same machine added twice: a refusal), or something that holds the
///   daemon's endpoint and does not answer as one: restart it;
/// * else run.
pub fn decide(
    r: &Report,
    own_build: &str,
    hub_id: &str,
    host: &HostId,
    twin: Option<&str>,
    uploaded: bool,
) -> Choice {
    match r.helper_build.as_deref() {
        None if uploaded => {
            return Choice::Refuse(
                "after the upload the host reported no Marsad helper installed".to_owned(),
            )
        }
        None if r.hasher.is_none() => {
            return Choice::Refuse(
                "this host has no SHA-256 tool (sha256sum, shasum or openssl), so the Marsad helper cannot be checked after it is sent and is not installed: install one of them (coreutils, perl or openssl), then connect again"
                    .to_owned(),
            )
        }
        None => return Choice::Upload,
        Some(b) if b != own_build => {
            return Choice::Refuse(format!(
                "the Marsad helper installed for this build reports build {b}, not {own_build}: this Marsad installation's helper for the host's platform is of another version. Reinstall Marsad, then connect again"
            ))
        }
        Some(_) => {}
    }
    match r.daemon {
        DaemonState::Running => {
            if let Some(theirs) = r.daemon_hub.as_deref().filter(|h| *h != hub_id) {
                let pid = r
                    .daemon_pid
                    .map_or_else(String::new, |p| format!(" (pid {p})"));
                return Choice::Refuse(format!(
                    "the Marsad daemon of this account on the host{pid} serves another Marsad hub ({theirs}); an account's daemon serves one hub, and it is not taken over while it runs. Stop it on the host (marsad-agent stop) or remove the host from the other hub, then connect again"
                ));
            }
            if r.daemon_build.as_deref() != Some(own_build) {
                return Choice::Restart(RestartWhy::Build);
            }
            if r.daemon_host.as_deref() != Some(host.as_str()) {
                if let Some(name) = twin {
                    return Choice::Refuse(format!(
                        "this is the same machine as the host {name:?} (machine key {}); remove one of the two hosts",
                        r.machine.as_deref().unwrap_or("?")
                    ));
                }
                return Choice::Restart(RestartWhy::Host);
            }
            Choice::Run
        }
        DaemonState::Unreachable => Choice::Restart(RestartWhy::Unreachable),
        DaemonState::Absent | DaemonState::Unknown => Choice::Run,
    }
}

/// A refusal of the bootstrap or the hub: a configuration of the host or the installation,
/// which retrying on a timer cannot change (the link parks until the user connects again).
fn refused(words: impl Into<String>) -> ConnectError {
    cmd_err(CommandFailureKind::HelperFailed, None, words)
}

/// The bootstraps' `reason=` for a helper directory the account cannot write (POSIX and Windows
/// alike, checked before anything is written). A structural fact of the host, so the hub explains
/// it with its own words ([`permission_denied_detail`]) rather than the host's text.
pub const NOT_WRITABLE: &str = "not_writable";

/// The user's message for a host that refuses to run or store the Marsad helper. The hint is
/// written here, by the hub, for every such refusal; `evidence` is what the host showed.
pub fn permission_denied_detail(evidence: &str) -> String {
    format!(
        "The host refused to run the Marsad helper (permission denied). Check that your home directory allows running programs (for example a noexec mount), then try again. ({evidence})"
    )
}

/// `MARSAD-FAIL stage= reason=`, in words. A `not_writable` reason is the same refusal as the
/// host refusing to run the helper: it gets the same hint.
fn host_failed(line: &Line, exit: Option<i32>) -> ConnectError {
    if line.get("reason") == Some(NOT_WRITABLE) {
        return cmd_err(
            CommandFailureKind::PermissionDenied,
            exit,
            permission_denied_detail("the helper directory on the host is not writable"),
        );
    }
    cmd_err(
        CommandFailureKind::HelperFailed,
        exit,
        format!(
            "the host's bootstrap failed ({}): {}",
            line.get("stage").unwrap_or("?"),
            line.get("reason").unwrap_or("no reason given")
        ),
    )
}

impl BootstrapConnector {
    /// The bootstrap of `host`, for this hub and build.
    fn target<'a>(&'a self, host: &'a HostId) -> BootTarget<'a> {
        BootTarget {
            host_id: host.as_str(),
            hub_id: &self.hub_id,
            build: &self.own.build,
        }
    }

    /// The host that is the same machine as `host` (by the machine key `machine`), by name.
    fn twin(&self, host: &HostId, machine: Option<&str>) -> Option<(HostId, String)> {
        let m = machine?;
        self.entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .find(|e| e.id != *host && e.machine.as_deref() == Some(m))
            .map(|e| (e.id.clone(), e.name.clone()))
    }

    /// The next protocol line tagged with one of `tags`; the stream ending (or anything else
    /// that keeps the line from coming) is a failure at `stage`, explained by the transport
    /// child (its exit status and stderr).
    async fn next_line<R: tokio::io::AsyncRead + Unpin>(
        &self,
        io: &mut R,
        trail: &Trail,
        stage: BootStage,
        tags: &[&str],
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
    ) -> Result<Line, Failure> {
        match read_boot_line(io, &trail.seen, tags).await {
            BootRead::Report(text) => {
                trail.seen.take();
                let line = Line::parse(&text);
                if line.tag == bootwire::FAIL {
                    return Err(Failure {
                        stage,
                        error: host_failed(&line, None),
                    });
                }
                Ok(line)
            }
            BootRead::Missing(why) => {
                let p = trail.seen.take();
                Err(Failure {
                    stage,
                    error: self
                        .failed(
                            signin,
                            child,
                            &format!(
                                "the host sent {} {} (exit status follows)",
                                p.describe(),
                                why.describe()
                            ),
                        )
                        .await,
                })
            }
        }
    }

    /// Writes `bytes` to the host and flushes; the host's end gone is a failure at `stage`.
    async fn send<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        io: &mut W,
        bytes: &[u8],
        stage: BootStage,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
    ) -> Result<(), Failure> {
        let r = async {
            io.write_all(bytes).await?;
            io.flush().await
        }
        .await;
        match r {
            Ok(()) => Ok(()),
            Err(e) => Err(self
                .remote_gone(stage, signin, child, "the host closed the connection", &e)
                .await),
        }
    }

    /// One connect attempt, step by step; `trail` records each step (see [`Trail`]). There is
    /// no bound on the attempt as a whole: every step waits for the host's next line, or for its
    /// end (ssh's keep-alives end a dead connection), and a slow host only takes longer, its
    /// progress shown in the link state. The user-facing bound is the sign-in prompts'.
    async fn run(
        &self,
        spec: &HostSpec,
        opts: ClientOptions,
        trail: &Trail,
    ) -> Result<ControlClient, Failure> {
        let entry = self.entry(&spec.id).map_err(at(BootStage::Prepare))?;
        let host = entry.id.clone();
        // (The supervisor asked `hold` before it started this attempt: a host that needs the
        // user is only here because the user asked, or because this follows their sign-in.)
        trail.attempt(&entry, self.prompts.armed(&host));
        let exe = std::env::current_exe()
            .map_err(|e| ConnectError::Spawn(e.to_string()))
            .map_err(at(BootStage::Prepare))?;
        // This attempt's prompts are its own: they end with it, whichever way it ends.
        let signin = self.prompts.begin_attempt(&host, None);
        learn_when_asked(self.learned.clone(), host.clone(), signin.asking());
        // An ssh host on a Linux or macOS hub has one shared connection: opened here, once,
        // with every prompt of the attempt (the sign-in step); every command after it rides it
        // and can never ask. Anywhere else the one command below does it all.
        let control = if Masters::applies(&entry) {
            Some(
                self.shared_connection(spec, &entry, &exe, trail, &signin)
                    .await?,
            )
        } else {
            None
        };
        let route = match &control {
            Some(c) => SshRoute::Link { control: &c.0 },
            None => SshRoute::Plain,
        };
        let argv = bootstrap_argv_via(&entry, self.target(&host), route)
            .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))
            .map_err(at(BootStage::Prepare))?;
        // A reconnect waits for the previous link command of this host to end (G11).
        if self.links.pending(&host) {
            trail.tell("waiting for the previous connection to the host to close".to_owned());
        }
        self.links.settle(&host).await;
        // The transport's spawn and exit are recorded under this link and attempt.
        let mut cmd = CommandSpec::new(argv).traced(spec.diag_link(), &trail.attempt);
        cmd.env = self.ssh_env(&exe, &host, &signin);
        let (mut io, child) = CommandTransport::spawn(&cmd).map_err(at(BootStage::Spawn))?;
        let child = Arc::new(child);
        self.links.remember(&host, child.clone());
        trail.expect_report();
        let mut line = match read_boot_line(&mut io, &trail.seen, FIRST_TAGS).await {
            BootRead::Report(text) => {
                trail.record_preamble();
                Line::parse(&text)
            }
            BootRead::Missing(why) => {
                let preamble = trail.record_preamble();
                let err = self
                    .no_bootstrap(
                        &entry,
                        &signin,
                        &child,
                        preamble.text().trim(),
                        &preamble,
                        why,
                    )
                    .await;
                return Err(Failure {
                    stage: BootStage::Report,
                    error: self.unridden(&entry, &control, trail, err).await,
                });
            }
        };
        // A POSIX host's `sh` asks for its script: one line, then its report.
        if line.tag == bootwire::SH_READY {
            let script = format!("{}\n", posix_script(self.target(&host)));
            self.send(
                &mut io,
                script.as_bytes(),
                BootStage::Report,
                &signin,
                &child,
            )
            .await?;
            line = self
                .next_line(
                    &mut io,
                    trail,
                    BootStage::Report,
                    REPORT_TAGS,
                    &signin,
                    &child,
                )
                .await?;
        } else if line.tag == bootwire::FAIL {
            return Err(Failure {
                stage: BootStage::Report,
                error: host_failed(&line, None),
            });
        }
        // The host answered and the sign-in worked. Whether it needs the user at login is a
        // fact of this sign-in: what it asked, nothing more (a host-key confirmation is not the
        // user's at login). With a shared connection the sign-in was the step that opened it,
        // and was learned from there.
        if control.is_none() {
            self.learn_login(&entry, &signin).await;
        }
        let mut uploaded = false;
        let decision = loop {
            let report = Report::parse(&line);
            trail.report(&report);
            trail.reach(BootStage::Platform);
            // Learned from every report (B14: a reinstalled host is never stuck as what it was).
            let platform = platform_from(&report.os, &report.arch).ok_or_else(|| Failure {
                stage: BootStage::Platform,
                error: cmd_err(
                    CommandFailureKind::UnsupportedPlatform,
                    None,
                    format!("Marsad has no helper for {} on {} (it ships helpers for Linux, macOS and Windows on x86_64 and ARM64). This host cannot run Marsad sessions; remove it.", report.os, report.arch),
                ),
            })?;
            if entry.platform.as_ref() != Some(&platform) {
                self.learn(Learned::Platform {
                    host: host.clone(),
                    platform: platform.clone(),
                })
                .await;
            }
            let twin = self.twin(&host, report.machine.as_deref());
            if let Some((twin_id, _)) = &twin {
                emit(MarsadEvent::BootTwin {
                    host: trail.host.clone(),
                    n: trail.n(),
                    twin: Ident::new(twin_id.as_str()),
                });
            }
            let choice = decide(
                &report,
                &self.own.build,
                &self.hub_id,
                &host,
                twin.as_ref().map(|t| t.1.as_str()),
                uploaded,
            );
            match choice {
                Choice::Refuse(words) => {
                    return Err(Failure {
                        stage: BootStage::Check,
                        error: refused(words),
                    })
                }
                Choice::Run | Choice::Restart(_) => {
                    if let Some(m) = &report.machine {
                        self.learn(Learned::Machine {
                            host: host.clone(),
                            machine: m.clone(),
                        })
                        .await;
                    }
                    let (d, action) = match choice {
                        Choice::Restart(why) => (Decision::Restart(why), HelperAction::Restart),
                        _ => (Decision::Run, HelperAction::Run),
                    };
                    trail.reach(BootStage::Ok);
                    trail.action(action, None, &platform.target);
                    let text = format!("{}\n", d.line());
                    self.send(&mut io, text.as_bytes(), BootStage::Ok, &signin, &child)
                        .await?;
                    break d;
                }
                Choice::Upload => {
                    self.upload(&mut io, trail, &platform, &signin, &child)
                        .await?;
                    uploaded = true;
                    // The new helper reports (`install=ok`) and gets the decision.
                    line = self
                        .next_line(
                            &mut io,
                            trail,
                            BootStage::Upload,
                            REPORT_TAGS,
                            &signin,
                            &child,
                        )
                        .await?;
                }
            }
        };
        let ok = self
            .next_line(&mut io, trail, BootStage::Ok, OK_TAGS, &signin, &child)
            .await?;
        trail.tell(format!(
            "the host carried out {} (stop: {}, install: {}, daemon: {})",
            decision.line(),
            ok.get("stop").unwrap_or("none"),
            ok.get("install").unwrap_or("none"),
            ok.get("daemon").unwrap_or("?")
        ));
        trail.ok();
        trail.reach(BootStage::Hello);
        let client = match ControlClient::connect(io, opts).await {
            Ok(c) => c,
            Err(e) => {
                // The bootstrap said OK: the helper has the connection, and what the transport
                // child says is the helper's own reason (wasit-control decides the kind).
                return Err(Failure {
                    stage: BootStage::Hello,
                    error: child.explain_handshake_failure(e).await,
                });
            }
        };
        let hello = client.connection().remote_hello().clone();
        trail.hello(&hello, &host);
        trail.reach(BootStage::Check);
        if hello.host_id != host {
            // The decision above made the daemon this host's; anything else is not ours to fix
            // by connecting again.
            emit(MarsadEvent::BootIdMismatch {
                host: trail.host.clone(),
                n: trail.n(),
                theirs: Ident::new(hello.host_id.as_str()),
            });
            return Err(Failure {
                stage: BootStage::Check,
                error: refused(format!(
                    "the daemon on this host runs as {:?}, not {host:?}, after the bootstrap made it this host's",
                    hello.host_id
                )),
            });
        }
        // Connected: whatever click there was is spent, asked or not (a key may have done it).
        self.prompts.disarm(&host);
        Ok(client
            .with_attachment(child.clone())
            .with_diagnostics(child))
    }

    /// A link step that was refused without being asked anything found no connection to ride:
    /// say so (the host needs the user to open one again).
    async fn unridden(
        &self,
        entry: &HostEntry,
        control: &Option<(PathBuf, bool)>,
        trail: &Trail,
        err: ConnectError,
    ) -> ConnectError {
        // A link step rides the shared connection and can never ask. Refused by ssh itself (its
        // own status, 255) for a host that needs the user, with no connection open: the host
        // needs the user to open one. (Decided from the connection, not from ssh's words.)
        let (
            Some((path, _)),
            ConnectError::Command {
                exit_code: Some(255),
                stderr,
                ..
            },
        ) = (control, &err)
        else {
            return err;
        };
        if !entry.interactive_auth {
            return err;
        }
        // The connection is looked at again: if it ended, why is told.
        if self
            .masters
            .check(entry, path, Some(trail.n()))
            .await
            .may_be_open()
        {
            return err;
        }
        let ended = self.masters.ended_words(&entry.id).unwrap_or_else(|| {
            "the shared connection to this host ended before this attempt could use it".to_owned()
        });
        cmd_err(
            CommandFailureKind::AuthNeeded,
            Some(255),
            format!("{ended}; ssh said: {stderr}"),
        )
    }

    /// Sends the helper for `platform`: `UPLOAD <n> <sha256>` and its bytes, the hash taken
    /// first from the file that is sent (a file replaced meanwhile fails the host's check, and
    /// nothing is installed). Bounded by progress only: the link state shows the bytes the host
    /// has taken so far.
    async fn upload<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
        &self,
        io: &mut S,
        trail: &Trail,
        platform: &HostPlatform,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
    ) -> Result<(), Failure> {
        trail.reach(BootStage::Helper);
        let file = self.artifact(platform).map_err(at(BootStage::Helper))?;
        let local = |e: std::io::Error| Failure {
            stage: BootStage::Helper,
            error: ConnectError::Io(format!("{}: {e}", file.display())),
        };
        let hashing = file.clone();
        let sha256 = tokio::task::spawn_blocking(move || bootwire::sha256_file(&hashing))
            .await
            .map_err(|e| local(std::io::Error::other(e)))?
            .map_err(local)?;
        let f = tokio::fs::File::open(&file).await.map_err(local)?;
        let len = f.metadata().await.map_err(local)?.len();
        trail.reach(BootStage::Upload);
        trail.action(HelperAction::Upload, Some(len), &platform.target);
        let began = Instant::now();
        let header = format!("{}\n", Decision::Upload { bytes: len, sha256 }.line());
        self.send(io, header.as_bytes(), BootStage::Upload, signin, child)
            .await?;
        let mut sent = 0u64;
        let mut from = f.take(len);
        let r = send_file(&mut from, io, &mut sent, |sent| {
            trail.tell(format!(
                "sending the Marsad helper to the host ({sent} of {len} bytes)"
            ))
        })
        .await;
        let r = match r {
            Ok(()) if sent < len => Err(SendError::Local(std::io::Error::other(format!(
                "the file shrank while it was sent ({sent} of {len} bytes)"
            )))),
            Ok(()) => io.flush().await.map_err(SendError::Remote),
            Err(e) => Err(e),
        };
        match r {
            Ok(()) => {
                trail.uploaded(len, began.elapsed());
                Ok(())
            }
            Err(SendError::Local(e)) => Err(local(e)),
            Err(SendError::Remote(e)) => Err(self
                .host_stopped_reading(
                    io,
                    trail,
                    signin,
                    child,
                    &format!(
                        "the host stopped reading while the Marsad helper was being sent ({sent} of {len} bytes)"
                    ),
                    &e,
                )
                .await),
        }
    }

    /// The host stopped reading the helper. When it said why in its own report (`MARSAD-FAIL`,
    /// e.g. `not_writable`: its bootstrap checked before it wrote), that report is the reason,
    /// and the hub words it. Otherwise its exit status and stderr are the reason (a full disk, a
    /// quota), not our broken pipe.
    ///
    /// WHY read only after the child exited: the report sits in the connection's buffer, and
    /// the stream ends with the child. Waiting first (bounded by [`CLOSED_EXIT_WAIT`]) means no
    /// line is awaited on a connection that is still open, so this cannot hang.
    async fn host_stopped_reading<S: tokio::io::AsyncRead + Unpin>(
        &self,
        io: &mut S,
        trail: &Trail,
        signin: &SignInAttempt,
        child: &wasit_control::ChildHandle,
        what: &str,
        e: &std::io::Error,
    ) -> Failure {
        if let Some(exit) = child.wait_exit(CLOSED_EXIT_WAIT).await {
            if let BootRead::Report(text) = read_boot_line(io, &trail.seen, REPORT_TAGS).await {
                let line = Line::parse(&text);
                if line.tag == bootwire::FAIL {
                    trail.seen.take();
                    return Failure {
                        stage: BootStage::Upload,
                        error: host_failed(&line, exit.code),
                    };
                }
            }
        }
        self.remote_gone(BootStage::Upload, signin, child, what, e)
            .await
    }
}

impl BootstrapConnector {
    /// The environment of an ssh command the hub runs: `SSH_ASKPASS` is this binary (every
    /// prompt it has comes to the user), `LC_ALL=C` (ssh's messages in English, as every ssh
    /// command of the hub), and `DISPLAY=:0` only when the hub has no display of its own.
    fn ssh_env(&self, exe: &Path, host: &HostId, signin: &SignInAttempt) -> Vec<(String, String)> {
        ssh_environment(
            exe,
            host,
            signin.tag(),
            &self.socket,
            &self.diag_dir,
            std::env::var_os("DISPLAY").as_deref(),
        )
    }

    /// What this sign-in asked decides whether the host needs the user at login: learned when it
    /// is true, un-learned when it stopped being true.
    async fn learn_login(&self, entry: &HostEntry, signin: &SignInAttempt) {
        match (signin.needed_user(), entry.interactive_auth) {
            (true, false) => {
                self.learn(Learned::Interactive {
                    host: entry.id.clone(),
                })
                .await
            }
            (false, true) => {
                self.learn(Learned::NotInteractive {
                    host: entry.id.clone(),
                })
                .await
            }
            _ => {}
        }
    }

    /// The shared connection of `entry` for this attempt, with the control socket path and
    /// whether this attempt opened it:
    ///
    /// 1. `ssh -O check` on the host's socket (local, no network).
    /// 2. If it is open, that is all: the attempt rides it.
    /// 3. If not, and the host needs the user at login (or they signed out) and they have not
    ///    asked for this attempt, the attempt fails as "needs the user" without starting
    ///    anything, whatever happened between the supervisor's question ([`hold`]) and now.
    /// 4. Otherwise the sign-in step: `ssh -M -N -n` (no `-f`) with the askpass helper, where
    ///    every prompt of the attempt happens. The hub owns that process until the connection
    ///    is open: ssh's detaching (status 0, once it authenticated) and the socket's appearance
    ///    are what [`Masters::sign_in`] waits for, with one check; a failure kills the process
    ///    the hub started, and nothing is bounded by time. A failure is classified from its exit
    ///    status and what ssh logged, as the one command of a host without a shared connection
    ///    is. Its human time is the prompts' (each bounded by the hub's prompt timeout) and its
    ///    machine time ssh's own `ConnectTimeout`; a slow host only takes longer, and the
    ///    attempt as a whole is bounded by the supervisor, as ever.
    ///
    /// [`hold`]: LinkConnector::hold
    async fn shared_connection(
        &self,
        spec: &HostSpec,
        entry: &HostEntry,
        exe: &Path,
        trail: &Trail,
        signin: &SignInAttempt,
    ) -> Result<(PathBuf, bool), Failure> {
        let host = &entry.id;
        let control = self
            .masters
            .path(entry)
            .map_err(ConnectError::Spawn)
            .map_err(at(BootStage::Prepare))?;
        self.masters
            .prepare()
            .await
            .map_err(|e| {
                ConnectError::Spawn(format!(
                    "cannot prepare the directory of the shared ssh connections: {e}"
                ))
            })
            .map_err(at(BootStage::Prepare))?;
        match self.masters.check(entry, &control, Some(trail.n())).await {
            Check::Alive => return Ok((control, false)),
            Check::Unanswered(words) => {
                return Err(Failure {
                    stage: BootStage::Prepare,
                    error: cmd_err(
                        CommandFailureKind::Other,
                        None,
                        format!("the shared connection to this host did not answer: {words}"),
                    ),
                })
            }
            Check::Absent => {}
        }
        let armed = self.prompts.armed(host);
        if !armed && self.masters.signed_out(host) {
            return Err(Failure {
                stage: BootStage::Prepare,
                error: signed_out_refusal(),
            });
        }
        if !armed && held_until_asked(entry) {
            return Err(Failure {
                stage: BootStage::Prepare,
                error: cmd_err(
                    CommandFailureKind::AuthNeeded,
                    None,
                    self.held_detail(entry),
                ),
            });
        }

        trail.reach(BootStage::SignIn);
        let generation = self.masters.begin_signin(entry, &control).await;
        emit(MarsadEvent::SshSignInStarted {
            host: trail.host.clone(),
            n: trail.n(),
            armed,
        });
        let began = Instant::now();
        let argv = bootstrap_argv_via(
            entry,
            self.target(host),
            SshRoute::SignIn { control: &control },
        )
        .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))
        .map_err(at(BootStage::Prepare))?;
        let cmd = SignInCommand {
            argv,
            env: self.ssh_env(exe, host, signin),
            link: spec.diag_link(),
            attempt: trail.n(),
        };
        match self.masters.sign_in(entry, &control, generation, cmd).await {
            SignInStep::Open => {}
            SignInStep::Unstarted(why) => {
                return Err(ConnectError::Spawn(why)).map_err(at(BootStage::Spawn));
            }
            SignInStep::SignedOut => {
                return Err(Failure {
                    stage: BootStage::SignIn,
                    error: signed_out_refusal(),
                })
            }
            SignInStep::Ended { exit, stderr } => {
                // With `-E` most of ssh's messages go to the connection's log; what it says
                // before it opens the log (an unreadable identity file) is on its stderr.
                let code = exit.code;
                let log = self.masters.signin_failure_text(&control).await;
                let text = [stderr.as_str(), log.as_str()]
                    .iter()
                    .filter(|t| !t.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                // The sign-in step is ssh with no remote command: its status is ssh's own.
                let kind = failure_kind(TransportKind::Ssh, code, signin, &text);
                let detail = if text.is_empty() {
                    format!("signing in to the host failed (exit {code:?})")
                } else {
                    text
                };
                return Err(Failure {
                    stage: BootStage::SignIn,
                    error: cmd_err(kind, code, detail),
                });
            }
            SignInStep::NoConnection { code } => {
                // Ended with status 0 and no connection, or killed because the check did not
                // find one: the same report either way.
                let why = self.masters.signin_failure_text(&control).await;
                return Err(Failure {
                    stage: BootStage::SignIn,
                    error: cmd_err(
                        CommandFailureKind::Other,
                        code,
                        if why.is_empty() {
                            "ssh signed in but left no shared connection open".to_owned()
                        } else {
                            format!("ssh signed in but left no shared connection open: {why}")
                        },
                    ),
                });
            }
        }
        if self.masters.signed_in(host, generation) {
            emit(MarsadEvent::SshSignInDone {
                host: trail.host.clone(),
                n: trail.n(),
                asked: signin.asked(),
                ms: diag::ms(began),
            });
            // The host answered and the user's sign-in worked: what it asked is learned now, and
            // the click that allowed it is spent.
            self.learn_login(entry, signin).await;
            self.prompts.disarm(host);
            Ok((control, true))
        } else {
            // Signed out during this sign-in: end the connection opened by this attempt and park
            // the host.
            self.masters.exit(entry, MasterExitWhy::SignedOut).await;
            Err(Failure {
                stage: BootStage::SignIn,
                error: signed_out_refusal(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(e: &ConnectError) -> CommandFailureKind {
        match e {
            ConnectError::Command { kind, .. } => *kind,
            other => panic!("not a command failure: {other:?}"),
        }
    }

    #[test]
    fn helper_for_a_gnu_linux_host_is_the_bundled_musl_build() {
        // The owner's report: a Linux server reports x86_64-unknown-linux-gnu; the Windows
        // installer ships agents\x86_64-unknown-linux-musl\marsad-agent.
        let dir = std::env::temp_dir().join(format!("marsad-pick-{}", std::process::id()));
        let musl = dir.join("x86_64-unknown-linux-musl");
        std::fs::create_dir_all(&musl).unwrap();
        std::fs::write(musl.join("marsad-agent"), b"x").unwrap();
        let linux = platform_from("Linux", "x86_64").unwrap();
        assert_eq!(linux.target, "x86_64-unknown-linux-gnu");
        assert_eq!(
            pick_helper("x86_64-pc-windows-msvc", Some(&dir), &linux).unwrap(),
            Helper::File(musl.join("marsad-agent"))
        );
        // The released Linux app's own sidecar is the musl build.
        assert_eq!(
            pick_helper("x86_64-unknown-linux-musl", Some(&dir), &linux).unwrap(),
            Helper::Running
        );
        // A gnu development build serves a same-arch Linux host itself when no musl build is
        // around.
        assert_eq!(
            pick_helper("x86_64-unknown-linux-gnu", None, &linux).unwrap(),
            Helper::Running
        );
        // ARM64 Linux, not in the bin dir: the installation is broken, not the host.
        let arm = platform_from("Linux", "aarch64").unwrap();
        let e = pick_helper("x86_64-pc-windows-msvc", Some(&dir), &arm).unwrap_err();
        assert_eq!(kind(&e), CommandFailureKind::HelperMissing);
        assert!(e.to_string().contains("Reinstall Marsad"), "{e}");
        assert!(!e.to_string().contains(r"\\?\"), "{e}");
        // No build exists for armv7 / riscv64: a configuration error naming os and arch.
        for arch in ["armv7l", "riscv64"] {
            let p = platform_from("Linux", arch).unwrap();
            let e = pick_helper("x86_64-pc-windows-msvc", Some(&dir), &p).unwrap_err();
            assert_eq!(kind(&e), CommandFailureKind::UnsupportedPlatform);
            assert!(e.to_string().contains(&format!("Linux on {arch}")), "{e}");
            assert!(kind(&e).is_configuration());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Reads `input` until the report; the report line (if any) and what the host sent.
    async fn from_host(input: &[u8]) -> (BootRead, Preamble) {
        let seen = Seen::default();
        let mut r = input;
        let read = read_boot_line(&mut r, &seen, FIRST_TAGS).await;
        (read, seen.take())
    }

    #[tokio::test]
    async fn bootstrap_report_after_a_banner_and_foreign_output() {
        let seen = Seen::default();
        let mut ok: &[u8] = b"Welcome!\nlast login: never\nMARSAD-BOOTX no\nMARSAD-SH\nrest";
        let BootRead::Report(line) = read_boot_line(&mut ok, &seen, FIRST_TAGS).await else {
            panic!("the report was there");
        };
        assert_eq!(line, "MARSAD-SH");
        // The banner is counted, line by line and byte by byte, and nothing past the report
        // was read.
        let preamble = seen.take();
        assert_eq!((preamble.lines, preamble.bytes), (3, 27 + 16));
        assert_eq!(
            preamble.text(),
            "Welcome!\nlast login: never\nMARSAD-BOOTX no\n"
        );
        assert_eq!(ok, b"rest");
        // The owner's report: wsl.exe's UTF-16LE error instead of the bootstrap.
        let utf16: Vec<u8> = "There is no distribution with the supplied name.\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let (read, preamble) = from_host(&utf16).await;
        assert!(matches!(read, BootRead::Missing(Missing::Ended)));
        let text = preamble.text();
        assert_eq!(
            text.trim(),
            "There is no distribution with the supplied name."
        );
        // Judged by WSL's own list, not by the words: the distribution is not among those named.
        let listed = |names: &[&str]| marsad_proto::hosts::WslDistros {
            available: true,
            distros: names
                .iter()
                .map(|n| marsad_proto::hosts::WslDistro {
                    name: (*n).into(),
                    default: false,
                    running: false,
                    version: None,
                })
                .collect(),
            problem: None,
        };
        assert_eq!(
            wsl_failure("Ubuntu", &listed(&["Debian"])),
            Some(CommandFailureKind::UnknownDistro)
        );
        assert!(
            text.contains("no distribution"),
            "the words are shown: {text}"
        );
    }

    #[tokio::test]
    async fn what_arrived_without_a_report_is_counted_whatever_ended_the_wait() {
        // The stream ends in the middle of a line (a banner without its newline).
        let (read, preamble) = from_host(b"one\ntwo\npartial").await;
        assert!(matches!(read, BootRead::Missing(Missing::Ended)));
        assert_eq!((preamble.lines, preamble.bytes), (3, 4 + 4 + 7));
        assert_eq!(preamble.describe(), "3 lines (15 bytes)");
        // Nothing at all.
        let (read, preamble) = from_host(b"").await;
        assert!(matches!(read, BootRead::Missing(Missing::Ended)));
        assert_eq!((preamble.lines, preamble.bytes), (0, 0));
        assert_eq!(preamble.describe(), "nothing");
        // More lines than the limit.
        let many = "x\n".repeat(MAX_PREAMBLE_LINES + 3);
        let (read, preamble) = from_host(many.as_bytes()).await;
        assert!(matches!(read, BootRead::Missing(Missing::TooManyLines)));
        assert_eq!(preamble.lines as usize, MAX_PREAMBLE_LINES);
        // A line that never ends.
        let long = "y".repeat(MAX_LINE + 10);
        let (read, preamble) = from_host(long.as_bytes()).await;
        assert!(matches!(read, BootRead::Missing(Missing::LineTooLong)));
        assert_eq!((preamble.lines, preamble.bytes), (1, MAX_LINE as u64 + 1));
    }

    /// The supervisor drops the connector's future when its bound runs out or the link is
    /// stopped: the trail records, when it is dropped, that the attempt did not finish, where it
    /// was, and what the host had printed so far (also the line it was in the middle of).
    #[tokio::test]
    async fn an_attempt_cancelled_while_waiting_for_the_report_records_what_arrived() {
        use std::future::Future as _;
        use wasit_diag::SinkConfig;
        let dir = std::env::temp_dir().join(format!("mrs-boot-trail-{}", std::process::id()));
        wasit_diag::install(SinkConfig::new(
            Code::st("hub"),
            Ident::st("boottest"),
            &dir,
            "boot.ndjson",
        ))
        .unwrap();
        let (mut host_end, mut hub_end) = tokio::io::duplex(1024);
        {
            let trail = Trail::new(&HostId::new("stuck"), Attempt::new(7));
            trail.expect_report();
            host_end
                .write_all(b"Welcome\nsecond line\npartial banner")
                .await
                .unwrap();
            let mut wait = Box::pin(read_boot_line(&mut hub_end, &trail.seen, FIRST_TAGS));
            // One poll takes what is there and then waits for more: the host is silent.
            let polled =
                std::future::poll_fn(|cx| std::task::Poll::Ready(wait.as_mut().poll(cx))).await;
            assert!(polled.is_pending());
            // The connect bound ran out: the future is dropped, and then the trail.
            drop(wait);
        }
        assert!(wasit_diag::flush(Duration::from_secs(30)));
        let text = std::fs::read_to_string(dir.join("boot.ndjson")).unwrap();
        let recs: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .filter(|r: &serde_json::Value| r["host"] == "stuck")
            .collect();
        let evs: Vec<&str> = recs.iter().map(|r| r["ev"].as_str().unwrap()).collect();
        assert_eq!(evs, ["boot.preamble", "boot.failed"], "{text}");
        assert_eq!(
            (&recs[0]["lines"], &recs[0]["bytes"]),
            (&serde_json::json!(3), &serde_json::json!(8 + 12 + 14))
        );
        assert!(recs[0]["shape"].as_str().unwrap().starts_with("Welcome"));
        assert_eq!(
            (&recs[1]["stage"], &recs[1]["n"]),
            (&serde_json::json!("report"), &serde_json::json!(7))
        );
        assert_eq!(recs[1]["error"]["code"], "cancelled");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn report(helper: Option<&str>, daemon: DaemonState) -> Report {
        Report {
            os: "Linux".into(),
            arch: "x86_64".into(),
            helper_build: helper.map(Into::into),
            helper_hash: None,
            hasher: Some("sha256sum".into()),
            daemon,
            daemon_build: Some("own".into()),
            daemon_host: Some("box".into()),
            daemon_pid: Some(7),
            daemon_hub: Some("hub1".into()),
            machine: Some("m".into()),
            installed: false,
        }
    }

    #[test]
    fn one_decision_per_report() {
        let host = HostId::new("box");
        let d = |r: &Report, twin: Option<&str>, uploaded: bool| {
            decide(r, "own", "hub1", &host, twin, uploaded)
        };
        // Nothing installed: upload, once, and only with a checksum tool.
        let none = report(None, DaemonState::Unknown);
        assert_eq!(d(&none, None, false), Choice::Upload);
        assert!(matches!(d(&none, None, true), Choice::Refuse(_)));
        let no_tool = Report {
            hasher: None,
            ..none.clone()
        };
        let Choice::Refuse(w) = d(&no_tool, None, false) else {
            panic!("no checksum tool: refused")
        };
        assert!(w.contains("sha256sum, shasum or openssl"), "{w}");
        // A helper of another build where ours belongs.
        assert!(matches!(
            d(&report(Some("other"), DaemonState::Absent), None, true),
            Choice::Refuse(_)
        ));
        // Our helper: run, whatever runs or not.
        for st in [DaemonState::Absent, DaemonState::Running] {
            assert_eq!(d(&report(Some("own"), st), None, false), Choice::Run);
        }
        let installed = Report {
            installed: true,
            ..report(Some("own"), DaemonState::Absent)
        };
        assert_eq!(d(&installed, None, true), Choice::Run);
        // A daemon of another build, of another id, or one that does not answer: restart.
        let mut r = report(Some("own"), DaemonState::Running);
        r.daemon_build = Some("old".into());
        assert_eq!(d(&r, None, false), Choice::Restart(RestartWhy::Build));
        // An earlier build's daemon does not say which hub it serves: it is taken over.
        r.daemon_hub = None;
        assert_eq!(d(&r, None, false), Choice::Restart(RestartWhy::Build));
        let mut r = report(Some("own"), DaemonState::Running);
        r.daemon_host = Some("elsewhere".into());
        assert_eq!(d(&r, None, false), Choice::Restart(RestartWhy::Host));
        // ... unless that id is a twin: the same machine added twice.
        assert!(matches!(d(&r, Some("Twin"), false), Choice::Refuse(w) if w.contains("\"Twin\"")));
        assert_eq!(
            d(&report(Some("own"), DaemonState::Unreachable), None, false),
            Choice::Restart(RestartWhy::Unreachable)
        );
        // Another hub's daemon is never taken over, whatever its build.
        let mut r = report(Some("own"), DaemonState::Running);
        r.daemon_hub = Some("hub2".into());
        r.daemon_build = Some("old".into());
        let Choice::Refuse(w) = d(&r, None, false) else {
            panic!("another hub's daemon: refused")
        };
        assert!(
            w.contains("another Marsad hub (hub2)") && w.contains("pid 7"),
            "{w}"
        );
    }

    /// A host the user signed out of, with no connection open and no attempt in flight, is refused
    /// its sign-in as signed out: the same refusal an attempt that was signed out mid-sign-in gets
    /// (so the supervisor parks it with `held: true`), never as a failed attempt that asked for a
    /// person (`held: false`).
    #[tokio::test]
    async fn an_attempt_for_a_signed_out_host_is_refused_as_signed_out() {
        let dir = std::env::temp_dir().join(format!("mrs-signed-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let masters = Masters::new(dir.join("ssh"));
        let id = HostId::new("signedout");
        let entry = HostEntry {
            id: id.clone(),
            name: "so".into(),
            kind: HostKind::Ssh {
                target: "u@127.0.0.1".into(),
                port: None,
                identity_file: None,
                options: Vec::new(),
            },
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            ask_before_connecting: false,
            local_files: false,
            shared_connection: false,
            signed_out: true,
            machine: None,
        };
        let entries: Entries = Arc::new(RwLock::new(HashMap::from([(id.clone(), entry.clone())])));
        let (learned, _learned_rx) = mpsc::channel(8);
        let prompts = Prompts::new(crate::hub::fleet::Fleet::spawn(), Duration::from_secs(5));
        let connector = BootstrapConnector {
            entries,
            learned,
            prompts: prompts.clone(),
            links: LinkChildren::default(),
            hub_id: "0123456789abcdef".into(),
            masters: masters.clone(),
            own: VersionInfo {
                build: "test".into(),
                target: "test".into(),
            },
            bin_dir: None,
            socket: dir.join("hub.sock"),
            diag_dir: dir.join("diag"),
        };
        masters.mark_signed_out(&id);
        masters.prepare().await.unwrap();
        let spec = HostSpec::new(id.clone());
        let trail = Trail::new(&id, Attempt::new(1));
        let signin = prompts.begin_attempt(&id, None);
        let Err(failure) = connector
            .shared_connection(
                &spec,
                &entry,
                Path::new("/nonexistent/askpass"),
                &trail,
                &signin,
            )
            .await
        else {
            panic!("a signed-out host must not sign in");
        };
        match failure.error {
            ConnectError::Refused(r) => {
                assert_eq!(r.code, "auth_needed");
                assert_eq!(r.message, SIGNED_OUT_DETAIL);
            }
            other => panic!("the signed-out refusal must be Refused, not {other:?}"),
        }
    }

    /// U5: the kind of a failed command comes from the transport's exit status and what the
    /// sign-in asked. ssh's own failure (255) with nothing asked is `Other` (retried), unless
    /// its fixed words say a person is needed ([`ssh_refusal`]).
    #[test]
    fn a_failure_is_decided_by_its_status_and_its_prompts() {
        let nothing = Asks::default();
        let beyond = Asks {
            beyond_host_key: true,
            ..Asks::default()
        };
        let user = Asks {
            beyond_host_key: true,
            needs_user: true,
            host_key_refused: false,
        };
        let refused = Asks {
            host_key_refused: true,
            ..Asks::default()
        };
        let no_words = "";
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(255), nothing, no_words),
            CommandFailureKind::Other
        );
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(255), user, no_words),
            CommandFailureKind::AuthNeeded
        );
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(255), refused, no_words),
            CommandFailureKind::HostKeyUnknown
        );
        // A prompt cancelled or timed out makes ssh give up by a signal or a status of its own.
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, None, beyond, no_words),
            CommandFailureKind::AuthNeeded
        );
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, None, nothing, no_words),
            CommandFailureKind::Other
        );
        // ssh ran the remote program (its status is that program's): the sign-in went through,
        // so the status decides, whatever was asked.
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(127), user, no_words),
            CommandFailureKind::CommandNotFound
        );
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(126), nothing, no_words),
            CommandFailureKind::PermissionDenied
        );
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(1), beyond, no_words),
            CommandFailureKind::HelperFailed
        );
        // wsl.exe's and other commands' statuses are not known to be the remote program's.
        assert_eq!(
            failure_kind_of(TransportKind::Wsl, Some(127), nothing, no_words),
            CommandFailureKind::Other
        );
        assert_eq!(
            failure_kind_of(TransportKind::Other, Some(127), nothing, no_words),
            CommandFailureKind::Other
        );
    }

    /// Requirement 1 (E3): ssh's exit 255 with no prompt is retried with backoff, except the two
    /// refusals that need a person: a changed host key and refused authentication, found by
    /// ssh's fixed messages. Network-level failures stay retried. Red without [`ssh_refusal`].
    #[test]
    fn ssh_refusals_that_need_a_person_park_and_network_failures_retry() {
        let changed = "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
            @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n\
            @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
            Host key verification failed.\n";
        let refused = "box: Permission denied (publickey,password).\n";
        let retried = [
            "ssh: Could not resolve hostname box: Name or service not known\n",
            "ssh: connect to host box port 22: Connection refused\n",
            "ssh: connect to host box port 22: Connection timed out\n",
            "kex_exchange_identification: Connection closed by remote host\n",
        ];
        let nothing = Asks::default();
        let ssh = |words: &str| failure_kind_of(TransportKind::Ssh, Some(255), nothing, words);
        assert_eq!(ssh(changed), CommandFailureKind::HostKeyChanged);
        assert_eq!(ssh(refused), CommandFailureKind::AuthNeeded);
        assert_eq!(
            ssh("box: Permission denied, please try again.\n"),
            CommandFailureKind::AuthNeeded
        );
        // The server's attempt limit is spent: another try now only adds to the lockout.
        assert_eq!(
            ssh("Received disconnect from 192.0.2.7 port 22:2: Too many authentication failures\n"),
            CommandFailureKind::AuthNeeded
        );
        // A declined or unanswered host key, reported by ssh's own line (no changed-key banner).
        assert_eq!(
            ssh("Host key verification failed.\n"),
            CommandFailureKind::HostKeyUnknown
        );
        for words in retried {
            assert_eq!(ssh(words), CommandFailureKind::Other, "{words}");
        }
        // The words only count for ssh's own status 255, and for ssh (not wsl.exe or others).
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(1), nothing, changed),
            CommandFailureKind::HelperFailed,
            "a remote status is the remote program's, whatever its words"
        );
        assert_eq!(ssh_refusal(TransportKind::Ssh, None, changed), None);
        assert_eq!(ssh_refusal(TransportKind::Wsl, Some(255), refused), None);
        // A changed key outranks a refusal in the same output.
        assert_eq!(
            ssh(&format!("{changed}{refused}")),
            CommandFailureKind::HostKeyChanged
        );
    }

    /// A host that cannot store the helper says so with a structural reason; the hub words the
    /// refusal itself, with the same hint as a host that refuses to run it. Any other reason
    /// keeps the host's own message. Red without the `not_writable` mapping in [`host_failed`].
    #[test]
    fn a_helper_directory_that_is_not_writable_gets_the_hub_hint() {
        let not_writable = host_failed(
            &Line::parse("MARSAD-FAIL stage=install reason=not_writable"),
            Some(70),
        );
        assert_eq!(kind(&not_writable), CommandFailureKind::PermissionDenied);
        let ConnectError::Command { stderr, .. } = &not_writable else {
            unreachable!("checked by kind above")
        };
        assert!(
            stderr.contains("Check that your home directory allows running programs"),
            "{stderr}"
        );
        assert_eq!(
            stderr,
            &permission_denied_detail("the helper directory on the host is not writable")
        );
        let other = host_failed(
            &Line::parse("MARSAD-FAIL stage=install reason=checksum%20mismatch"),
            None,
        );
        assert_eq!(kind(&other), CommandFailureKind::HelperFailed);
    }

    /// Requirement 2 (E3): a host key the user accepted and a later failure is not a host-key
    /// refusal; only a declined or unanswered host-key question parks as one. Red without
    /// `host_key_refused` taking the accepted answer into account.
    #[test]
    fn a_host_key_the_user_accepted_is_not_refused_by_a_later_failure() {
        let accepted_then_network = Asks {
            host_key_refused: false,
            ..Asks::default()
        };
        assert_eq!(
            failure_kind_of(
                TransportKind::Ssh,
                Some(255),
                accepted_then_network,
                "ssh: connect to host box port 22: Connection refused\n"
            ),
            CommandFailureKind::Other,
            "an accepted key then a network failure is retried, not parked as a host key"
        );
        let declined = Asks {
            host_key_refused: true,
            ..Asks::default()
        };
        assert_eq!(
            failure_kind_of(TransportKind::Ssh, Some(255), declined, ""),
            CommandFailureKind::HostKeyUnknown
        );
    }

    /// Requirement 3 (E3): what a host sent before its report is the reason shown when its own
    /// status ended the attempt, so its stderr tail (the broken startup line) is always in it.
    #[test]
    fn what_a_host_sent_without_a_report_keeps_its_stderr_tail() {
        let rc = "bash: /home/u/.bashrc: line 3: syntax error near unexpected token `fi'";
        assert_eq!(sent_words("", rc), rc);
        assert_eq!(sent_words("banner", rc), format!("banner\n{rc}"));
        assert_eq!(sent_words("banner", ""), "banner");
        assert_eq!(sent_words("", ""), "");
    }

    /// U16: the hub gives its ssh commands `DISPLAY=:0` only when it has no display (a forced
    /// value clobbered a real one); `LC_ALL=C` always (ssh's messages in English).
    #[test]
    fn the_ssh_environment_keeps_the_hubs_display_and_asks_for_english() {
        use std::ffi::OsStr;
        let get = |env: &[(String, String)], k: &str| {
            env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
        };
        let make = |display: Option<&OsStr>| {
            ssh_environment(
                Path::new("/bin/marsad-agent"),
                &HostId::new("h"),
                7,
                Path::new("/run/s"),
                Path::new("/run/d"),
                display,
            )
        };
        let none = make(None);
        assert_eq!(get(&none, "DISPLAY").as_deref(), Some(":0"));
        assert_eq!(get(&none, "LC_ALL").as_deref(), Some("C"));
        assert_eq!(get(&none, "SSH_ASKPASS_REQUIRE").as_deref(), Some("force"));
        assert_eq!(get(&none, ASKPASS_ATTEMPT_ENV).as_deref(), Some("7"));
        let set = make(Some(OsStr::new(":1")));
        assert_eq!(get(&set, "DISPLAY"), None, "the hub's own display is kept");
        assert_eq!(get(&set, "LC_ALL").as_deref(), Some("C"));
        assert_eq!(
            display_fallback(Some(OsStr::new(""))),
            Some(("DISPLAY".into(), ":0".into())),
            "an empty display is none"
        );
    }

    /// G11: a host's next link command waits, by its own exit event, for the previous one to end
    /// (a server with one session at a time refuses the second while the first is open). While
    /// the previous command's stdin is open it cannot end, so the wait is pending.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_reconnect_waits_for_the_previous_link_command_to_end() {
        use std::future::Future;
        use std::task::Poll;
        let links = LinkChildren::default();
        let host = HostId::new("h");
        let spec = CommandSpec::new(["sh", "-c", "read line"]);
        let (io, child) = CommandTransport::spawn(&spec).expect("sh starts");
        let child = Arc::new(child);
        links.remember(&host, child.clone());
        assert!(links.pending(&host));
        let mut settle = Box::pin(links.settle(&host));
        let still_pending =
            std::future::poll_fn(|cx| Poll::Ready(settle.as_mut().poll(cx).is_pending())).await;
        assert!(
            still_pending,
            "the next attempt must not start while the previous command runs"
        );
        // Closing its stdin ends `read`, and with it the command.
        drop(io);
        settle.await;
        assert!(
            child.exit().is_some(),
            "the wait ended with the command's exit"
        );
        assert!(
            !links.pending(&host),
            "the previous command is waited for once"
        );
    }

    /// A previous link command that ignores hangup and termination and never ends on its own (a
    /// wedged ssh) is ended by the reconnect, which then proceeds. Its stdin stays open, so
    /// closing the stream does not end it either. The timeout is only the test's guard: a
    /// regression fails here instead of hanging the suite.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_reconnect_ends_a_wedged_previous_link_command() {
        let links = LinkChildren::default();
        let host = HostId::new("h");
        let spec = CommandSpec::new(["sh", "-c", "trap '' HUP TERM; while :; do sleep 0.1; done"]);
        let (io, child) = CommandTransport::spawn(&spec).expect("sh starts");
        let child = Arc::new(child);
        links.remember(&host, child.clone());
        assert!(child.exit().is_none(), "the wedged command is running");
        tokio::time::timeout(Duration::from_secs(20), links.settle(&host))
            .await
            .expect("the reconnect proceeds once the wedged command is ended");
        assert!(
            child.exit().is_some(),
            "the previous command has exited before the next attempt starts"
        );
        assert!(!links.pending(&host));
        drop(io);
    }

    /// The connector of the tests: `entry` is its only host.
    fn connector_for(entry: HostEntry, dir: &Path) -> (BootstrapConnector, Prompts) {
        let id = entry.id.clone();
        let entries: Entries = Arc::new(RwLock::new(HashMap::from([(id, entry)])));
        let (learned, _rx) = mpsc::channel(8);
        let prompts = Prompts::new(crate::hub::fleet::Fleet::spawn(), Duration::from_secs(5));
        let connector = BootstrapConnector {
            entries,
            learned,
            prompts: prompts.clone(),
            links: LinkChildren::default(),
            hub_id: "0123456789abcdef".into(),
            masters: Masters::new(dir.join("ssh")),
            own: VersionInfo {
                build: "test".into(),
                target: "test".into(),
            },
            bin_dir: None,
            socket: dir.join("hub.sock"),
            diag_dir: dir.join("diag"),
        };
        (connector, prompts)
    }

    /// G4: a host set to be asked before it connects is held whatever its connection does, until
    /// the user connects it (the click arms the next attempt); a host not so set is not held by
    /// that rule (here a WSL host: no shared connection, nothing learned that needs the user).
    #[tokio::test]
    async fn a_host_asked_before_connecting_is_held_until_the_user_connects_it() {
        let dir = std::env::temp_dir().join(format!("mrs-ask-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut entry = HostEntry {
            id: HostId::new("asked"),
            name: "asked".into(),
            kind: HostKind::Wsl {
                distro: "Ubuntu".into(),
            },
            platform: None,
            created_at_ms: 0,
            interactive_auth: false,
            ask_before_connecting: true,
            local_files: false,
            shared_connection: false,
            signed_out: false,
            machine: None,
        };
        let spec = HostSpec::new(entry.id.clone());
        let (connector, prompts) = connector_for(entry.clone(), &dir);
        let held = connector.hold(&spec).await;
        assert_eq!(held.as_deref(), Some(HELD_ASKED_DETAIL), "held: {held:?}");
        prompts.arm(&entry.id);
        assert_eq!(
            connector.hold(&spec).await,
            None,
            "the click lets the attempt run"
        );
        // Not set: an attempt is not held by this rule.
        entry.ask_before_connecting = false;
        let (plain, _) = connector_for(entry, &dir);
        assert_eq!(plain.hold(&spec).await, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
