//! Marsad's diagnostics events (design §12.6): what the daemon, the bridge, `stop` and the
//! askpass helper record about themselves, and what the hub records of its hosts (the
//! connection trail `boot.*` of every connect attempt, sign-in prompts `auth.*`, the registry
//! `hosts.*`, `hostenv.apply`, the session feed `fleet.feed`), declared once for every process
//! of the product.
//!
//! Every field type is one of `wasit-diag`'s value types: ids, sizes, timings, states, reasons
//! and error codes; never a prompt, a response, a tool's input or output, a file's contents,
//! a secret, a session or workstream name, or a project path. Marsad's own paths are
//! [`HomePath`]s (written `~`-relative).
//!
//! Records are written by one sink per process (`wasit_diag::install`), to the files of
//! [`crate::layout::Layout::diag`]: the daemon's `daemon.ndjson`, and the shared `aux.ndjson`
//! that short-lived processes append to ([`crate::layout::DIAG_AUX`]).

use wasit_control::TraceId;
use wasit_diag::{Bytes, Code, DiagText, ErrorInfo, HomePath, Ident, Ms, ProjectPath, TimeMs};

use crate::hosts::{HostKind, PromptKind};

wasit_diag::diag_enum! {
    /// The step of `serve`'s startup that failed (the process then exits).
    pub enum StartStage {
        /// Creating this machine's data directory.
        DataDir,
        /// Resolving the runtime configuration (a driver override, the shim launcher).
        Config,
        /// Taking the single-instance lock and binding the endpoint.
        Listen,
        /// Opening the session manager.
        Sessions,
        /// Opening the Wasl node.
        Node,
        /// Locating the daemon's own executable.
        Executable,
        /// Registering the control services.
        Services,
        /// Starting the hub services (`serve --hub`).
        Hub,
        /// Serializing the readiness line.
        Readiness,
    }
}

wasit_diag::diag_enum! {
    /// Why the readiness line was not delivered to whoever started the daemon.
    pub enum ReadinessFailure {
        /// The reader had closed the pipe (it gave up waiting, or is not interested).
        PipeClosed,
        /// Writing it failed for another reason.
        WriteFailed,
    }
}

wasit_diag::diag_enum! {
    /// Why a daemon stopped serving.
    pub enum ShutdownWhy {
        /// SIGTERM / SIGINT / Ctrl-C (the signal is recorded next to it).
        Signal,
        /// A client asked it to (`daemon.shutdown`, i.e. `marsad-agent stop`).
        Request,
        /// The accept loop ended without an error.
        ServeEnded,
        /// The accept loop failed.
        ServeFailed,
    }
}

wasit_diag::diag_enum! {
    /// How `bridge` came by its daemon.
    pub enum BridgeOutcome {
        /// A daemon was already serving the endpoint (emitted when the bridge ends: the
        /// connect that found it is inside `wasit-control`'s bridge, which reports nothing
        /// earlier).
        Running,
        /// None was serving; the bridge started one and it became ready.
        Started,
        /// None was serving and starting one failed (see `reason`, `exit`, `error`).
        StartFailed,
        /// The endpoint could not be connected to (or not after a start): the hub gets the
        /// `bridge_unavailable` goodbye.
        Unavailable,
    }
}

wasit_diag::diag_enum! {
    /// Why starting a daemon failed.
    pub enum StartFailure {
        /// The daemon's stderr file could not be opened.
        LogOpenFailed,
        /// The daemon's own executable could not be located.
        NoExecutable,
        /// The daemon process could not be started.
        SpawnFailed,
        /// The daemon process had no stdout to read its readiness line from.
        NoStdout,
        /// Reading the readiness line failed.
        ReadFailed,
        /// The daemon exited before it was ready (`exit` is its status when it has one).
        ExitedBeforeReady,
        /// The daemon closed its readiness pipe but did not exit within the wait.
        PipeClosedStillRunning,
        /// The bridge's bound on starting the daemon ran out.
        TimedOut,
    }
}

wasit_diag::diag_enum! {
    /// How a bridge ended after it had a daemon.
    pub enum BridgeEndReason {
        /// Both directions finished (the hub closed the link, or the daemon did).
        Ended,
        /// Copying between stdio and the daemon failed.
        IoError,
        /// The bridge never had a daemon to copy to (see `bridge.daemon`).
        NoDaemon,
    }
}

