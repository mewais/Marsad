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

// ── the desktop app (design §12.7) ───────────────────────────────────────────

/// Declares the desktop app's events once, as the two types they need:
///
/// * [`UiEvent`]: what the **webview** may say (`diag_log`), declared with
///   `wasit_diag::diag_data!`, so it has serde (`Deserialize`: the shell reads the webview's
///   JSON into it) and TypeScript (the webview's type) from the one declaration, and every
///   field must be a diagnostics value type (a `String`, a path or a JSON value is a compile
///   error). Unknown variants are refused by serde; [`UiEvent::from_wire`], the one way the
///   shell reads one, also refuses unknown fields (`diag_data!` cannot carry
///   `deny_unknown_fields`: serde wants that attribute after the derive it belongs to).
/// * [`AppEvent`]: the app's log record (`src` `app`, file `app.ndjson`), declared with
///   `wasit_diag::diag_events!` (flat records with a level and a rate budget): the `ui` variants
///   below with the same names and fields, and the `shell` variants, which only the shell's own
///   code can emit (a webview cannot name them: they are not in [`UiEvent`]).
///
/// `From<UiEvent> for AppEvent` is generated with them, so the two cannot drift. Optional fields
/// (after the `;`) are `Option<T>` in both.
///
/// A webview-facing field must not be a `DiagText` or an `ErrorInfo`: reading those back from a
/// wire keeps their text as it arrived (they were scrubbed where they were made), so a webview
/// could put free text in them. The `ui` variants therefore hold ids, codes, numbers and flags
/// only; a test (`a_free_text_string_in_any_field_never_survives`) sends a marked string in
/// every field of every variant and fails if one comes out.
macro_rules! app_events {
    (
        shell { $($shell:tt)* }
        ui {
            $(
                $(#[doc = $doc:literal])*
                #[diag(ev = $ev:literal $(, lvl = $lvl:ident)? $(, budget = $budget:expr)?)]
                $V:ident { $( $f:ident : $t:ty ),* $(; $( $of:ident : $ot:ty ),* )? $(,)? }
            ),* $(,)?
        }
    ) => {
        wasit_diag::diag_data! {
            #[diag(tag = "ev")]
            /// What the desktop app's webview may record in the app log (`diag_log`): the user
            /// interface's own events, made only of ids, codes, sizes, timings and flags.
            #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
            pub enum UiEvent {
                $(
                    $(#[doc = $doc])*
                    #[diag(ev = $ev)]
                    $V { $( $f : $t ),* $(; $( $of : $ot ),* )? }
                ),*
            }
        }

        wasit_diag::diag_events! {
            /// Events of the desktop app (`src` `app`): the shell's own and, converted from
            /// [`UiEvent`], the webview's.
            pub enum AppEvent {
                $($shell)*
                $(
                    $(#[doc = $doc])*
                    #[diag(ev = $ev $(, lvl = $lvl)? $(, budget = $budget)?)]
                    $V { $( $f : $t, )* $($( $of : Option<$ot>, )*)? }
                ),*
            }
        }

        impl From<UiEvent> for AppEvent {
            fn from(e: UiEvent) -> AppEvent {
                match e {
                    $( UiEvent::$V { $($f,)* $($($of,)*)? } => AppEvent::$V { $($f,)* $($($of,)*)? } ),*
                }
            }
        }
    };
}

app_events! {
    shell {
        /// The app started. `version`: the app's version; `os`: the operating system
        /// (`linux`, `windows`, `macos`).
        #[diag(ev = "app.start")]
        Start { version: Ident, os: Code },
        /// The app is exiting (the daemon and its sessions keep running).
        #[diag(ev = "app.exit")]
        Exit {},
        /// The app's link to its local daemon (the hub) changed state. `state`: the link's
        /// state word (`connecting`, `provisioning`, `connected`, `reconnecting`,
        /// `auth_needed`, `incompatible`, `failed`, `stopped`); `attempt`: which reconnect
        /// attempt (when it is one); `error`: why it is not up (a code, the text's length and
        /// fingerprint: the text is the daemon link's, not ours to keep).
        #[diag(ev = "app.hub_link")]
        HubLink { state: Code, attempt: Option<u32>, error: Option<ErrorInfo> },
        /// The shell's watcher of the fleet (OS notifications, the tray's count, the connection
        /// state the webview shows) stopped for good: from now on none of them is updated.
        /// `reason`: `fleet_failed` (the subscription ended with an error), `fleet_closed` (it
        /// ended without one) or `link_closed` (the link supervisor is gone).
        #[diag(ev = "app.watcher_ended", lvl = error)]
        WatcherEnded { reason: Code, error: Option<ErrorInfo> },
        /// An operation of the shell failed and it carried on. `op`: `tray_create`,
        /// `tray_update`, `bring_forward`, `navigate`, `connection_event`, `notification`,
        /// `notification_prefs`, `notification_log`; `error`: a code, length and fingerprint
        /// of the OS's or the framework's text.
        #[diag(ev = "app.failed", lvl = warn, budget = 120)]
        Failed { op: Code, error: ErrorInfo },
        /// A notification was decided on. `kind`: `needs_you`, `failed`, `finished`,
        /// `host_lost`; `outcome`: `shown`, or what held it back (`prefs`, `dnd`, `focus`).
        /// Never its title or body (they name sessions).
        #[diag(ev = "app.notification")]
        Notification { kind: Code, outcome: Code },
        /// The window background was set: `translucent` as asked, `transparent` what the window
        /// can show now, the native `effect` that took (`acrylic`, `mica`, `blur`, `vibrancy`),
        /// none when the OS has none.
        #[diag(ev = "app.surface")]
        Surface { translucent: bool, transparent: bool, effect: Option<Code> },
    }
    ui {
        /// `ui.prompt`: the app accepted a prompt (leg 1 of its trail). `kind`: `prompt` or
        /// `slash`; `chars`: the prompt's length, never its text.
        #[diag(ev = "ui.prompt")]
        Prompt {
            trace: TraceId,
            host: Ident,
            session: Ident,
            kind: Code,
            chars: u32,
            attachments: u32,
            bytes: wasit_diag::Bytes,
        },
        /// `ui.upload`: attachments were uploaded for a prompt or a launch. `outcome`: `ok` or
        /// `error`; `code`: the error's code.
        #[diag(ev = "ui.upload")]
        Upload {
            trace: TraceId,
            files: u32,
            bytes: wasit_diag::Bytes,
            ms: Ms,
            outcome: Code
            ; code: Code
        },
        /// `ui.command`: a command other than a prompt. `command`: `answer_gate`,
        /// `answer_question`, `interrupt`, `set_model`, `set_mode`, `stop` or `continue`.
        #[diag(ev = "ui.command")]
        Command { trace: TraceId, host: Ident, session: Ident, command: Code },
        /// `ui.launch`: a session was launched or resumed from the app. `resume`: `fresh`,
        /// `continue` or `resume`; `first_prompt`: it carried one (never its text).
        #[diag(ev = "ui.launch")]
        Launch {
            trace: TraceId,
            host: Ident,
            driver: Ident,
            resume: Code,
            first_prompt: bool,
            attachments: u32,
        },
        /// `ui.rpc`: a traced request left the app and was answered (leg 3). `outcome`: `ok`
        /// or `error`; `code`: the error's code.
        #[diag(ev = "ui.rpc")]
        Rpc { trace: TraceId, method: Code, ms: Ms, outcome: Code ; code: Code },
        /// `ui.render_error`: a component failed to render (the error boundary caught it).
        /// Only the component's name is recorded, never the error's message.
        #[diag(ev = "ui.render_error", lvl = error, budget = 60)]
        RenderError { component: Code },
        /// `ui.developer_mode`: the developer-mode switch was turned `on` or off.
        #[diag(ev = "ui.developer_mode")]
        DeveloperMode { on: bool },
        /// `ui.export`: a step of exporting diagnostics. `stage`: `start`, `saved`,
        /// `save_cancelled` or `failed`; `code`: the failure's code.
        #[diag(ev = "ui.export")]
        Export { stage: Code ; code: Code },
    }
}

/// Why [`UiEvent::from_wire`] refused a payload. Only a word: what the webview sent is never
/// kept, quoted or logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRefusal {
    /// Not a JSON object.
    NotAnObject,
    /// No `ev`, or one that names no [`UiEvent`] (including the shell's own `app.*` events).
    UnknownEvent,
    /// A field the event does not declare.
    UnknownField,
    /// A field of the wrong type, a missing one, or a trace id that is not one.
    Invalid,
}

impl UiRefusal {
    /// The refusal as a [`Code`] word.
    pub fn code(self) -> &'static str {
        match self {
            UiRefusal::NotAnObject => "not_an_object",
            UiRefusal::UnknownEvent => "unknown_event",
            UiRefusal::UnknownField => "unknown_field",
            UiRefusal::Invalid => "invalid",
        }
    }
}

impl std::fmt::Display for UiRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UiRefusal::NotAnObject => "a diagnostics event is a JSON object",
            UiRefusal::UnknownEvent => "not a diagnostics event the webview may record",
            UiRefusal::UnknownField => "a field the event does not declare",
            UiRefusal::Invalid => "a field is missing or of the wrong type",
        })
    }
}

