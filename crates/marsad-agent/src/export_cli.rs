//! `marsad-agent diagnostics export [--since <duration>] [--out <path>] [--no-trails] [options]`:
//! the diagnostics export for when the app cannot start (design §6.7).
//!
//! With a hub serving this machine's endpoint it asks the hub (`diagnostics.export.start`,
//! follows the topic `diagnostics.exports` until the export ends) and copies the file the hub
//! wrote to `--out` (default: the current directory); the file is the one the app's Settings
//! would save. With no hub — no daemon at all, or a daemon that is not the hub — it exports this
//! machine's own files (`daemon`, `aux` and `app` logs of `<data>/diag/`, the shim log of every
//! session directory), without trails or host facts: the manifest says `mode: local_files` and
//! why. Either way a failure is on stderr with a non-zero exit status; an export that is
//! incomplete (a host was not connected, a section was left out) says so on stderr and exits 0:
//! the file is what could be had.
//!
//! `--since` takes `<n>s`, `<n>m`, `<n>h` or `<n>d` (default `24h`, at most `31d`). The other
//! options are those of `serve` that name the machine's files (`--data`, `--run-dir`,
//! `--socket`, `--host-id`).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use marsad_proto::export::{
    ExportRequest, ExportSourceState, ExportStarted, ExportState, ExportStatus, ExportsEvent,
    DEFAULT_WINDOW_MS, EXPORT_DIR, MAX_WINDOW_MS,
};
use marsad_proto::{methods, topics};
use wasit_control::{CallError, ClientOptions, ConnectError, ControlClient, Hello};

use crate::cli::{default_host_id, Options};
use crate::hub::export::job::{self, HubFacts, Job, Limits, Mode, Update};
use crate::hub::export::source::FileLogs;
use crate::util::now_ms;
use crate::version;

/// How a request to start an export is waited for.
const START_DEADLINE: Duration = Duration::from_secs(30);

struct Args {
    since_ms: u64,
    out: Option<PathBuf>,
    trails: bool,
    options: Options,
}

/// `<n>s`, `<n>m`, `<n>h`, `<n>d`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let bad = || format!("{text:?} is not a duration (use 30m, 24h, 3d, …)");
    let (n, unit) = text.split_at(text.len().saturating_sub(1));
    let n: u64 = n.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => Some(n),
        "m" => n.checked_mul(60),
        "h" => n.checked_mul(3600),
        "d" => n.checked_mul(86_400),
        _ => return Err(bad()),
    }
    .ok_or_else(bad)?;
    Ok(Duration::from_secs(secs))
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut window = Duration::from_millis(DEFAULT_WINDOW_MS);
    let (mut out, mut trails, mut rest) = (None, true, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--since" => {
                window = parse_duration(it.next().ok_or("--since needs a value")?)?;
            }
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "--no-trails" => trails = false,
            other => {
                rest.push(other.to_owned());
                // The option's value (every option of `serve` takes one except `--hub`).
                if other.starts_with("--") && other != "--hub" {
                    if let Some(v) = it.next() {
                        rest.push(v.clone());
                    }
                }
            }
        }
    }
    if window.as_millis() as u64 > MAX_WINDOW_MS {
        return Err("--since is at most 31d".into());
    }
    Ok(Args {
        since_ms: now_ms().saturating_sub(window.as_millis() as u64),
        out,
        trails,
        options: Options::parse(&rest)?,
    })
}

