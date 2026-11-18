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
        p::hosts::LinkStates,
        p::fleet::FleetEvent,
        p::fleet::FleetHost,
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
        // Wasit runtime
        rt::HostInfo,
        rt::SessionList,
        rt::SessionSnapshot,
        rt::SessionEvent,
        rt::HostEvent,
        rt::CreateSession,
        rt::CreateSessionReply,
        rt::SessionRef,
        rt::HistoryRequest,
        rt::HistoryPage,
        rt::CommandRequest,
        rt::CommandReply,
        rt::CommandError,
        rt::RuntimeError,
        rt::InventoryRequest,
        rt::InventoryReply,
        rt::RefreshRequest,
        rt::Ack,
        // Wasit core
        p::AgentEvent,
        p::Command,
        p::GateDecision,
        p::GateRequest,
        p::QuestionAnswer,
        p::ConfigItem,
        p::Unsupported,
    );
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
        if stem == "index" {
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
    Ok(names.len() + 1)
}
