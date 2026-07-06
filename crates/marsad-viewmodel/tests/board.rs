//! board.rs — tests for the B11 additive `board_view` extension.
//!
//! These tests WILL fail to compile until the code-agent implements `board_view`,
//! `BoardView`, `BoardSummary`, `HostChip`, `LinkChip`, `WorkstreamCard`, `HostLink`,
//! and `LinkStatus` in `marsad-viewmodel`. That is the expected tests-first signal.
//!
//! Spec: docs/specs/viewmodel-ext.md — §Board, test plan cases BV1-BV7.

use marsad_hosts::{Host, HostStatus};
use marsad_viewmodel::{board_view, HostLink, LinkStatus};
use marsad_workstream::{ConnKind, Connection, Driver, Handoff, Node, Phase, Workstream};

// ── helpers ──────────────────────────────────────────────────────────────────

fn ws(id: &str, name: &str, goal: &str, profile: Option<&str>) -> Workstream {
    Workstream {
        id: id.to_string(),
        name: name.to_string(),
        goal: goal.to_string(),
        profile: profile.map(|p| p.to_string()),
    }
}

fn node(id: &str, ws: &str, driver: Driver, host: &str, state: Phase) -> Node {
    Node {
        id: id.to_string(),
        ws: ws.to_string(),
        name: format!("node-{id}"),
        driver,
        host: host.to_string(),
        state,
        lead: false,
        branch: "main".to_string(),
        context_pct: 0,
        status: "ok".to_string(),
    }
}

fn conn(id: &str, ws: &str, from: &str, to: &str, kind: ConnKind) -> Connection {
    Connection {
        id: id.to_string(),
        ws: ws.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        kind,
        bi: false,
        gate: None,
    }
}

fn host(id: &str, label: &str, target: &str, status: HostStatus, is_local: bool) -> Host {
    Host {
        id: id.to_string(),
        label: label.to_string(),
        target: target.to_string(),
        status,
        session_count: 0,
        is_local,
        removable: !is_local,
        claude_bin: None,
        setup: None,
        env: Vec::new(),
        agent_commands: Vec::new(),
    }
}

fn link(a: &str, b: &str, status: LinkStatus, note: Option<&str>) -> HostLink {
    HostLink {
        a: a.to_string(),
        b: b.to_string(),
        status,
        note: note.map(|n| n.to_string()),
    }
}

// ── BV1: summary counts ──────────────────────────────────────────────────────

#[test]
fn bv1_summary_counts_multi_ws_multi_node_multi_host() {
    let workstreams = vec![
        ws("w1", "Workstream One", "goal one", None),
        ws("w2", "Workstream Two", "goal two", None),
    ];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1", Phase::Working),
        node("n2", "w1", Driver::Codex, "h2", Phase::Done),
        node("n3", "w2", Driver::Gemini, "h1", Phase::Needs),
    ];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![
        host("h1", "Host One", "user@host1", HostStatus::Connected, false),
        host(
            "h2",
            "Host Two",
            "this machine",
            HostStatus::Connected,
            true,
        ),
    ];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);

    assert_eq!(board.summary.workstreams, 2);
    assert_eq!(board.summary.sessions, 3);
    assert_eq!(board.summary.hosts, 2);
}

// ── BV2: profile filter ──────────────────────────────────────────────────────

#[test]
fn bv2_profile_filter_some_includes_only_that_profile() {
    let workstreams = vec![
        ws("w1", "One", "g1", Some("alpha")),
        ws("w2", "Two", "g2", Some("beta")),
        ws("w3", "Three", "g3", Some("alpha")),
    ];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1", Phase::Working),
        node("n2", "w2", Driver::Claude, "h1", Phase::Working),
        node("n3", "w3", Driver::Claude, "h1", Phase::Working),
    ];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h1",
        "Host One",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(
        &workstreams,
        &nodes,
        &connections,
        &hosts,
        &links,
        Some("alpha"),
    );

    assert_eq!(board.summary.workstreams, 2);
    let ids: Vec<&str> = board.all.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["w1", "w3"],
        "only alpha-profile workstreams, input order preserved"
    );
}