/// The command; `args` follow `diagnostics`.
pub async fn run(args: &[String]) -> ExitCode {
    let rest = match args.split_first() {
        Some((sub, rest)) if sub == "export" => rest,
        _ => {
            eprintln!("usage: marsad-agent diagnostics export [--since <n>s|m|h|d] [--out <path>] [--no-trails] [--data <dir>]");
            return ExitCode::from(2);
        }
    };
    let a = match parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("marsad-agent: {e}");
            return ExitCode::from(2);
        }
    };
    let finished = export(&a).await;
    match finished {
        Ok(done) => match deliver(&a, &done.dir, &done.file).await {
            Ok(path) => {
                println!("{}", path.display());
                if !done.complete {
                    eprintln!("note: the export is incomplete: {}", done.why);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("marsad-agent: {e}");
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("marsad-agent: {e}");
            ExitCode::from(1)
        }
    }
}

struct Done {
    /// The directory of the exports and the file's name in it.
    dir: PathBuf,
    file: String,
    complete: bool,
    /// What is missing, in words.
    why: String,
}

fn missing(status: &ExportStatus) -> String {
    let rows: Vec<String> = status
        .sources
        .iter()
        .filter(|s| s.state != ExportSourceState::Done)
        .map(|s| {
            format!(
                "{} {:?}{}",
                s.source,
                s.state,
                s.reason
                    .as_deref()
                    .map(|r| format!(" ({r})"))
                    .unwrap_or_default()
            )
        })
        .collect();
    if rows.is_empty() {
        "some sections were left out".into()
    } else {
        rows.join("; ")
    }
}

async fn export(a: &Args) -> Result<Done, String> {
    let o = &a.options;
    let dir = o.layout().diag().join(EXPORT_DIR);
    let opts = ClientOptions::new(Hello::new(
        version::build_id(),
        "marsad-agent-diagnostics",
        "cli",
    ));
    let client = match ControlClient::connect_endpoint(&o.endpoint(), opts).await {
        Ok(c) => c,
        Err(ConnectError::Io(_)) => return local_files(a, dir, "no_daemon").await,
        Err(e) => return Err(format!("cannot reach the daemon: {e}")),
    };
    let request = ExportRequest {
        since_ms: Some(a.since_ms),
        include_trails: a.trails,
    };
    let started: ExportStarted = match client
        .request(methods::DIAGNOSTICS_EXPORT_START, request, START_DEADLINE)
        .await
    {
        Ok(s) => s,
        // A daemon that is not the hub: this machine's files are all there is to export.
        Err(CallError::Rpc(r)) if r.code == wasit_control::codes::METHOD_NOT_FOUND => {
            return local_files(a, dir, "not_a_hub").await;
        }
        Err(e) => return Err(format!("the hub refused the export: {e}")),
    };
    // The topic starts with every export the hub knows, ours included; it ends when ours does.
    let mut sub = client
        .subscribe(topics::DIAGNOSTICS_EXPORTS, None)
        .await
        .map_err(|e| format!("cannot follow the export: {e}"))?;
    let mut shown: Vec<(String, ExportSourceState)> = Vec::new();
    while let Some((_, v)) = sub.next().await {
        let ev: ExportsEvent = serde_json::from_value(v)
            .map_err(|e| format!("cannot read the hub's progress: {e}"))?;
        let mine: Vec<ExportStatus> = match ev {
            ExportsEvent::Reset { exports } => exports,
            ExportsEvent::Upsert { export } => vec![export],
        }
        .into_iter()
        .filter(|s| s.id == started.id)
        .collect();
        for s in mine {
            for src in &s.sources {
                let key = (src.source.clone(), src.state);
                if !shown.contains(&key)
                    && !matches!(
                        src.state,
                        ExportSourceState::Waiting | ExportSourceState::Reading
                    )
                {
                    eprintln!(
                        "  {}: {:?}{}",
                        src.source,
                        src.state,
                        src.reason
                            .as_deref()
                            .map(|r| format!(" ({r})"))
                            .unwrap_or_default()
                    );
                    shown.push(key);
                }
            }
            match s.state {
                ExportState::Running => {}
                ExportState::Failed => {
                    return Err(format!(
                        "the export failed: {}",
                        s.error.as_deref().unwrap_or("no reason given")
                    ));
                }
                ExportState::Done => {
                    let file = s.file.clone().ok_or("the hub reported no file")?;
                    return Ok(Done {
                        dir,
                        file,
                        complete: s.complete.unwrap_or(false),
                        why: missing(&s),
                    });
                }
            }
        }
    }
    Err("the hub closed the connection before the export ended".into())
}

/// No hub: this machine's own files, in the same file format.
async fn local_files(a: &Args, dir: PathBuf, note: &'static str) -> Result<Done, String> {
    let o = &a.options;
    eprintln!(
        "no hub is serving {}: exporting this machine's files only",
        o.socket_path().display()
    );
    let (diag_dir, sessions) = (o.layout().diag(), o.layout().data.join("sessions"));
    let logs = tokio::task::spawn_blocking(move || FileLogs::machine(diag_dir, sessions))
        .await
        .map_err(|e| format!("cannot list the files: {e}"))?;
    let now = now_ms();
    let progress: job::Progress = Arc::new(|u| {
        if let Update::Source(s) = u {
            if !matches!(
                s.state,
                ExportSourceState::Waiting | ExportSourceState::Reading
            ) {
                eprintln!("  {}: {:?}", s.source, s.state);
            }
        }
    });
    let finished = job::run(Job {
        id: job::new_id(now),
        mode: Mode::LocalFiles { note },
        since_ms: a.since_ms,
        include_trails: false,
        hub: HubFacts {
            host_id: o.host_id.clone().unwrap_or_else(default_host_id),
            build: version::build_id(),
            target: version::target().to_owned(),
            machine: o.layout().machine.clone(),
            pid: std::process::id(),
        },
        hosts: Vec::new(),
        app: None,
        local: Some(Arc::new(logs)),
        dir: dir.clone(),
        limits: Limits::default(),
        progress,
    })
    .await
    .map_err(|e| e.words)?;
    Ok(Done {
        dir,
        file: finished.name,
        complete: finished.complete,
        why: "some sections were left out".into(),
    })
}

/// Copies the export to where the user asked (the current directory by default).
async fn deliver(a: &Args, dir: &Path, file: &str) -> Result<PathBuf, String> {
    // The name comes from the hub: a plain file name or nothing.
    if file.is_empty() || file.contains(['/', '\\']) || file.starts_with('.') {
        return Err("the hub named a file that is not a plain file name".into());
    }
    let from = dir.join(file);
    let to = match &a.out {
        Some(p) if p.is_dir() => p.join(file),
        Some(p) => p.clone(),
        None => PathBuf::from(file),
    };
    let copy = (from.clone(), to.clone());
    tokio::task::spawn_blocking(move || std::fs::copy(&copy.0, &copy.1))
        .await
        .map_err(|e| format!("cannot copy the export: {e}"))?
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_a_number_and_a_unit() {
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86_400));
        assert_eq!(parse_duration("3d").unwrap(), Duration::from_secs(259_200));
        assert_eq!(parse_duration("90m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        for bad in ["", "h", "24", "1.5h", "-1h", "2w", "99999999999999999999d"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
    }
}
