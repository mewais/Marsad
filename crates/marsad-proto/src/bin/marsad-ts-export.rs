//! Writes TypeScript definitions of every wire type a GUI needs (Marsad's and Wasit's) into
//! one directory, one file per type with relative imports, plus an `index.ts`.
//!
//! ```text
//! cargo run -p marsad-proto --features ts --bin marsad-ts-export -- apps/desktop/src/bridge/generated
//! ```
//!
//! JSON numbers: the wire is plain JSON, so 64-bit integers are exported as `number` (all such
//! values are counters, cursors and millisecond times well inside 2^53).

use std::path::{Path, PathBuf};

use marsad_proto as p;
use ts_rs::TS;

fn main() -> std::process::ExitCode {
    let Some(dir) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: marsad-ts-export <output dir>");
        return std::process::ExitCode::from(2);
    };
    match run(&dir) {
        Ok(n) => {
            println!("wrote {n} files to {}", dir.display());
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("marsad-ts-export: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

macro_rules! export {
    ($cfg:expr, $($t:ty),* $(,)?) => {{
        $( <$t as TS>::export_all($cfg).map_err(|e| format!("{}: {e}", stringify!($t)))?; )*
    }};
}

fn run(dir: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    use p::runtime as rt;
    let cfg = ts_rs::Config::new()
        .with_out_dir(dir)
        .with_large_int("number");
    export!(
        &cfg,
        // Marsad
        p::MarsadError,
        p::git::GitStatusRequest,
        p::git::GitStatus,
        p::git::GitDiffRequest,
        p::git::GitDiff,
        p::git::GitCommitRequest,
        p::git::GitCommitReply,
        p::git::GitDiscardRequest,
        p::pty::PtyStartRequest,
        p::pty::PtyInfo,
        p::pty::PtyList,
        p::pty::PtyRef,
        p::pty::PtyInputRequest,
        p::pty::PtyResizeRequest,
        p::pty::PtyEvent,
        p::fs::FsListRequest,
        p::ask::AskUserRequest,
        p::ask::AskUserReply,
        p::fs::FsListing,
        p::fs::FsHome,
        p::fs::FsPathRequest,
        p::fs::FsExists,
        p::daemon::VersionInfo,
        p::daemon::DaemonInfo,
        p::hosts::HostEntry,
        p::hosts::HostDraft,
        p::hosts::HostRef,
        p::hosts::HostList,
        p::hosts::ImportSuggestions,
        p::hosts::WslDistro,
        p::hosts::WslDistros,
        p::hosts::LinkStates,
        p::hosts::AuthAnswer,
        p::hosts::AuthCancel,
        p::hosts::AuthPrompt,
        p::hosts::HostEnvSettings,
        p::hosts::HostVar,
        p::hosts::HostEnvSaved,
        p::hosts::SecretsLocation,
        p::export::ExportRequest,
        p::export::ExportStarted,
        p::export::ExportStatus,
        p::export::ExportState,
        p::export::ExportSource,
        p::export::ExportSourceState,
        p::export::ExportsEvent,
        p::fleet::FleetEvent,
        p::fleet::FleetHost,
        p::fleet::GitSummary,
        p::fleet::GitRefresh,
        p::agentconfig::BaseRequest,
        p::agentconfig::BaseConfig,
        p::agentconfig::EffectiveRequest,
        p::agentconfig::EffectiveConfig,
        p::agentconfig::LaunchRequest,
        p::agentconfig::LaunchReply,
        p::agentconfig::OverrideLayer,
        p::agentconfig::LayerScope,
        p::agentconfig::AutomaticItem,
        p::store::Settings,
        p::store::SettingSet,
        p::store::NotificationPrefs,
        p::store::LayerList,
        p::store::StoredLayer,
        p::store::SeenMark,
        p::store::SeenMarkers,
        p::store::RecentProject,
        p::store::RecentProjects,
        // Workstreams
        p::workstream::Workstream,
        p::workstream::WorkstreamTemplate,
        p::workstream::WorkstreamDraft,
        p::workstream::WorkstreamUpdate,
        p::workstream::WorkstreamRef,
        p::workstream::WorkstreamList,
        p::workstream::LaunchWorkstreamReply,
        p::workstream::AddNode,
        p::workstream::NodeRef,
        p::workstream::Connect,
        p::workstream::EdgeRef,
        p::workstream::SaveTemplate,
        p::workstream::FromTemplate,
        p::workstream::TemplateRef,
        p::workstream::TemplateList,
        p::workstream::EdgeHistoryRequest,
        p::workstream::EdgeHistoryPage,
        p::workstream::WorkstreamsEvent,
        p::workstream::WorkstreamView,
        // Wasl
        p::wasl::AgentId,
        p::wasl::Topology,
        p::wasl::TopologyApplied,
        p::wasl::TopologyList,
        p::wasl::PeerList,
        p::wasl::DelegateRequest,
        p::wasl::DelegateReply,
        p::wasl::MessageRequest,
        p::wasl::TaskIds,
        p::wasl::WaitRequest,
        p::wasl::TaskStates,
        p::wasl::CancelRequest,
        p::wasl::DecideRequest,
        p::wasl::TaskList,
        p::wasl::TaskEvent,
        p::wasl::TaskBrief,
        p::wasl::EdgeEvent,
        p::wasl::HandoffDecision,
        // Wasit control
        p::Hello,
        p::RpcError,
        p::CloseReason,
        p::LinkState,
        p::Cursor,
        p::HostId,
        wasit_control::TraceId,
        // Marsad diagnostics (what the webview may record in the app log)
        p::diag::UiEvent,
        // Wasit runtime
        rt::HostInfo,
        rt::HostEnvironment,
        rt::EnvSource,
        rt::SetupReport,
        rt::EnvironmentTest,
        rt::SessionList,
        rt::SessionSnapshot,
        rt::NotContinuable,
        rt::ResumeCommand,
        rt::SessionEvent,
        rt::HostEvent,
        rt::CreateSession,
        rt::CreateSessionReply,
        rt::SessionRef,
        rt::RenameSession,
        rt::ResumeRequest,
        rt::HistoryRequest,
        rt::HistoryPage,
        rt::TrailRequest,
        rt::TrailItem,
        rt::TrailPage,
        rt::DiagSources,
        rt::DiagSource,
        rt::DiagGen,
        rt::DiagReadRequest,
        rt::DiagChunk,
        rt::DiagTailParams,
        rt::DiagTailEvent,
        wasit_runtime::trail::TrailEntry,
        wasit_runtime::trail::TrailCause,
        rt::DiagSources,
        rt::DiagSource,
        rt::DiagGen,
        rt::DiagReadRequest,
        rt::DiagChunk,
        rt::DiagTailParams,
        rt::DiagTailEvent,
        rt::CommandRequest,
        rt::CommandReply,
        rt::CommandError,
        rt::RuntimeError,
        rt::InventoryRequest,
        rt::InventoryReply,
        rt::RefreshRequest,
        rt::Ack,
        p::media::AttachmentPut,
        p::media::AttachmentPutReply,
        p::media::FileReadRequest,
        p::media::FileChunk,
        p::media::FilesStatRequest,
        p::media::FilesStatReply,
        p::media::FileStat,
        // Wasit core
        p::AgentEvent,
        p::Command,
        p::GateDecision,
        p::GateRequest,
        p::GateChoice,
        p::GateChoiceKind,
        p::ChoiceText,
        p::QuestionAnswer,
        p::ConfigItem,
        p::Unsupported,
        p::ContentPart,
        p::MediaSource,
        p::Attachment,
        p::PromptInput,
        p::FileInput,
    );
    write_names(dir)?;
    let mut names = Vec::new();
    for ent in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = ent.map_err(|e| e.to_string())?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ts") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        if stem == "index" || stem == "names" {
            continue;
        }
        names.push(stem);
    }
    names.sort();
    let index: String = names
        .iter()
        .map(|n| format!("export type * from \"./{n}\";\n"))
        .collect();
    std::fs::write(
        dir.join("index.ts"),
        format!("// Generated by marsad-ts-export. Do not edit.\n{index}"),
    )
    .map_err(|e| e.to_string())?;
    Ok(names.len() + 2)
}

/// `names.ts`: every method and topic name a GUI calls, from the Rust constants (Wasit
/// runtime, Marsad host and hub services, Wasl), so the UI never spells one itself.
fn write_names(dir: &Path) -> Result<(), String> {
    use p::methods as m;
    use p::runtime::{methods as rm, topics as rt};
    use p::wasl::{methods as wm, topics as wt};
    let methods: &[(&str, &str)] = &[
        ("HOST_INFO", rm::HOST_INFO),
        ("DRIVERS_INVENTORY", rm::DRIVERS_INVENTORY),
        ("DRIVERS_REFRESH", rm::DRIVERS_REFRESH),
        ("HOST_ENVIRONMENT", rm::HOST_ENVIRONMENT),
        ("HOST_ENVIRONMENT_REFRESH", rm::HOST_ENVIRONMENT_REFRESH),
        ("SESSIONS_LIST", rm::SESSIONS_LIST),
        ("SESSIONS_CREATE", rm::SESSIONS_CREATE),
        ("SESSIONS_CONTINUE", rm::SESSIONS_CONTINUE),
        ("SESSIONS_STOP", rm::SESSIONS_STOP),
        ("SESSIONS_DELETE", rm::SESSIONS_DELETE),
        ("SESSIONS_RENAME", rm::SESSIONS_RENAME),
        ("SESSION_SNAPSHOT", rm::SESSION_SNAPSHOT),
        ("SESSION_HISTORY", rm::SESSION_HISTORY),
        ("SESSION_TRAIL_PAGE", rm::SESSION_TRAIL_PAGE),
        ("DIAG_SOURCES", rm::DIAG_SOURCES),
        ("DIAG_READ", rm::DIAG_READ),
        ("SESSION_COMMAND", rm::SESSION_COMMAND),
        ("SESSION_ATTACHMENT_PUT", rm::SESSION_ATTACHMENT_PUT),
        ("SESSION_FILE_READ", rm::SESSION_FILE_READ),
        ("SESSION_FILES_STAT", rm::SESSION_FILES_STAT),
        ("GIT_STATUS", m::GIT_STATUS),
        ("GIT_DIFF", m::GIT_DIFF),
        ("GIT_COMMIT", m::GIT_COMMIT),
        ("GIT_DISCARD", m::GIT_DISCARD),
        ("PTY_START", m::PTY_START),
        ("PTY_INPUT", m::PTY_INPUT),
        ("PTY_RESIZE", m::PTY_RESIZE),
        ("PTY_CLOSE", m::PTY_CLOSE),
        ("PTY_LIST", m::PTY_LIST),
        ("FS_LIST", m::FS_LIST),
        ("FS_HOME", m::FS_HOME),
        ("FS_EXISTS", m::FS_EXISTS),
        ("ASK_USER", m::ASK_USER),
        ("AGENTCONFIG_BASE", m::AGENTCONFIG_BASE),
        ("AGENTCONFIG_EFFECTIVE", m::AGENTCONFIG_EFFECTIVE),
        ("SESSIONS_LAUNCH", m::SESSIONS_LAUNCH),
        ("FLEET_GIT_REFRESH", m::FLEET_GIT_REFRESH),
        ("DAEMON_INFO", m::DAEMON_INFO),
        ("HOSTS_LIST", m::HOSTS_LIST),
        ("HOSTS_ADD", m::HOSTS_ADD),
        ("HOSTS_UPDATE", m::HOSTS_UPDATE),
        ("HOSTS_REMOVE", m::HOSTS_REMOVE),
        ("HOSTS_IMPORT_SUGGESTIONS", m::HOSTS_IMPORT_SUGGESTIONS),
        ("HOSTS_WSL_DISTROS", m::HOSTS_WSL_DISTROS),
        ("HOSTS_RECONNECT", m::HOSTS_RECONNECT),
        ("HOSTS_AUTH_ANSWER", m::HOSTS_AUTH_ANSWER),
        ("HOSTS_AUTH_CANCEL", m::HOSTS_AUTH_CANCEL),
        ("HOSTS_ENVIRONMENT_GET", m::HOSTS_ENVIRONMENT_GET),
        ("HOSTS_ENVIRONMENT_SET", m::HOSTS_ENVIRONMENT_SET),
        ("HOSTS_ENVIRONMENT_TEST", m::HOSTS_ENVIRONMENT_TEST),
        ("HOSTS_ENVIRONMENT_SECRETS", m::HOSTS_ENVIRONMENT_SECRETS),
        ("DIAGNOSTICS_EXPORT_START", m::DIAGNOSTICS_EXPORT_START),
        ("STORE_SETTINGS_GET", m::STORE_SETTINGS_GET),
        ("STORE_SETTINGS_SET", m::STORE_SETTINGS_SET),
        ("STORE_NOTIFICATIONS_GET", m::STORE_NOTIFICATIONS_GET),
        ("STORE_NOTIFICATIONS_SET", m::STORE_NOTIFICATIONS_SET),
        ("STORE_LAYERS_LIST", m::STORE_LAYERS_LIST),
        ("STORE_LAYERS_GET", m::STORE_LAYERS_GET),
        ("STORE_LAYERS_SET", m::STORE_LAYERS_SET),
        ("STORE_SEEN_LIST", m::STORE_SEEN_LIST),
        ("STORE_SEEN_MARK", m::STORE_SEEN_MARK),
        ("STORE_SEEN_MARK_ALL", m::STORE_SEEN_MARK_ALL),
        ("STORE_RECENT_LIST", m::STORE_RECENT_LIST),
        ("STORE_RECENT_ADD", m::STORE_RECENT_ADD),
        ("WS_LIST", m::WS_LIST),
        ("WS_GET", m::WS_GET),
        ("WS_CREATE", m::WS_CREATE),
        ("WS_UPDATE", m::WS_UPDATE),
        ("WS_DELETE", m::WS_DELETE),
        ("WS_LAUNCH", m::WS_LAUNCH),
        ("WS_ADD_NODE", m::WS_ADD_NODE),
        ("WS_REMOVE_NODE", m::WS_REMOVE_NODE),
        ("WS_CONNECT", m::WS_CONNECT),
        ("WS_DISCONNECT", m::WS_DISCONNECT),
        ("WS_SET_POLICY", m::WS_SET_POLICY),
        ("WS_PAUSE", m::WS_PAUSE),
        ("WS_RESUME", m::WS_RESUME),
        ("WS_ARCHIVE", m::WS_ARCHIVE),
        ("WS_SAVE_TEMPLATE", m::WS_SAVE_TEMPLATE),
        ("WS_FROM_TEMPLATE", m::WS_FROM_TEMPLATE),
        ("WS_TEMPLATES_LIST", m::WS_TEMPLATES_LIST),
        ("WS_TEMPLATES_DELETE", m::WS_TEMPLATES_DELETE),
        ("WS_HISTORY", m::WS_HISTORY),
        ("WASL_TASKS_LIST", wm::TASKS_LIST),
        ("WASL_TASK_CANCEL", wm::TASK_CANCEL),
    ];
    let topics: &[(&str, &str)] = &[
        ("FLEET", p::topics::FLEET),
        ("HOSTS_LINKS", p::topics::HOSTS_LINKS),
        ("WORKSTREAMS", p::topics::WORKSTREAMS),
        ("DIAGNOSTICS_EXPORTS", p::topics::DIAGNOSTICS_EXPORTS),
        ("HOST_SESSIONS", rt::HOST_SESSIONS),
        ("DIAG_TAIL", rt::DIAG_TAIL),
        ("DRIVERS_INVENTORY", rt::DRIVERS_INVENTORY),
        ("DIAG_TAIL", rt::DIAG_TAIL),
        ("WASL_TASKS", wt::TASKS),
        ("WASL_EDGES", wt::EDGES),
    ];
    let prefixes: &[(&str, &str)] = &[
        ("SESSION_EVENTS", rt::SESSION_EVENTS_PREFIX),
        ("SESSION_TRAIL", rt::SESSION_TRAIL_PREFIX),
        ("PTY_OUTPUT", p::topics::PTY_OUTPUT_PREFIX),
    ];
    let table = |rows: &[(&str, &str)]| -> String {
        rows.iter()
            .map(|(k, v)| format!("  {k}: {v:?},\n"))
            .collect()
    };
    let out = format!(
        "// Generated by marsad-ts-export from the Rust method/topic constants. Do not edit.\n\
         export const methods = {{\n{}}} as const;\n\n\
         export const topics = {{\n{}}} as const;\n\n\
         /** Prefixes of per-object topics (append the session / terminal id). */\n\
         export const topicPrefixes = {{\n{}}} as const;\n",
        table(methods),
        table(topics),
        table(prefixes),
    );
    std::fs::write(dir.join("names.ts"), out).map_err(|e| e.to_string())
}
