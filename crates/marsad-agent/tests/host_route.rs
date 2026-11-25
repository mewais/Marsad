//! Editing a host's route (the machine it reaches) takes the old machine's state out of the fleet:
//! the host is removed and added again, so no session, git row or feed error of the old machine is
//! kept for the new one.

mod common;

use std::cell::Cell;

use common::*;
use marsad_proto::fleet::FleetEvent;
use marsad_proto::hosts::{HostDraft, HostKind};
use marsad_proto::{HostId, LinkState};

/// A port on this machine that nothing listens on (ssh says "Connection refused" at once).
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn ssh(port: u16) -> HostKind {
    HostKind::Ssh {
        target: "route-user@127.0.0.1".into(),
        port: Some(port),
        identity_file: None,
        options: vec![],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_route_change_removes_the_host_before_it_is_added_again() {
    let d = TempDir::new();
    let data = d.path().join("r");
    let c = client(&data);
    let gone = HostId::new("routed");
    let entry = c
        .add_host(HostDraft {
            name: "Routed".into(),
            kind: ssh(closed_port()),
            id: Some(gone.clone()),
        })
        .await
        .unwrap();
    let mut fleet = c.fleet(None);
    until(&mut fleet, "the host reconnecting", |e| {
        link_is(e, "routed", |s| matches!(s, LinkState::Reconnecting { .. }))
    })
    .await;

    // Another machine behind the same host: the route (the kind) changes.
    let mut routed = entry.clone();
    routed.kind = ssh(closed_port());
    let removed = Cell::new(false);
    c.update_host(routed).await.unwrap();
    until(&mut fleet, "the host added again", |e| match e {
        FleetEvent::HostRemoved { id } if *id == gone => {
            removed.set(true);
            false
        }
        FleetEvent::HostUpserted { host } => host.id == gone && removed.get(),
        _ => false,
    })
    .await;
    assert!(
        removed.get(),
        "the old route's host was removed from the fleet before the new route added it"
    );
    stop_hub(&c).await;
}