#[test]
fn bv2_profile_filter_none_includes_all() {
    let workstreams = vec![
        ws("w1", "One", "g1", Some("alpha")),
        ws("w2", "Two", "g2", Some("beta")),
        ws("w3", "Three", "g3", None),
    ];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1", Phase::Working),
        node("n2", "w2", Driver::Claude, "h1", Phase::Working),
        node("n3", "w3", Driver::Claude, "h1", Phase::Working),
    ];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h1",
        "Host One",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);

    assert_eq!(board.summary.workstreams, 3);
    let ids: Vec<&str> = board.all.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["w1", "w2", "w3"]);
}

// ── BV3: card derivations for a mixed ws ─────────────────────────────────────

#[test]
fn bv3_card_derivations_for_mixed_workstream() {
    let workstreams = vec![ws("w1", "Mixed", "goal", None)];
    let nodes = vec![
        node("n1", "w1", Driver::Gemini, "host-b", Phase::Working),
        node("n2", "w1", Driver::Claude, "host-a", Phase::Needs),
        node("n3", "w1", Driver::Gemini, "host-b", Phase::Done),
        node("n4", "w1", Driver::Codex, "host-c", Phase::Done),
    ];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![
        host(
            "host-a",
            "Host A",
            "this machine",
            HostStatus::Connected,
            true,
        ),
        host("host-b", "Host B", "user@b", HostStatus::Connected, false),
        host("host-c", "Host C", "user@c", HostStatus::Connected, false),
    ];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);

    assert_eq!(board.all.len(), 1);
    let card = &board.all[0];
    assert_eq!(card.id, "w1");
    assert_eq!(card.name, "Mixed");
    assert_eq!(card.goal, "goal");
    // head_state: Needs beats Working/Done
    assert_eq!(card.head_state, Some(marsad_workstream::HeadState::Needs));
    // driver_dots: first-appearance dedup order [Gemini, Claude, Codex]
    assert_eq!(
        card.driver_dots,
        vec![Driver::Gemini, Driver::Claude, Driver::Codex]
    );
    // host_chips: first-appearance dedup order [host-b, host-a, host-c]
    assert_eq!(
        card.host_chips,
        vec![
            "host-b".to_string(),
            "host-a".to_string(),
            "host-c".to_string(),
        ]
    );
    assert_eq!(card.session_count, 4);
    assert_eq!(
        card.counts,
        marsad_workstream::StateCounts {
            working: 1,
            needs: 1,
            error: 0,
            done: 2,
        }
    );
    assert!(!card.is_single_node);
}

// ── BV4: needs_attention truth table ─────────────────────────────────────────

#[test]
fn bv4_needs_attention_true_when_head_state_needs() {
    let workstreams = vec![ws("w1", "W1", "g", None)];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1", Phase::Needs)];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h1",
        "H1",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    assert!(board.all[0].needs_attention);
    assert_eq!(board.attention.len(), 1);
    assert_eq!(board.attention[0].id, "w1");
}

#[test]
fn bv4_needs_attention_true_when_head_state_error() {
    let workstreams = vec![ws("w1", "W1", "g", None)];
    let nodes = vec![node("n1", "w1", Driver::Claude, "h1", Phase::Error)];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h1",
        "H1",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    assert!(board.all[0].needs_attention);
    assert_eq!(board.attention.len(), 1);
    assert_eq!(board.attention[0].id, "w1");
}