impl std::error::Error for UiRefusal {}

impl UiEvent {
    /// Reads one event from the JSON the webview sent: an object whose `ev` names a [`UiEvent`]
    /// and whose other keys are all fields that event declares, each valid for its type (ids
    /// and codes that fail validation are kept as their length only, see `wasit_diag::Ident`).
    pub fn from_wire(value: serde_json::Value) -> Result<UiEvent, UiRefusal> {
        let object = value.as_object().ok_or(UiRefusal::NotAnObject)?;
        let ev = object
            .get("ev")
            .and_then(serde_json::Value::as_str)
            .ok_or(UiRefusal::UnknownEvent)?;
        let (_, fields) = UiEvent::DECLARED
            .iter()
            .find(|(name, _)| *name == ev)
            .ok_or(UiRefusal::UnknownEvent)?;
        if object
            .keys()
            .any(|k| k != "ev" && !fields.contains(&k.as_str()))
        {
            return Err(UiRefusal::UnknownField);
        }
        serde_json::from_value(value).map_err(|_| UiRefusal::Invalid)
    }
}

#[cfg(test)]
mod app_event_tests {
    use super::*;
    use serde_json::{json, Value};
    use wasit_diag::DiagEvent;

    /// One valid payload per `ui` event, the base of the tests below (`ui_payloads_cover_every_
    /// declared_event` fails when a variant is added without one).
    fn valid() -> Vec<Value> {
        let t = "u-0123456789abcdef";
        vec![
            json!({"ev":"ui.prompt","trace":t,"host":"local","session":"s-1","kind":"prompt","chars":12,"attachments":1,"bytes":2048}),
            json!({"ev":"ui.upload","trace":t,"files":2,"bytes":4096,"ms":30,"outcome":"error","code":"too_large"}),
            json!({"ev":"ui.command","trace":t,"host":"local","session":"s-1","command":"interrupt"}),
            json!({"ev":"ui.launch","trace":t,"host":"gpu","driver":"claude","resume":"fresh","first_prompt":true,"attachments":0}),
            json!({"ev":"ui.rpc","trace":t,"method":"session.command","ms":12,"outcome":"error","code":"timeout"}),
            json!({"ev":"ui.render_error","component":"Transcript"}),
            json!({"ev":"ui.developer_mode","on":true}),
            json!({"ev":"ui.export","stage":"failed","code":"io"}),
        ]
    }

