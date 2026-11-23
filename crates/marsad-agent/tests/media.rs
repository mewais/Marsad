//! Images and files on a REMOTE host over a real ssh link (localhost sshd): an attachment
//! uploaded in chunks over the control link and sent with a prompt, an image a tool returned
//! served from the host's blob store, a file the agent wrote read back with ranges, and the
//! host refusing paths outside the session's folder (traversal, symlink escape).
#![cfg(target_os = "linux")]

mod common;

use std::path::Path;
use std::time::Duration;

use common::*;
use marsad_client::{MarsadClient, Subscription};
use marsad_proto::agentconfig::{LaunchRequest, OverrideLayer};
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::HostDraft;
use marsad_proto::media::FileReadRequest;
use marsad_proto::runtime::{SessionEvent, SessionEventKind};
use marsad_proto::{AgentEvent, Command, ContentPart, HostId, MediaSource};
use serde_json::json;
use wasit_core::content::{b64_decode, b64_encode};
use wasit_fake_cli::scenario::{ImageStep, Scenario, Step, ToolStep, Turn, WriteFileStep};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAF0lEQVR4nGP4z8BAEiJN9aiGUQ1DSgMAkPn/Afnh+ngAAAAASUVORK5CYII=";

fn hub_client(dir: &Path) -> MarsadClient {
    let mut cfg = client_config(dir, wasit_control::Hello::new("test", "gui", "gui"));
    cfg.serve_args.extend(serve_args());
    cfg.supervise.backoff.initial = Duration::from_millis(50);
    MarsadClient::connect(cfg)
}

async fn connected(fleet: &mut Subscription<FleetEvent>, host: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let e = tokio::time::timeout_at(deadline, next(fleet))
            .await
            .expect("host connected in time");
        if matches!(&e.1, FleetEvent::Link { host: h, state } if h.as_str() == host && state.is_connected())
        {
            return;
        }
    }
}

async fn event(sub: &mut wasit_runtime::TypedSubscription<SessionEvent>) -> SessionEvent {
    match tokio::time::timeout(Duration::from_secs(60), sub.next()).await {
        Ok(Some(Ok((_, e)))) => e,
        _ => panic!("no session event"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attachments_blobs_and_files_travel_over_the_ssh_link() {
    let d = TempDir::new();
    let sshd = Sshd::start(d.path());
    let c = hub_client(&d.path().join("l"));
    let mut fleet = c.fleet(None);
    let box_id = HostId::new("box");
    c.add_host(HostDraft {
        name: "Box".into(),
        kind: sshd.kind(&sshd.key),
        id: Some(box_id.clone()),
    })
    .await
    .unwrap();
    connected(&mut fleet, "box").await;

    let work = sshd.remote_home.join("w");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(sshd.remote_home.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(sshd.remote_home.join("secret.txt"), work.join("link.txt")).unwrap();
    let sc = Scenario::builder()
        .turn(Turn {
            when: None,
            steps: vec![
                Step::Tool(ToolStep {
                    name: "Write".into(),
                    input: json!({"file_path": "chart.svg"}),
                    image: Some(ImageStep {
                        mime: Some("image/png".into()),
                        path: None,
                        data: Some(PNG.into()),
                    }),
                    write_file: Some(WriteFileStep {
                        path: "chart.svg".into(),
                        content: "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>"
                            .into(),
                    }),
                    ..ToolStep::default()
                }),
                Step::Say {
                    text: "made chart.svg".into(),
                    chunks: None,
                },
            ],
        })
        .build();
    let mut req = LaunchRequest::new("claude");
    req.host = Some(box_id.clone());
    req.cwd = Some(work.to_string_lossy().into_owned());
    req.overrides = OverrideLayer {
        env: scenario_env(d.path(), &sc),
        ..OverrideLayer::default()
    };
    let id = c.launch(req).await.unwrap().id;
    let rt = c.runtime(Some(box_id.clone()));
    let mut sub = rt.subscribe_session(&id, None);
    loop {
        let e = event(&mut sub).await;
        if matches!(&e.kind, SessionEventKind::Phase { phase, .. } if phase.name() == "idle") {
            break;
        }
    }

    // A 2.5 MiB attachment crosses the link in 1 MiB chunks into the host's attachments.
    let big: Vec<u8> = (0..2_621_440u32).map(|i| (i % 251) as u8).collect();
    let att = rt.upload(&id, "upabcdef1", "data.bin", &big).await.unwrap();
    assert!(att.path.contains("attachments"), "{}", att.path);
    assert_eq!(
        std::fs::read(&att.path).unwrap(),
        big,
        "bytes landed on the host"
    );
    let img = rt
        .upload(&id, "upabcdef2", "red.png", &b64_decode(PNG).unwrap())
        .await
        .unwrap();
    assert_eq!(img.mime, "image/png");
    rt.command(
        &id,
        Command::Prompt {
            text: "make a chart like this".into(),
            attachments: vec![img, att],
        },
    )
    .await
    .unwrap();
    let mut blob = None;
    loop {
        let e = event(&mut sub).await;
        match &e.kind {
            SessionEventKind::Input {
                command: Command::Prompt { attachments, .. },
                ..
            } => assert_eq!(attachments.len(), 2),
            SessionEventKind::Agent {
                event: AgentEvent::ToolCallFinished { parts, .. },
            } => {
                if let Some(ContentPart::Image {
                    source: MediaSource::Blob { id, .. },
                    ..
                }) = parts.first()
                {
                    blob = Some(id.clone());
                }
            }
            SessionEventKind::Agent {
                event: AgentEvent::TurnCompleted { .. },
            } => break,
            _ => {}
        }
    }
    let read = |blob: Option<String>, path: Option<&str>, offset: u64, length: Option<u64>| {
        FileReadRequest {
            id: id.clone(),
            blob,
            path: path.map(str::to_string),
            offset,
            length,
            outside: false,
        }
    };
    let b = rt
        .file_read(read(blob.clone(), None, 0, None))
        .await
        .unwrap();
    assert_eq!(b64_encode(&b64_decode(&b.data).unwrap()), PNG);
    assert_eq!(b.mime, "image/png");
    // The file the agent wrote, whole and as a range.
    let f = rt
        .file_read(read(None, Some("chart.svg"), 0, None))
        .await
        .unwrap();
    assert_eq!(f.mime, "image/svg+xml");
    assert!(f.eof);
    let r = rt
        .file_read(read(None, Some("chart.svg"), 1, Some(3)))
        .await
        .unwrap();
    assert_eq!(b64_decode(&r.data).unwrap(), b"svg");
    assert!(!r.eof);
    let st = rt
        .files_stat(&id, vec!["chart.svg".into(), "nope.txt".into()])
        .await
        .unwrap();
    assert_eq!(st.files.len(), 1);
    // Outside the session's folder: refused on the host.
    for bad in ["../secret.txt", "link.txt"] {
        assert!(
            rt.file_read(read(None, Some(bad), 0, None)).await.is_err(),
            "{bad} was readable"
        );
    }
    assert!(rt
        .file_read(read(Some("../x".into()), None, 0, None))
        .await
        .is_err());
    // The link stays healthy for other calls.
    assert!(c.hosts().await.unwrap().iter().any(|h| h.id == box_id));
    rt.delete(&id).await.unwrap();
    if let Ok(i) = c.daemon_info(None).await {
        stop_pid(i.pid);
    }
}
