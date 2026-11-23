//! Marsad's diagnostics events (design §12.6): what the daemon, the bridge, `stop` and the
//! askpass helper record about themselves, declared once for every process of the product.
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
use wasit_diag::{Code, ErrorInfo, HomePath, Ident, Ms, ProjectPath};

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
    }
}
