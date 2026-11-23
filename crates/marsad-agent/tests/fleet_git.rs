//! The hub's background git summaries in the `fleet` topic: a session in a repository makes
//! its folder's summary appear without any `git.status` call from the client; a change after
//! `fleet.git_refresh` is published; no request per list row.

mod common;

use common::*;
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::{FleetEvent, GitSummary};
use marsad_proto::HostId;
use serde_json::json;
use wasit_fake_cli::scenario::{Scenario, Turn};

async fn summary_until(
    fleet: &mut marsad_client::Subscription<FleetEvent>,
    cwd: &str,
    pred: impl Fn(&GitSummary) -> bool,
) -> GitSummary {
    loop {
        let (_, e) = next(fleet).await;
        let found = match e {
            FleetEvent::Reset { hosts } => hosts
                .into_iter()
                .flat_map(|h| h.git)
                .find(|g| g.cwd == cwd && pred(g)),
            FleetEvent::Git { summary, .. } if summary.cwd == cwd && pred(&summary) => {
                Some(summary)
            }
            _ => None,
        };
        if let Some(g) = found {
            return g;
        }
    }
}

#[tokio::test]
async fn sessions_bring_their_folders_git_summary_into_the_fleet() {
    let d = TempDir::new();
    let data = d.path().join("d");
    let repo = d.path().join("repo");
    git_repo(&repo);
    std::fs::write(repo.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    let cwd = repo.to_string_lossy().into_owned();
    let c = client(&data);
    let mut fleet = c.fleet(None);

    let sc = Scenario::builder()
        .turn(
            Turn::builder()
                .tool("Bash", json!({"command": "ls"}))
                .result_if_allowed("listed")
                .done()
                .say("done"),
        )
        .build();
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(cwd.clone());
    req.first_prompt = Some("go".into());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &sc),
        ..OverrideLayer::default()
    };
    let launched = c.launch(req).await.unwrap();

    let g = summary_until(&mut fleet, &cwd, |g| g.files == 1).await;
    assert_eq!(g.branch.as_deref(), Some("main"));
    assert_eq!((g.added, g.removed), (2, 1), "{g:?}");
    assert!(g.error.is_none());

    // A change the session did not announce shows after an explicit refresh.
    std::fs::write(repo.join("new.txt"), "n\n").unwrap();
    c.git_refresh(launched.host.clone(), cwd.clone())
        .await
        .unwrap();
    let g = summary_until(&mut fleet, &cwd, |g| g.files == 2).await;
    assert_eq!(g.added, 3);

    // A folder that is not a repository reports why.
    let plain = d.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let plain = plain.to_string_lossy().into_owned();
    c.git_refresh(HostId::new("hub"), plain.clone())
        .await
        .unwrap();
    // Not a session folder yet: nothing is published for it (summaries follow sessions);
    // once a session works there, its summary says why there is none.
    let mut req = LaunchRequest::new("claude");
    req.cwd = Some(plain.clone());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &Scenario::builder().build()),
        ..OverrideLayer::default()
    };
    c.launch(req).await.unwrap();
    let g = summary_until(&mut fleet, &plain, |g| g.error.is_some()).await;
    assert!(g.repo_root.is_none());
    let out = std::process::Command::new(agent())
        .arg("stop")
        .args(data_args(&data))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}
