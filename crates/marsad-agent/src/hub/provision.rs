//! Connecting to and provisioning a host over ONE command per attempt (ARCHITECTURE §4.1,
//! with the owner's interactive-auth rule). An ssh host on a Linux or macOS hub has a shared
//! connection ([`super::master`]): that command is the link step, riding the connection, and
//! the attempt first looks for it and, if none is open and signing in is allowed, opens it in
//! a sign-in step where every prompt happens ([`BootstrapConnector::shared_connection`]).
//!
//! [`BootstrapConnector`] (the hub's `LinkConnector`) runs `<transport> <bootstrap>` (see
//! [`super::transport`]) with `SSH_ASKPASS` pointing at this binary, reads the bootstrap's
//! report (OS, arch, installed build), uploads the matching binary through the same channel
//! when the build differs, and then speaks the control protocol over it. A failed attempt is
//! classified from ssh's exit status and stderr (wasit-control's classifier), so anything
//! needing a human parks the link in `auth_needed`.
//!
//! A host that needs the user at login (a connect asked them for something other than a
//! host-key confirmation, see [`marsad_proto::hosts::PromptKind::needs_user`]) is never tried
//! on the connector's own account: the link's supervisor asks [`BootstrapConnector::hold`]
//! before every attempt it would start by itself, and a host that needs the user and has not
//! been asked for (`hosts.reconnect` arms the next attempt) is parked in `auth_needed` instead,
//! so a dropped link, a hub start or a retry never fires a prompt or a push on its own, unless
//! the host's shared connection is still open: then nothing needs asking and the link
//! reconnects by itself. The
//! click is spent when an attempt asks the user something or connects, not when it starts: an
//! attempt that fails before the host answered (no network) asked nobody anything, and the link
//! goes on trying, armed. The fact is learned and un-learned here, once per successful sign-in
//! (see [`Learned`]).
//!
//! Every attempt records its steps (`boot.*`, see [`Trail`]) under the attempt number the
//! supervisor passes in, and every failure says where it happened and why: the transport
//! child's exit status and stderr tail for anything the remote side did (a refused install, a
//! helper that cannot run), what the host printed instead of its report, and an attempt cut
//! short by the supervisor leaves what it had seen, and tells the supervisor the step it had
//! reached (its link state then says where the attempt was, not only that it timed out). A
//! failed handshake after the bootstrap said OK is explained by wasit-control from the transport
//! child (`ChildHandle::explain_handshake_failure`): a helper that ended by itself parks the
//! link with its words (`CommandFailureKind::HelperFailed`) instead of being retried for ever.
//!
//! [`AgentProvisioner`] handles what only the hello reveals (a daemon of another build still
//! running): it marks the host so the next attempt sends `RESTART`.
//!
//! Binaries: the host's (os, arch) picks the shipped helper triple
//! ([`super::transport::helper_target`]: any Linux → the static musl build); that is the
//! running executable when it was built for that triple, else
//! `MARSAD_AGENT_BIN_DIR/<triple>/marsad-agent[.exe]` (assumed to be the hub's build), else —
//! development builds — the running executable when it runs on the same (os, arch).
//! No build for the platform, or a missing file, fails the link (no retry loop) with a message
//! saying what to do.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use marsad_proto::daemon::VersionInfo;
use marsad_proto::diag::{
    BootShell, BootStage, HelperAction, HostsOp, MarsadEvent, MasterExitWhy, WindowsRetryWhy,
};
use marsad_proto::hosts::{HostEntry, HostKind, HostPlatform, SignInRepeat};
use marsad_proto::HostId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use wasit_control::wire::RpcError;
use wasit_control::{
    classify_command_failure, Attempt, ClientOptions, CommandFailureKind, CommandSpec,
    CommandTransport, ConnectDiag, ConnectError, ControlClient, HostSpec, LinkConnector,
    ProvisionError, ProvisionReason, Provisioner, CLOSED_EXIT_WAIT,
};
use wasit_diag::{Bytes, Code, Ident, Ms};