    #[test]
    fn ui_payloads_cover_every_declared_event() {
        let sent = valid();
        let evs: Vec<&str> = sent.iter().map(|v| v["ev"].as_str().unwrap()).collect();
        let declared: Vec<&str> = UiEvent::DECLARED.iter().map(|(ev, _)| *ev).collect();
        assert_eq!(evs, declared);
        for v in valid() {
            let e = UiEvent::from_wire(v.clone()).unwrap();
            // The wire form is the form a record has: what was sent is what is kept.
            assert_eq!(serde_json::to_value(&e).unwrap(), v);
            // And the record variant has the same name.
            assert_eq!(AppEvent::from(e.clone()).ev(), e.ev());
        }
    }

    #[test]
    fn unknown_variants_and_fields_are_refused() {
        let mut v = valid()[0].clone();
        v["note"] = json!("FREE TEXT MARK");
        assert_eq!(UiEvent::from_wire(v), Err(UiRefusal::UnknownField));
        // The shell's own events cannot be sent by a webview.
        for ev in ["app.start", "app.hub_link", "app.failed", "diag.panic"] {
            assert_eq!(
                UiEvent::from_wire(json!({ "ev": ev })),
                Err(UiRefusal::UnknownEvent),
                "{ev}"
            );
        }
        assert_eq!(
            UiEvent::from_wire(json!({"ev":"ui.prompt"})),
            Err(UiRefusal::Invalid)
        );
        assert_eq!(
            UiEvent::from_wire(json!({"trace":"u-0123456789abcdef"})),
            Err(UiRefusal::UnknownEvent)
        );
        assert_eq!(
            UiEvent::from_wire(json!("ui.prompt")),
            Err(UiRefusal::NotAnObject)
        );
        let mut v = valid()[0].clone();
        v["trace"] = json!("not a trace id");
        assert_eq!(UiEvent::from_wire(v), Err(UiRefusal::Invalid));
    }

    /// For every field of every variant: a string made of free text (spaces, a path, a newline)
    /// there is either refused or comes out without it.
    #[test]
    fn a_free_text_string_in_any_field_never_survives() {
        const MARK: &str = "FREE TEXT MARK /home/u/secret\nline two";
        let mut tried = 0;
        for base in valid() {
            for key in base.as_object().unwrap().keys().filter(|k| *k != "ev") {
                let mut v = base.clone();
                v[key] = json!(MARK);
                tried += 1;
                if let Ok(e) = UiEvent::from_wire(v) {
                    let shown = serde_json::to_string(&e).unwrap();
                    assert!(
                        !shown.contains("MARK") && !shown.contains("secret"),
                        "{key}: {shown}"
                    );
                    assert!(!format!("{e:?}").contains("secret"), "{key}: {e:?}");
                }
            }
        }
        // Every field of every event, optional ones included.
        let fields: usize = UiEvent::DECLARED.iter().map(|(_, f)| f.len()).sum();
        assert_eq!(tried, fields);
    }

    #[test]
    fn words_that_are_not_words_keep_only_their_length() {
        let mut v = valid()[0].clone();
        v["host"] = json!("my host/with spaces");
        v["kind"] = json!("a b");
        let e = UiEvent::from_wire(v).unwrap();
        let shown = serde_json::to_value(&e).unwrap();
        assert_eq!(shown["host"], "<other:19>");
        assert_eq!(shown["kind"], "<other:3>");
    }
}