#[test]
fn bv4_needs_attention_true_when_gated_connection_even_if_working_or_done() {
    let workstreams = vec![ws("w1", "W1", "g", None)];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1", Phase::Working),
        node("n2", "w1", Driver::Claude, "h1", Phase::Done),
    ];
    let mut c = conn("c1", "w1", "n1", "n2", ConnKind::Data);
    c.gate = Some(Handoff {
        payload: "design doc".to_string(),
        risk: "medium".to_string(),
    });
    let connections = vec![c];
    let hosts = vec![host(
        "h1",
        "H1",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    assert!(board.all[0].needs_attention);
    assert_eq!(board.attention.len(), 1);
    assert_eq!(board.attention[0].id, "w1");
}

#[test]
fn bv4_needs_attention_false_otherwise() {
    let workstreams = vec![ws("w1", "W1", "g", None), ws("w2", "W2", "g2", None)];
    let nodes = vec![
        node("n1", "w1", Driver::Claude, "h1", Phase::Working),
        node("n2", "w1", Driver::Claude, "h1", Phase::Done),
        node("n3", "w2", Driver::Claude, "h1", Phase::Done),
    ];
    let connections = vec![conn("c1", "w1", "n1", "n2", ConnKind::Data)];
    let hosts = vec![host(
        "h1",
        "H1",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    assert!(!board.all[0].needs_attention);
    assert!(!board.all[1].needs_attention);
    assert!(board.attention.is_empty(), "no ws should need attention");
}

// ── BV5: HostChip kind mapping + status string ───────────────────────────────

#[test]
fn bv5_host_chip_kind_local() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h1",
        "Local Host",
        "this machine",
        HostStatus::Connected,
        true,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    assert_eq!(board.host_chips.len(), 1);
    let chip = &board.host_chips[0];
    assert_eq!(chip.id, "h1");
    assert_eq!(chip.label, "Local Host");
    assert_eq!(chip.kind, "local");
    assert_eq!(chip.status, "connected");
}

#[test]
fn bv5_host_chip_kind_wsl() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h2",
        "WSL Host",
        "wsl:Ubuntu",
        HostStatus::Connecting,
        false,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    let chip = &board.host_chips[0];
    assert_eq!(chip.kind, "wsl");
    assert_eq!(chip.status, "connecting");
}

#[test]
fn bv5_host_chip_kind_remote() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h3",
        "Remote Host",
        "user@remote.example.com",
        HostStatus::Reconnecting,
        false,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    let chip = &board.host_chips[0];
    assert_eq!(chip.kind, "remote");
    assert_eq!(chip.status, "reconnecting");
}

#[test]
fn bv5_host_chip_status_unreachable() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![host(
        "h4",
        "Down Host",
        "user@down.example.com",
        HostStatus::Unreachable,
        false,
    )];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    let chip = &board.host_chips[0];
    assert_eq!(chip.kind, "remote");
    assert_eq!(chip.status, "unreachable");
}

#[test]
fn bv5_host_chips_in_input_order_one_per_host() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts = vec![
        host("h1", "Local", "this machine", HostStatus::Connected, true),
        host("h2", "Remote", "user@r", HostStatus::Connected, false),
        host("h3", "Wsl", "wsl:Debian", HostStatus::Connected, false),
    ];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);
    let ids: Vec<&str> = board.host_chips.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["h1", "h2", "h3"],
        "one chip per host, input order"
    );
}

// ── BV6: LinkChip omission of Ok links ───────────────────────────────────────

#[test]
fn bv6_link_chip_ok_omitted_degraded_and_down_emitted() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts: Vec<Host> = vec![];
    let links = vec![
        link("h1", "h2", LinkStatus::Ok, None),
        link("h2", "h3", LinkStatus::Degraded, Some("slow ping")),
        link("h3", "h4", LinkStatus::Down, None),
    ];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);

    assert_eq!(
        board.link_chips.len(),
        2,
        "only the two non-Ok links should be emitted"
    );
    assert_eq!(board.link_chips[0].a, "h2");
    assert_eq!(board.link_chips[0].b, "h3");
    assert_eq!(board.link_chips[0].status, "degraded");
    assert_eq!(board.link_chips[0].note, Some("slow ping".to_string()));

    assert_eq!(board.link_chips[1].a, "h3");
    assert_eq!(board.link_chips[1].b, "h4");
    assert_eq!(board.link_chips[1].status, "down");
    assert_eq!(board.link_chips[1].note, None);
}

// ── BV7: empty inputs → empty board ──────────────────────────────────────────

#[test]
fn bv7_empty_inputs_produce_empty_board_no_panic() {
    let workstreams: Vec<Workstream> = vec![];
    let nodes: Vec<Node> = vec![];
    let connections: Vec<Connection> = vec![];
    let hosts: Vec<Host> = vec![];
    let links: Vec<HostLink> = vec![];

    let board = board_view(&workstreams, &nodes, &connections, &hosts, &links, None);

    assert_eq!(board.summary.workstreams, 0);
    assert_eq!(board.summary.sessions, 0);
    assert_eq!(board.summary.hosts, 0);
    assert!(board.host_chips.is_empty());
    assert!(board.link_chips.is_empty());
    assert!(board.attention.is_empty());
    assert!(board.all.is_empty());
}