wasit_diag::diag_enum! {
    /// How `marsad-agent stop` ended.
    pub enum StopOutcome {
        /// The daemon exited and its lock is free.
        Stopped,
        /// Nothing was serving the endpoint.
        NotRunning,
        /// The daemon could not be reached for another reason.
        Unreachable,
        /// The daemon did not close its connection after the request.
        NotClosed,
        /// The daemon closed its connection but its lock stayed held.
        NotExited,
        /// The lock could not be tested.
        LockError,
    }
}

wasit_diag::diag_enum! {
    /// How the `SSH_ASKPASS` helper ended.
    pub enum AskpassOutcome {
        /// The hub returned an answer (never recorded).
        Answered,
        /// The hub answered with an error (cancelled, timed out, superseded, unknown host).
        Refused,
        /// The hub could not be reached.
        Unreachable,
        /// Started without the environment a hub link sets.
        NotAHubLink,
    }
}

wasit_diag::diag_enum! {
    /// The remote shell a connect attempt's bootstrap runs in.
    pub enum BootShell {
        /// POSIX `sh` (Unix hosts, WSL, and the first try of a host never connected).
        Posix,
        /// PowerShell and the compiled boot program (native Windows OpenSSH).
        Powershell,
    }
}

wasit_diag::diag_enum! {
    /// Why a host first tried as POSIX is asked again as native Windows.
    pub enum WindowsRetryWhy {
        /// The `sh` that answered is a Windows port (MINGW, MSYS, Cygwin): its report names it.
        MingwSh,
        /// The remote command ran but printed no bootstrap report (cmd.exe has no `sh`).
        NoReport,
    }
}

wasit_diag::diag_enum! {
    /// What the hub told the host's bootstrap to do after its report.
    pub enum HelperAction {
        /// The installed helper is this build: run it.
        Run,
        /// The installed helper is this build: stop its running daemon, then run it.
        Restart,
        /// Install this build first (the host has none, or another build).
        Upload,
    }
}

wasit_diag::diag_enum! {
    /// The step of a connect attempt that failed (`boot.failed`).
    pub enum BootStage {
        /// Before anything was started: the host is unknown, an interactive host was not asked
        /// to connect, the hub cannot locate itself.
        Prepare,
        /// The transport command (`ssh`, `wsl.exe`) could not be started.
        Spawn,
        /// The host never printed the bootstrap's report (sign-in, network, WSL, foreign
        /// output instead).
        Report,
        /// The report names a platform Marsad has no helper for.
        Platform,
        /// This installation has no usable helper file for the platform.
        Helper,
        /// Sending the command and, for an install, the helper's bytes: the remote stopped
        /// reading (its exit and stderr say why).
        Upload,
        /// Waiting for `MARSAD-OK` (the remote's install or `stop` failed, or it closed).
        Ok,
        /// The helper started by the bootstrap did not complete the control handshake.
        Hello,
        /// The handshake worked but the host is the same machine as another host, or runs
        /// under another id.
        Check,
    }
}

wasit_diag::diag_enum! {
    /// What happened to a host entry.
    pub enum HostChange {
        Added,
        Updated,
        Removed,
    }
}

wasit_diag::diag_enum! {
    /// The kind of a host entry.
    pub enum HostWord {
        Local,
        Ssh,
        Wsl,
    }
}

wasit_diag::diag_enum! {
    /// The shape of an ssh target (never the target itself).
    pub enum TargetShape {
        /// `~/.ssh/config` alias (a bare word).
        Alias,
        /// `user@host`.
        UserAtHost,
        /// A host name or address without a user.
        Host,
    }
}

/// What a host entry's kind shows in a record: its kind and the ssh target's shape, whether a
/// port and an identity file are set, and the number of extra options; never the target, the
/// distro, the key's path or an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostShape {
    pub kind: HostWord,
    pub target: Option<TargetShape>,
    pub port_set: bool,
    pub identity_set: bool,
    pub options: u32,
}

impl HostShape {
    pub fn of(kind: &HostKind) -> HostShape {
        match kind {
            HostKind::Local => HostShape {
                kind: HostWord::Local,
                target: None,
                port_set: false,
                identity_set: false,
                options: 0,
            },
            HostKind::Wsl { .. } => HostShape {
                kind: HostWord::Wsl,
                target: None,
                port_set: false,
                identity_set: false,
                options: 0,
            },
            HostKind::Ssh {
                target,
                port,
                identity_file,
                options,
            } => HostShape {
                kind: HostWord::Ssh,
                target: Some(if target.contains('@') {
                    TargetShape::UserAtHost
                } else if target.contains('.') || target.contains(':') {
                    TargetShape::Host
                } else {
                    TargetShape::Alias
                }),
                port_set: port.is_some(),
                identity_set: identity_file.is_some(),
                options: u32::try_from(options.len()).unwrap_or(u32::MAX),
            },
        }
    }
}