use super::auth::{Prompts, SignInAttempt};
use super::master::{Check, Masters};
use super::transport::{
    bootstrap_argv_via, helper_target, native_windows_sign, platform_from, triple_platform,
    RemoteShell, SshRoute,
};
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

/// Hosts whose next attempt must restart the remote daemon, and why (the user is told when that
/// asks them to sign in again).
pub type Restarts = Arc<Mutex<HashMap<HostId, SignInRepeat>>>;

pub struct BootstrapConnector {
    pub entries: Entries,
    pub learned: mpsc::Sender<Learned>,
    pub prompts: Prompts,
    pub restarts: Restarts,
    /// The shared ssh connections of ssh hosts (a Windows hub has none, see [`Masters`]).
    pub masters: Masters,
    pub own: VersionInfo,
    pub bin_dir: Option<PathBuf>,
    /// The hub's endpoint path (for the askpass helper).
    pub socket: PathBuf,
    /// This machine's diagnostics directory (for the askpass helper).
    pub diag_dir: PathBuf,
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

/// Read one line of the bootstrap's answers (`None`: the stream ended).
async fn read_line<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> std::io::Result<Option<String>> {
    let line = Mutex::new(Vec::new());
    match read_raw_line(r, &line).await? {
        LineEnd::Newline => Ok(Some(
            String::from_utf8_lossy(&locked(&line))
                .trim_end_matches('\r')
                .to_string(),
        )),
        LineEnd::Eof => Ok(None),
        LineEnd::TooLong => Err(std::io::Error::other("bootstrap line too long")),
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
    /// The `MARSAD-BOOT …` line.
    Report(String),
    /// It did not come.
    Missing(Missing),
}

/// Read until the `MARSAD-BOOT …` line, keeping what came before it in `seen` as it arrives.
async fn read_boot_line<R: tokio::io::AsyncRead + Unpin>(r: &mut R, seen: &Seen) -> BootRead {
    for _ in 0..MAX_PREAMBLE_LINES {
        let why = match read_raw_line(r, &seen.line).await {
            Ok(LineEnd::Newline) => {
                let line = std::mem::take(&mut *locked(&seen.line));
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches('\r');
                if text.starts_with("MARSAD-BOOT ") {
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

/// The bootstrap's report line, parsed.
#[derive(Debug)]
struct BootReport {
    os: String,
    arch: String,
    installed: Option<VersionInfo>,
}

impl BootReport {
    fn parse(line: &str) -> BootReport {
        let mut parts = line.split_whitespace();
        parts.next(); // MARSAD-BOOT
        let os = parts.next().unwrap_or("").to_owned();
        let arch = parts.next().unwrap_or("").to_owned();
        let installed = parts
            .next()
            .and_then(|v| serde_json::from_str::<VersionInfo>(v).ok());
        BootReport {
            os,
            arch,
            installed,
        }
    }
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
            BootStage::Ok => "waiting for the host to confirm the helper",
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

    fn attempt(&self, shell: RemoteShell, entry: &HostEntry, armed: bool) {
        emit(MarsadEvent::BootAttempt {
            host: self.host.clone(),
            n: self.n(),
            shell: match shell {
                RemoteShell::Posix => BootShell::Posix,
                RemoteShell::PowerShell => BootShell::Powershell,
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

    fn report(&self, r: &BootReport) {
        emit(MarsadEvent::BootReport {
            host: self.host.clone(),
            n: self.n(),
            os: Code::new(&r.os),
            arch: Code::new(&r.arch),
            installed: r.installed.as_ref().map(|v| Ident::new(&v.build)),
            ms: self.since(),
        });
    }

    fn windows_retry(&self, why: WindowsRetryWhy) {
        emit(MarsadEvent::BootWindowsRetry {
            host: self.host.clone(),
            n: self.n(),
            why,
        });
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

/// Copy `from` to `to` in chunks; `sent` counts what the host's end took.
async fn send_file<R, W>(from: &mut R, to: &mut W, sent: &mut u64) -> Result<(), SendError>
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
        let host = l.host().clone();
        if self.learned.send(l).await.is_err() {
            emit(MarsadEvent::HostsFailed {
                host: Some(Ident::new(host.as_str())),
                op: HostsOp::RegistryGone,
                error: diag::ours("registry_gone", &"the hosts registry is gone"),
            });
        }
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
        let mut kind = classify_command_failure(code, &stderr);
        if signin.asked() > 0 && kind == CommandFailureKind::Other {
            // A cancelled or timed-out prompt makes ssh give up without a recognisable message.
            kind = CommandFailureKind::AuthNeeded;
        }
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
        let both = match (stdout.is_empty(), stderr.is_empty()) {
            (true, _) => stderr.clone(),
            (false, true) => stdout.to_string(),
            (false, false) => format!("{stdout}\n{stderr}"),
        };
        let mut kind = classify_command_failure(code, &both);
        if signin.asked() > 0 && kind == CommandFailureKind::Other {
            // A cancelled or timed-out prompt makes ssh give up without a recognisable message.
            kind = CommandFailureKind::AuthNeeded;
        }
        if kind == CommandFailureKind::Other && !stdout.is_empty() {
            kind = CommandFailureKind::UnexpectedOutput;
        }
        let detail = match kind {
            CommandFailureKind::UnknownDistro => {
                let distro = match &entry.kind {
                    HostKind::Wsl { distro } => distro.as_str(),
                    _ => "",
                };
                let list = super::wsl::distros().await;
                let names: Vec<&str> = list.distros.iter().map(|d| d.name.as_str()).collect();
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
            CommandFailureKind::PermissionDenied => format!(
                "The host refused to run the Marsad helper (permission denied). Check that your home directory allows running programs (for example a noexec mount), then try again. ({both})"
            ),
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

/// The remote command ran (the connection and sign-in worked) but printed no bootstrap report.
fn remote_command_ran(e: &ConnectError) -> bool {
    matches!(
        e,
        ConnectError::Command {
            kind: CommandFailureKind::Other
                | CommandFailureKind::CommandNotFound
                | CommandFailureKind::UnexpectedOutput
                | CommandFailureKind::PermissionDenied,
            ..
        }
    )
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
    /// Why `entry` waits for the user, in words.
    fn held_detail(&self, entry: &HostEntry) -> String {
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
        if Masters::applies(&entry) {
            if self.masters.signed_out(&spec.id) {
                // The user's own word: no look at the connection is needed.
            } else {
                // A path that cannot be made is the attempt's to report.
                let control = self.masters.path(&entry).ok()?;
                if self.masters.check(&entry, &control, None).await == Check::Alive {
                    return None;
                }
                if !entry.interactive_auth {
                    return None;
                }
            }
        } else if !entry.interactive_auth {
            return None;
        }
        emit(MarsadEvent::HostsHeld {
            host: Ident::new(spec.id.as_str()),
        });
        Some(self.held_detail(&entry))
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

impl BootstrapConnector {
    /// One connect attempt, step by step; `trail` records each step (see [`Trail`]).
    async fn run(
        &self,
        spec: &HostSpec,
        opts: ClientOptions,
        trail: &Trail,
    ) -> Result<ControlClient, Failure> {
        let entry = self.entry(&spec.id).map_err(at(BootStage::Prepare))?;
        let host = entry.id.clone();
        let mut shell = RemoteShell::for_entry(&entry);
        // (The supervisor asked `hold` before it started this attempt: a host that needs the
        // user is only here because the user asked, or because this follows their sign-in.)
        trail.attempt(shell, &entry, self.prompts.armed(&host));
        let exe = std::env::current_exe()
            .map_err(|e| ConnectError::Spawn(e.to_string()))
            .map_err(at(BootStage::Prepare))?;
        // This attempt's prompts are its own: they end with it, whichever way it ends. A restart
        // the previous attempt asked for makes this a second sign-in of one Connect: its prompts
        // say so.
        let repeat = locked(&self.restarts).get(&host).copied();
        let signin = self.prompts.begin_attempt(&host, repeat);
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
        // A host never connected is tried with `sh`. One that turns out to be native Windows
        // is asked again at once with PowerShell (its platform is then learned): its `sh` is a
        // Windows port that reports MINGW/MSYS/CYGWIN, or the remote command ran but printed no
        // report (cmd.exe has no `sh` and mangles the POSIX line; its messages are localised,
        // so any such failure gets one PowerShell attempt, and a host where that fails too
        // keeps the POSIX attempt's error).
        let mut posix_failed: Option<ConnectError> = None;
        let (mut io, child, report) = loop {
            let route = match &control {
                Some(c) => SshRoute::Link { control: &c.0 },
                None => SshRoute::Plain,
            };
            let argv = bootstrap_argv_via(&entry, shell, route)
                .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))
                .map_err(at(BootStage::Prepare))?;
            // The transport's spawn and exit are recorded under this link and attempt.
            let mut cmd = CommandSpec::new(argv).traced(spec.diag_link(), &trail.attempt);
            cmd.env = self.askpass_env(&exe, &host, &signin);
            let (mut io, child) = CommandTransport::spawn(&cmd).map_err(at(BootStage::Spawn))?;
            let child = Arc::new(child);
            // Only a host that never connected (once connected, its platform is known).
            let first_posix = shell == RemoteShell::Posix
                && entry.platform.is_none()
                && matches!(entry.kind, HostKind::Ssh { .. });
            trail.expect_report();
            match read_boot_line(&mut io, &trail.seen).await {
                BootRead::Report(line) => {
                    trail.record_preamble();
                    let report = BootReport::parse(&line);
                    trail.report(&report);
                    if first_posix && native_windows_sign(&report.os) {
                        trail.windows_retry(WindowsRetryWhy::MingwSh);
                        shell = RemoteShell::PowerShell;
                        // Another connection: the user may be asked again, and is told why
                        // (not over a shared connection: it is the same one).
                        if control.is_none() {
                            signin.again(SignInRepeat::WindowsShell);
                        }
                        continue;
                    }
                    break (io, child, report);
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
                    // A link step that was refused without being asked anything found no
                    // connection to ride: say so (the host needs the user to open one again).
                    let err = match (&control, err) {
                        (
                            Some((path, _)),
                            ConnectError::Command {
                                kind: CommandFailureKind::AuthNeeded,
                                exit_code,
                                stderr,
                            },
                        ) => {
                            // The connection is looked at again: if it ended, why is told.
                            self.masters.check(&entry, path, Some(trail.n())).await;
                            let ended = self.masters.ended_words(&host).unwrap_or_else(|| {
                                "the shared connection to this host ended before this attempt could use it"
                                    .to_owned()
                            });
                            cmd_err(
                                CommandFailureKind::AuthNeeded,
                                exit_code,
                                format!("{ended}; ssh said: {stderr}"),
                            )
                        }
                        (_, err) => err,
                    };
                    if first_posix && remote_command_ran(&err) {
                        trail.windows_retry(WindowsRetryWhy::NoReport);
                        posix_failed = Some(err);
                        shell = RemoteShell::PowerShell;
                        if control.is_none() {
                            signin.again(SignInRepeat::WindowsShell);
                        }
                        continue;
                    }
                    return Err(Failure {
                        stage: BootStage::Report,
                        error: match posix_failed {
                            Some(ConnectError::Command {
                                kind,
                                exit_code,
                                stderr,
                            }) => cmd_err(
                                kind,
                                exit_code,
                                format!("{stderr} (tried as a Windows host too: {err})"),
                            ),
                            _ => err,
                        },
                    });
                }
            }
        };
        // The host answered and the sign-in worked. Whether it needs the user at login is a
        // fact of this sign-in: what it asked, nothing more (a host-key confirmation is not the
        // user's at login). Learned when it is true, un-learned when it stopped being true. With
        // a shared connection the sign-in was the step that opened it, and was learned from
        // there; an attempt that only rode a connection learns nothing (nobody was asked, and
        // that says nothing about the next sign-in).
        if control.is_none() {
            self.learn_login(&entry, &signin).await;
        }
        trail.reach(BootStage::Platform);
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
        let restart = locked(&self.restarts).remove(&host).is_some();
        let up_to_date = report
            .installed
            .as_ref()
            .is_some_and(|v| v.build == self.own.build);
        // What went to the host (an install's size and how long it took).
        let mut uploaded: Option<(u64, Duration)> = None;
        if up_to_date {
            let (action, line): (_, &[u8]) = if restart {
                (HelperAction::Restart, b"RESTART\n")
            } else {
                (HelperAction::Run, b"RUN\n")
            };
            trail.reach(BootStage::Ok);
            trail.action(action, None, &platform.target);
            if let Err(e) = io.write_all(line).await {
                return Err(self
                    .remote_gone(
                        BootStage::Ok,
                        &signin,
                        &child,
                        "the host closed the connection",
                        &e,
                    )
                    .await);
            }
        } else {
            trail.reach(BootStage::Helper);
            let file = self.artifact(&platform).map_err(at(BootStage::Helper))?;
            let local = |e: std::io::Error| Failure {
                stage: BootStage::Helper,
                error: ConnectError::Io(format!("{}: {e}", file.display())),
            };
            let mut f = tokio::fs::File::open(&file).await.map_err(local)?;
            let len = f.metadata().await.map_err(local)?.len();
            trail.reach(BootStage::Upload);
            trail.action(HelperAction::Upload, Some(len), &platform.target);
            let began = Instant::now();
            let mut sent = 0u64;
            let upload = async {
                io.write_all(format!("UPLOAD {len}\n").as_bytes())
                    .await
                    .map_err(SendError::Remote)?;
                send_file(&mut f, &mut io, &mut sent).await
            }
            .await;
            match upload {
                Ok(()) => uploaded = Some((len, began.elapsed())),
                Err(SendError::Local(e)) => return Err(local(e)),
                // The remote stopped reading: its exit status and stderr are the reason (a full
                // disk, a quota, a directory it may not write), not our broken pipe.
                Err(SendError::Remote(e)) => {
                    return Err(self
                        .remote_gone(
                            BootStage::Upload,
                            &signin,
                            &child,
                            &format!(
                                "the host stopped reading while the Marsad helper was being sent ({sent} of {len} bytes)"
                            ),
                            &e,
                        )
                        .await);
                }
            }
        }
        if let Err(e) = io.flush().await {
            let stage = if uploaded.is_some() {
                BootStage::Upload
            } else {
                BootStage::Ok
            };
            return Err(self
                .remote_gone(stage, &signin, &child, "the host closed the connection", &e)
                .await);
        }
        if let Some((len, took)) = uploaded {
            trail.uploaded(len, took);
        }
        trail.reach(BootStage::Ok);
        match read_line(&mut io).await {
            Ok(Some(l)) if l == "MARSAD-OK" => {}
            Ok(Some(l)) => {
                return Err(Failure {
                    stage: BootStage::Ok,
                    error: ConnectError::Protocol(format!(
                        "unexpected bootstrap answer {:?}",
                        truncate(&l, 200)
                    )),
                })
            }
            // The remote stopped before it answered (its install failed, or the connection
            // ended): what it said is the reason.
            Ok(None) | Err(_) => {
                return Err(Failure {
                    stage: BootStage::Ok,
                    error: self
                        .failed(&signin, &child, "installing marsad-agent failed")
                        .await,
                })
            }
        }
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
        if let Some(m) = &hello.machine {
            let twin = self
                .entries
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .find(|e| e.id != host && e.machine.as_deref() == Some(m.as_str()))
                .map(|e| (e.id.clone(), e.name.clone()));
            if let Some((twin_id, twin_name)) = twin {
                emit(MarsadEvent::BootTwin {
                    host: trail.host.clone(),
                    n: trail.n(),
                    twin: Ident::new(twin_id.as_str()),
                });
                if hello.host_id != host {
                    // Restarting it under this id would take it away from the other host,
                    // which would then restart it back.
                    return Err(Failure {
                        stage: BootStage::Check,
                        error: ConnectError::Protocol(format!(
                            "this is the same machine as the host {twin_name:?} (machine key {m}); \
                             remove one of the two hosts"
                        )),
                    });
                }
            }
            self.learn(Learned::Machine {
                host: host.clone(),
                machine: m.clone(),
            })
            .await;
        }
        let theirs = hello.host_id;
        // (Another build is restarted through the provisioner, which also fixes the id.)
        if theirs != host && hello.build == self.own.build {
            // A daemon started earlier under another id (e.g. by hand): the next attempt
            // restarts it as this host.
            emit(MarsadEvent::BootIdMismatch {
                host: trail.host.clone(),
                n: trail.n(),
                theirs: Ident::new(theirs.as_str()),
            });
            locked(&self.restarts).insert(host.clone(), SignInRepeat::IdRestart);
            // The user's sign-in is not over: the restart attempt is part of it. (A shared
            // connection carries the restart attempt without asking anything: nothing is armed,
            // so that one that has ended meanwhile cannot make this attempt ask unrequested.)
            if control.is_none() {
                self.prompts.arm(&host);
            }
            return Err(Failure {
                stage: BootStage::Check,
                error: ConnectError::Protocol(format!(
                    "the daemon on this host runs as {theirs:?}, not {host:?}; restarting it"
                )),
            });
        }
        // Connected: whatever click there was is spent, asked or not (a key may have done it).
        self.prompts.disarm(&host);
        Ok(client
            .with_attachment(child.clone())
            .with_diagnostics(child))
    }
}

impl BootstrapConnector {
    /// The environment of an ssh command that may ask the user: `SSH_ASKPASS` is this binary.
    fn askpass_env(
        &self,
        exe: &Path,
        host: &HostId,
        signin: &SignInAttempt,
    ) -> Vec<(String, String)> {
        vec![
            (
                "SSH_ASKPASS".into(),
                wasit_process::plain_path(exe).display().to_string(),
            ),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ("DISPLAY".into(), ":0".into()),
            (ASKPASS_HOST_ENV.into(), host.to_string()),
            (ASKPASS_ATTEMPT_ENV.into(), signin.tag().to_string()),
            (ASKPASS_SOCKET_ENV.into(), self.socket.display().to_string()),
            (ASKPASS_DIAG_ENV.into(), self.diag_dir.display().to_string()),
        ]
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
    /// 4. Otherwise the sign-in step: `ssh -M -N -f` with the askpass helper, where every prompt
    ///    of the attempt happens. It ends, with status 0, when ssh has authenticated and left
    ///    the connection open in the background: that exit is the signal (nothing is polled or
    ///    waited for). A failure is classified from its exit status and what ssh said, as the
    ///    one command of a host without a shared connection is. Its human time is the prompts'
    ///    (each bounded by the hub's prompt timeout) and its machine time ssh's own
    ///    `ConnectTimeout`; the attempt as a whole is bounded by the supervisor, as ever.
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
        if !armed && entry.interactive_auth {
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
            RemoteShell::Posix,
            SshRoute::SignIn { control: &control },
        )
        .ok_or_else(|| ConnectError::Spawn("the local host has no link".into()))
        .map_err(at(BootStage::Prepare))?;
        let mut cmd = CommandSpec::new(argv).traced(spec.diag_link(), &trail.attempt);
        cmd.env = self.askpass_env(exe, host, signin);
        let (_io, child) = CommandTransport::spawn(&cmd).map_err(at(BootStage::Spawn))?;
        let exit = child.wait_exit_unbounded().await;
        let code = exit.and_then(|e| e.code);
        if code != Some(0) {
            // With `-E` ssh's own messages go to the connection's log, not to its stderr.
            let log = self.masters.signin_failure_text(&control).await;
            let said = child.stderr_tail();
            let text = [said.as_str(), log.as_str()]
                .iter()
                .filter(|t| !t.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            let mut kind = classify_command_failure(code, &text);
            if signin.asked() > 0 && kind == CommandFailureKind::Other {
                // A cancelled or timed-out prompt makes ssh give up without a recognisable
                // message.
                kind = CommandFailureKind::AuthNeeded;
            }
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
        // The exit says ssh authenticated; ask the socket whether it is the connection that
        // stayed (ssh goes on without sharing when it cannot listen on the socket).
        if self.masters.check(entry, &control, Some(trail.n())).await != Check::Alive {
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

/// Handles a running daemon of another build (seen in its hello): the next attempt restarts it.
pub struct AgentProvisioner {
    pub restarts: Restarts,
    pub prompts: Prompts,
    pub entries: Entries,
}

#[async_trait]
impl Provisioner for AgentProvisioner {
    async fn ensure(
        &self,
        host: &HostSpec,
        problem: ProvisionReason,
    ) -> Result<(), ProvisionError> {
        if let ProvisionReason::BuildMismatch { expected, found } = &problem {
            emit(MarsadEvent::BootBuildMismatch {
                host: Ident::new(host.id.as_str()),
                expected: Ident::new(expected),
                found: Ident::new(found),
            });
        }
        locked(&self.restarts).insert(host.id.clone(), SignInRepeat::HelperRestart);
        // The user just signed in for this link; the restart attempt is part of that. Over a
        // shared connection it rides that connection and asks nothing, so nothing is armed
        // (a connection that has ended meanwhile must not make it ask unrequested).
        let shared = self
            .entries
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&host.id)
            .is_some_and(Masters::applies);
        if !shared {
            self.prompts.arm(&host.id);
        }
        Ok(())
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
        let read = read_boot_line(&mut r, &seen).await;
        (read, seen.take())
    }

    #[tokio::test]
    async fn bootstrap_report_after_a_banner_and_foreign_output() {
        let seen = Seen::default();
        let mut ok: &[u8] = b"Welcome!\nlast login: never\nMARSAD-BOOT Linux x86_64 none\nrest";
        let BootRead::Report(line) = read_boot_line(&mut ok, &seen).await else {
            panic!("the report was there");
        };
        assert_eq!(line, "MARSAD-BOOT Linux x86_64 none");
        // The banner is counted, line by line and byte by byte, and nothing past the report
        // was read.
        let preamble = seen.take();
        assert_eq!((preamble.lines, preamble.bytes), (2, 27));
        assert_eq!(preamble.text(), "Welcome!\nlast login: never\n");
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
        assert_eq!(
            classify_command_failure(Some(1), &text),
            CommandFailureKind::UnknownDistro
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
            let mut wait = Box::pin(read_boot_line(&mut hub_end, &trail.seen));
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

    #[test]
    fn the_report_line_is_parsed_into_words() {
        let r = BootReport::parse(
            r#"MARSAD-BOOT Linux x86_64 {"build":"0.2.0+abc","target":"x86_64-unknown-linux-musl"}"#,
        );
        assert_eq!((r.os.as_str(), r.arch.as_str()), ("Linux", "x86_64"));
        assert_eq!(r.installed.unwrap().build, "0.2.0+abc");
        let none = BootReport::parse("MARSAD-BOOT Darwin arm64 none");
        assert!(none.installed.is_none());
        assert_eq!(none.arch, "arm64");
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
            restarts: Restarts::default(),
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
        let spec = HostSpec::new(id.clone(), ["true"]);
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
}