wasit_diag::diag_enum! {
    /// What a connect attempt taught the registry about a host.
    pub enum LearnedWord {
        Platform,
        Interactive,
        Machine,
    }
}

wasit_diag::diag_enum! {
    /// What the registry failed to do for a host.
    pub enum HostsOp {
        /// Starting the supervised link of a stored host.
        StartLink,
        /// Recording what a connect attempt learned.
        Learn,
        /// Handing a learned fact to the registry (it is gone).
        RegistryGone,
    }
}

wasit_diag::diag_enum! {
    /// The kind of a sign-in prompt (never its text).
    pub enum PromptWord {
        Secret,
        Code,
        Confirm,
        Info,
    }
}

impl From<PromptKind> for PromptWord {
    fn from(k: PromptKind) -> Self {
        match k {
            PromptKind::Secret => PromptWord::Secret,
            PromptKind::Code => PromptWord::Code,
            PromptKind::Confirm => PromptWord::Confirm,
            PromptKind::Info => PromptWord::Info,
        }
    }
}

wasit_diag::diag_enum! {
    /// How a sign-in prompt ended without an answer.
    pub enum CancelWhy {
        /// The user cancelled it.
        User,
        /// Nobody answered within the prompt timeout.
        Timeout,
        /// A newer prompt of the same host replaced it.
        Superseded,
        /// The askpass helper that asked went away (ssh gave up, or was killed).
        HelperGone,
    }
}

wasit_diag::diag_enum! {
    /// How applying a host's environment settings ended.
    pub enum ApplyOutcome {
        /// The host took the stored settings.
        Applied,
        /// The host already ran the stored revision.
        Noop,
        /// It could not be done (see `error`).
        Failed,
        /// The host's link is down; the settings apply when it connects.
        NotConnected,
    }
}

wasit_diag::diag_enum! {
    /// A host's session feed (the `host.sessions` subscription behind the fleet).
    pub enum FeedEvent {
        /// The feed began following the host.
        Started,
        /// The feed stopped (`error` says why when the service failed it): the fleet no
        /// longer follows this host's sessions.
        Ended,
        /// The subscriber fell behind; it resubscribes from its last cursor.
        Lagged,
        /// An event of the host could not be decoded and was dropped.
        Undecodable,
    }
}

wasit_diag::diag_events! {
    /// Events of the `marsad-agent` processes (`src` `hub`, `daemon`, `bridge`, `stop`,
    /// `askpass`).
    pub enum MarsadEvent {
        /// `serve` holds the single-instance lock and its diagnostics moved to `daemon.ndjson`.
        /// `free_text_logging`: `MARSAD_LOG` is set, so plain `tracing` text also goes to
        /// stderr.
        #[diag(ev = "daemon.start")]
        DaemonStart {
            hub: bool,
            host_id: Ident,
            build: Ident,
            pid: u32,
            machine: Ident,
            data: HomePath,
            run_dir: HomePath,
            socket: HomePath,
            free_text_logging: bool,
        },
        /// Startup is complete and the daemon accepts connections. The phases are measured in
        /// `serve`: environment capture, session adoption, opening the Wasl node, starting the
        /// hub (`None` on a host daemon). `line_written`: whoever started the daemon was
        /// handed the readiness line (when not, `daemon.readiness_unwritten` says why and the
        /// daemon serves on).
        #[diag(ev = "daemon.ready")]
        DaemonReady {
            total_ms: Ms,
            env_ms: Ms,
            adopt_ms: Ms,
            node_ms: Ms,
            hub_ms: Option<Ms>,
            line_written: bool,
        },
        /// The readiness line could not be written; the daemon is healthy and serves.
        #[diag(ev = "daemon.readiness_unwritten", lvl = warn)]
        DaemonReadinessUnwritten { why: ReadinessFailure, error: ErrorInfo },
        /// Another daemon holds this machine's lock; this one exits with status 3. Written to
        /// the shared file: a daemon that lost never writes `daemon.ndjson`.
        #[diag(ev = "daemon.lock_held", lvl = warn)]
        DaemonLockHeld { lock: HomePath },
        /// A startup step failed; the process exits with `exit`.
        #[diag(ev = "daemon.failed", lvl = error)]
        DaemonFailed {
            stage: StartStage,
            exit: i32,
            error: Option<ErrorInfo>,
        },
        /// The daemon stopped serving (sessions keep running in their shims).
        #[diag(ev = "daemon.shutdown")]
        DaemonShutdown {
            why: ShutdownWhy,
            signal: Option<Code>,
            error: Option<ErrorInfo>,
        },
        /// `marsad-agent bridge` started (the remote end of a hub link).
        #[diag(ev = "bridge.start")]
        BridgeStart {
            host_id: Ident,
            pid: u32,
            data: HomePath,
        },
        /// How the bridge came by its daemon. `ms`: how long a start took (`started`,
        /// `start_failed`) or how long the bridge tried before giving up (`unavailable`).
        #[diag(ev = "bridge.daemon")]
        BridgeDaemon {
            outcome: BridgeOutcome,
            ms: Option<Ms>,
            reason: Option<StartFailure>,
            exit: Option<i32>,
            error: Option<ErrorInfo>,
        },
        /// The bridge process is ending.
        #[diag(ev = "bridge.end")]
        BridgeEnd {
            reason: BridgeEndReason,
            error: Option<ErrorInfo>,
        },
        /// `marsad-agent stop` ended. `legacy`: a daemon of a build before the per-machine
        /// layout was stopped too.
        #[diag(ev = "stop.end")]
        StopEnd {
            outcome: StopOutcome,
            ms: Ms,
            legacy: bool,
            error: Option<ErrorInfo>,
        },
        /// The `SSH_ASKPASS` helper ended. Neither the prompt nor the answer is recorded.
        #[diag(ev = "askpass.end")]
        AskpassEnd {
            host: Option<Ident>,
            outcome: AskpassOutcome,
            ms: Ms,
            error: Option<ErrorInfo>,
        },
        /// The hub's `sessions.launch` ended, success or failure. `trace`: the request's trace
        /// id, when it had one. `session`: the session launched or resumed (on a failure, the id
        /// the request named, when it named one). `resume`: `fresh`, `continue` or `resume`.
        /// `first_prompt`: the request carried a first prompt (its text is never recorded).
        /// `cwd`: the project directory the request named (hashed). `outcome`: `ok` or `error`;
        /// `error` on a failure is the RPC error's code and a fingerprint of its text, never the
        /// text itself (the message can quote the request).
        #[diag(ev = "launch")]
        Launch {
            trace: Option<TraceId>,
            host: Ident,
            session: Option<Ident>,
            driver: Ident,
            resume: Code,
            first_prompt: bool,
            cwd: Option<ProjectPath>,
            ms: Ms,
            outcome: Code,
            error: Option<ErrorInfo>,
        },
        // ── the connection trail (design §5.3, S14): one connect attempt of a host, joined to
        // `link.attempt_begin` / `link.attempt_end` of `hosts:<host>` on `(host, n)` ──

        /// The hub's connector began attempt `n` of `host`. `first_contact`: the host has
        /// never connected (its platform is unknown). `interactive`: it has needed a sign-in
        /// answer. `armed`: the user asked for this attempt (an interactive host connects only
        /// then).
        #[diag(ev = "boot.attempt", budget = 300)]
        BootAttempt {
            host: Ident,
            n: u64,
            shell: BootShell,
            first_contact: bool,
            interactive: bool,
            armed: bool,
        },
        /// What the host printed before the bootstrap's report (a banner, rc-file output),
        /// recorded on every attempt: lines (a last line without its newline counts), bytes,
        /// and for a non-empty preamble its first 120 characters, scrubbed. Also when the
        /// report never came: then it is the only evidence there is.
        #[diag(ev = "boot.preamble", budget = 300)]
        BootPreamble {
            host: Ident,
            n: u64,
            lines: u32,
            bytes: Bytes,
            shape: Option<DiagText>,
        },
        /// The bootstrap's report: the host's OS and architecture words and the installed
        /// helper's build (`None`: none installed). `ms` since this attempt began.
        #[diag(ev = "boot.report", budget = 300)]
        BootReport {
            host: Ident,
            n: u64,
            os: Code,
            arch: Code,
            installed: Option<Ident>,
            ms: Ms,
        },
        /// The host is asked again as native Windows (PowerShell).
        #[diag(ev = "boot.windows_retry", budget = 300)]
        BootWindowsRetry {
            host: Ident,
            n: u64,
            why: WindowsRetryWhy,
        },
        /// What the hub told the bootstrap to do. `bytes`: the helper's size (an upload).
        /// `target`: the host's target triple.
        #[diag(ev = "boot.action", budget = 300)]
        BootActionChosen {
            host: Ident,
            n: u64,
            action: HelperAction,
            bytes: Option<Bytes>,
            target: Ident,
        },
        /// The helper's bytes were all sent (and flushed) to the host; `ms` is how long that
        /// took. The host's check that it received them all follows (`boot.ok` or a failure).
        #[diag(ev = "boot.uploaded", budget = 300)]
        BootUploaded {
            host: Ident,
            n: u64,
            bytes: Bytes,
            ms: Ms,
        },
        /// The bootstrap answered `MARSAD-OK`: any install worked and it started the helper.
        /// `ms` since this attempt began.
        #[diag(ev = "boot.ok", budget = 300)]
        BootOk { host: Ident, n: u64, ms: Ms },
        /// The helper answered the control handshake. `host_id_matches`: its daemon knows
        /// itself by this host's id. `ms` since this attempt began.
        #[diag(ev = "boot.hello", budget = 300)]
        BootHello {
            host: Ident,
            n: u64,
            build: Ident,
            host_id_matches: bool,
            machine: Option<Ident>,
            ms: Ms,
        },
        /// The attempt failed at `stage`: the one record of why, besides the link's own. For a
        /// failure of the transport command: `kind` (the classifier's word), the child's exit
        /// status when it had ended, and `stderr`, the reason the user is shown (the child's
        /// stderr tail, or what the hub made of what the host printed), scrubbed and bounded.
        /// For another failure: `error` (its code, and its text where the text is ours).
        #[diag(ev = "boot.failed", lvl = warn, budget = 300)]
        BootFailed {
            host: Ident,
            n: u64,
            stage: BootStage,
            kind: Option<Code>,
            exit: Option<i32>,
            stderr: Option<DiagText>,
            error: Option<ErrorInfo>,
        },
        /// Another host entry is the same machine (same machine key); this attempt fails.
        #[diag(ev = "boot.twin", lvl = warn, budget = 120)]
        BootTwin { host: Ident, n: u64, twin: Ident },
        /// The host's daemon runs under another id than this host's; the next attempt
        /// restarts it under this one.
        #[diag(ev = "boot.id_mismatch", budget = 120)]
        BootIdMismatch {
            host: Ident,
            n: u64,
            theirs: Ident,
        },
        /// The host's daemon is another build than the hub's; the next attempt restarts it.
        #[diag(ev = "boot.build_mismatch", budget = 120)]
        BootBuildMismatch {
            host: Ident,
            expected: Ident,
            found: Ident,
        },
        // ── sign-in prompts (never the prompt's text, never the answer) ──

        /// A sign-in prompt was raised for `host`; `attempt` counts how often this same
        /// prompt came in the current connect attempt (2: the previous answer was wrong).
        #[diag(ev = "auth.prompt", budget = 300)]
        AuthPrompt {
            host: Ident,
            prompt: u64,
            kind: PromptWord,
            attempt: u32,
        },
        /// The prompt was answered, `ms` after it was raised.
        #[diag(ev = "auth.answered", budget = 300)]
        AuthAnswered {
            host: Ident,
            prompt: u64,
            ms: Ms,
        },
        /// The prompt ended without an answer.
        #[diag(ev = "auth.cancelled", budget = 300)]
        AuthCancelled {
            host: Ident,
            prompt: u64,
            why: CancelWhy,
        },
        // ── the hosts registry ──

        /// A host entry was added, updated or removed (the ssh target only by its shape).
        #[diag(ev = "hosts.changed", budget = 300)]
        HostsChanged {
            host: Ident,
            op: HostChange,
            kind: HostWord,
            target_kind: Option<TargetShape>,
            port_set: bool,
            identity_set: bool,
            options: u32,
        },
        /// A connect attempt taught the registry something about a host (and it was stored).
        #[diag(ev = "hosts.learned", budget = 300)]
        HostsLearned { host: Ident, what: LearnedWord },
        /// The registry could not do something for a host (`host` is absent when it is not
        /// known).
        #[diag(ev = "hosts.failed", lvl = warn, budget = 300)]
        HostsFailed {
            host: Option<Ident>,
            op: HostsOp,
            error: ErrorInfo,
        },
        /// The hub applied (or tried to apply) a host's stored environment settings.
        /// `revision`: the stored one. `ms`: the whole apply, including the host's report.
        /// `setup_ok`: how the host's setup commands went, when it ran any.
        #[diag(ev = "hostenv.apply", budget = 300)]
        HostenvApply {
            host: Ident,
            revision: Option<Ident>,
            outcome: ApplyOutcome,
            ms: Ms,
            setup_ok: Option<bool>,
            error: Option<ErrorInfo>,
        },
        /// A host's session feed (what the fleet shows of its sessions).
        #[diag(ev = "fleet.feed", budget = 300)]
        FleetFeed {
            host: Ident,
            event: FeedEvent,
            error: Option<ErrorInfo>,
        },
        // ── the diagnostics export (design §6.5-§6.7; the exporter is a task of the hub) ──

        /// An export began. `since_ms`: the start of its window. `hosts`: the fleet's hosts it
        /// will visit. `mode`: `hub`, or `local_files` (the CLI found no hub serving).
        #[diag(ev = "export.start", budget = 30)]
        ExportStart {
            id: Ident,
            since_ms: TimeMs,
            hosts: u32,
            include_trails: bool,
            mode: Code,
        },
        /// One source of an export ended. `source`: `app`, `host` or `local`; `host`: the host's
        /// id. `outcome`: `done`, `partial`, `not_connected` or `failed`. `bytes`: raw bytes of
        /// records it contributed; `sections`: how many; `skipped`: sections left out (the size
        /// cap, the host's budget or time). `error`: what the host said when a read of it failed
        /// (the first time).
        #[diag(ev = "export.source", budget = 300)]
        ExportSource {
            id: Ident,
            source: Code,
            host: Option<Ident>,
            outcome: Code,
            bytes: Bytes,
            sections: u32,
            skipped: u32,
            ms: Ms,
            error: Option<ErrorInfo>,
        },
        /// An export ended: `complete` when nothing planned is missing; `bytes` is the raw size
        /// of the records in the file; `skipped` the sections left out; `error` is why no file
        /// was produced.
        #[diag(ev = "export.done", budget = 30)]
        ExportDone {
            id: Ident,
            bytes: Bytes,
            complete: bool,
            skipped: u32,
            ms: Ms,
            error: Option<ErrorInfo>,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(target: &str) -> HostKind {
        HostKind::Ssh {
            target: target.into(),
            port: Some(2222),
            identity_file: Some("/home/u/.ssh/id_work".into()),
            options: vec!["ProxyJump=bastion".into(), "Compression=yes".into()],
        }
    }

    #[test]
    fn a_host_shows_its_shape_never_its_target_port_key_or_options() {
        let s = HostShape::of(&ssh("alice@gpu01.lab.example"));
        assert_eq!(s.kind, HostWord::Ssh);
        assert_eq!(s.target, Some(TargetShape::UserAtHost));
        assert_eq!((s.port_set, s.identity_set, s.options), (true, true, 2));
        assert_eq!(
            HostShape::of(&ssh("gpu01.lab.example")).target,
            Some(TargetShape::Host)
        );
        assert_eq!(
            HostShape::of(&ssh("10.0.0.7")).target,
            Some(TargetShape::Host)
        );
        assert_eq!(
            HostShape::of(&ssh("fe80::1")).target,
            Some(TargetShape::Host)
        );
        assert_eq!(HostShape::of(&ssh("gpu")).target, Some(TargetShape::Alias));
        let wsl = HostShape::of(&HostKind::Wsl {
            distro: "Ubuntu-22.04".into(),
        });
        assert_eq!(
            (
                wsl.kind,
                wsl.target,
                wsl.port_set,
                wsl.identity_set,
                wsl.options
            ),
            (HostWord::Wsl, None, false, false, 0)
        );
        assert_eq!(HostShape::of(&HostKind::Local).kind, HostWord::Local);
    }

    #[test]
    fn every_prompt_kind_has_its_word() {
        for (k, w) in [
            (PromptKind::Secret, PromptWord::Secret),
            (PromptKind::Code, PromptWord::Code),
            (PromptKind::Confirm, PromptWord::Confirm),
            (PromptKind::Info, PromptWord::Info),
        ] {
            assert_eq!(PromptWord::from(k), w);
        }
    }
}
