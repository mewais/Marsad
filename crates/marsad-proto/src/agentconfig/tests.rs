//! Merge table tests for [`effective_config`], with the real Wasit renderers deciding what
//! each driver cannot honour.

use super::*;
use wasit_core::DriverDescriptor;

fn desc(id: &str) -> std::sync::Arc<dyn DriverDescriptor> {
    wasit_runtime::default_registry()
        .get(&DriverId::new(id))
        .unwrap()
}

fn base_item(kind: ConfigItemKind, name: &str, scope: &str) -> ConfigItem {
    ConfigItem {
        kind,
        name: name.into(),
        source: ConfigSource {
            scope: scope.into(),
            path: Some(format!("/cfg/{name}")),
        },
        description: String::new(),
        enabled: true,
        mcp: (kind == ConfigItemKind::McpServer).then(|| McpTransport::Stdio {
            command: "orig".into(),
            args: vec![],
            env: Default::default(),
        }),
        path: Some(format!("/cfg/{name}/SKILL.md")),
    }
}

fn base() -> Vec<ConfigItem> {
    vec![
        base_item(ConfigItemKind::McpServer, "github", "user"),
        base_item(ConfigItemKind::Skill, "review", "user"),
        base_item(ConfigItemKind::Agent, "helper", "project"),
        base_item(ConfigItemKind::Plugin, "demo@m", "user"),
    ]
}

fn ov(kind: ConfigItemKind, name: &str, action: OverrideAction) -> ItemOverride {
    ItemOverride {
        kind,
        name: name.into(),
        driver: None,
        action,
    }
}

fn mcp(cmd: &str) -> ItemValue {
    ItemValue::McpServer {
        server: McpTransport::Stdio {
            command: cmd.into(),
            args: vec![],
            env: Default::default(),
        },
    }
}

fn layer(items: Vec<ItemOverride>) -> OverrideLayer {
    OverrideLayer {
        items,
        ..OverrideLayer::default()
    }
}

fn host() -> LayerScope {
    LayerScope::Host {
        host: HostId::new("box"),
    }
}

fn session() -> LayerScope {
    LayerScope::Session {
        host: HostId::new("box"),
        session: SessionId("abcdefghijkl".into()),
    }
}

fn eff(driver: &str, layers: &[(LayerScope, OverrideLayer)]) -> EffectiveConfig {
    effective_config(
        &DriverId::new(driver),
        &base(),
        layers,
        &[],
        desc(driver).as_ref(),
    )
}

/// (case, layers, expected (kind, name, state, active) for the items under test)
#[test]
fn merge_table() {
    use ConfigItemKind as K;
    use ItemState as S;
    type Case = (
        &'static str,
        Vec<(LayerScope, OverrideLayer)>,
        Vec<(K, &'static str, S, bool)>,
    );
    let cases: Vec<Case> = vec![
        (
            "no layers: everything inherited",
            vec![],
            vec![
                (K::McpServer, "github", S::Inherited, true),
                (K::Skill, "review", S::Inherited, true),
            ],
        ),
        (
            "global disables, host changes another",
            vec![
                (
                    LayerScope::Global,
                    layer(vec![ov(K::Skill, "review", OverrideAction::Disable)]),
                ),
                (
                    host(),
                    layer(vec![ov(
                        K::McpServer,
                        "github",
                        OverrideAction::Set { value: mcp("new") },
                    )]),
                ),
            ],
            vec![
                (K::Skill, "review", S::Disabled, false),
                (K::McpServer, "github", S::Changed, true),
            ],
        ),
        (
            "session resets what global disabled",
            vec![
                (
                    LayerScope::Global,
                    layer(vec![ov(K::Skill, "review", OverrideAction::Disable)]),
                ),
                (
                    session(),
                    layer(vec![ov(K::Skill, "review", OverrideAction::Reset)]),
                ),
            ],
            vec![(K::Skill, "review", S::Inherited, true)],
        ),
        (
            "add, then a narrower layer disables the added item",
            vec![
                (
                    host(),
                    layer(vec![ov(
                        K::McpServer,
                        "extra",
                        OverrideAction::Set { value: mcp("x") },
                    )]),
                ),
                (
                    session(),
                    layer(vec![ov(K::McpServer, "extra", OverrideAction::Disable)]),
                ),
            ],
            vec![(K::McpServer, "extra", S::Disabled, false)],
        ),
        (
            "re-enable a disabled inherited item by setting it",
            vec![
                (
                    LayerScope::Global,
                    layer(vec![ov(K::McpServer, "github", OverrideAction::Disable)]),
                ),
                (
                    session(),
                    layer(vec![ov(
                        K::McpServer,
                        "github",
                        OverrideAction::Set { value: mcp("y") },
                    )]),
                ),
            ],
            vec![(K::McpServer, "github", S::Changed, true)],
        ),
        (
            "reset of an added item removes it",
            vec![
                (
                    host(),
                    layer(vec![ov(
                        K::McpServer,
                        "extra",
                        OverrideAction::Set { value: mcp("x") },
                    )]),
                ),
                (
                    session(),
                    layer(vec![ov(K::McpServer, "extra", OverrideAction::Reset)]),
                ),
            ],
            vec![],
        ),
    ];
    for (name, layers, want) in cases {
        let e = eff("claude", &layers);
        for (kind, item, state, active) in &want {
            let it = e
                .item(*kind, item)
                .unwrap_or_else(|| panic!("{name}: {item} missing"));
            assert_eq!((it.state, it.active), (*state, *active), "{name}: {item}");
        }
        if want.is_empty() {
            assert!(e.item(K::McpServer, "extra").is_none(), "{name}");
        }
    }
}

#[test]
fn provenance_and_rendered_config() {
    use ConfigItemKind as K;
    let layers = vec![
        (
            LayerScope::Global,
            OverrideLayer {
                items: vec![ov(K::Skill, "review", OverrideAction::Disable)],
                extra_instructions: Some("be brief".into()),
                env: vec![("A".into(), "1".into()), ("B".into(), "1".into())],
                disallowed_tools: vec!["WebFetch".into()],
            },
        ),
        (
            host(),
            OverrideLayer {
                items: vec![
                    ov(
                        K::McpServer,
                        "github",
                        OverrideAction::Set { value: mcp("new") },
                    ),
                    ov(
                        K::McpServer,
                        "local",
                        OverrideAction::Set { value: mcp("l") },
                    ),
                ],
                extra_instructions: Some("use tabs".into()),
                env: vec![("A".into(), "2".into())],
                disallowed_tools: vec!["WebFetch".into(), "Bash".into()],
            },
        ),
    ];
    let e = eff("codex", &layers);
    let gh = e.item(K::McpServer, "github").unwrap();
    assert!(matches!(&gh.origin, Provenance::Agent { source } if source.scope == "user"));
    assert_eq!(gh.changed_by, Some(Provenance::Layer { scope: host() }));
    let local = e.item(K::McpServer, "local").unwrap();
    assert_eq!(local.origin, Provenance::Layer { scope: host() });
    assert_eq!(
        e.extra_instructions.as_ref().unwrap().value,
        "be brief\n\nuse tabs"
    );
    let env: Vec<(&str, &str)> = e
        .env
        .iter()
        .map(|(k, v, _)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(env, [("B", "1"), ("A", "2")]);
    assert_eq!(e.disallowed_tools, ["WebFetch", "Bash"]);

    let cfg = e.agent_config();
    let names: Vec<&str> = cfg.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["github", "local"],
        "inherited items are never re-rendered"
    );
    assert_eq!(cfg.disabled.len(), 1);
    assert_eq!(
        cfg.disabled[0].path.as_deref(),
        Some("/cfg/review/SKILL.md")
    );
    // codex cannot deny tools: reported as a non-item unsupported setting.
    assert!(e.unsupported.iter().any(|u| u.item == "tool_policy"));
}

#[test]
fn per_driver_unsupported_reasons_come_from_the_renderer() {
    use ConfigItemKind as K;
    let layers = vec![(
        LayerScope::Global,
        layer(vec![
            ov(K::Agent, "helper", OverrideAction::Disable),
            ov(K::Plugin, "demo@m", OverrideAction::Disable),
        ]),
    )];
    let reason = |driver: &str, kind, name: &str| {
        eff(driver, &layers)
            .item(kind, name)
            .unwrap()
            .unsupported
            .clone()
    };
    // claude: plugins can be switched off, agents cannot.
    assert!(reason("claude", K::Agent, "helper").is_some());
    assert!(reason("claude", K::Plugin, "demo@m").is_none());
    // opencode / kilo: agents can, plugins cannot.
    for d in ["opencode", "kilo"] {
        assert!(reason(d, K::Agent, "helper").is_none(), "{d}");
        assert!(reason(d, K::Plugin, "demo@m").is_some(), "{d}");
    }
    // codex: neither.
    assert!(reason("codex", K::Agent, "helper").is_some());
    assert!(reason("codex", K::Plugin, "demo@m").is_some());
}

#[test]
fn driver_scoped_overrides_ignored_items_and_automatic_items() {
    use ConfigItemKind as K;
    let mut only_codex = ov(K::Skill, "review", OverrideAction::Disable);
    only_codex.driver = Some(DriverId::new("codex"));
    let layers = vec![(
        LayerScope::Global,
        layer(vec![
            only_codex,
            ov(K::Skill, "ghost", OverrideAction::Disable),
            ov(
                K::Skill,
                "review2",
                OverrideAction::Set {
                    value: mcp("wrong kind"),
                },
            ),
            ov(K::McpServer, "wasl", OverrideAction::Disable),
        ]),
    )];
    let automatic = vec![AutomaticItem {
        provider: "orchestration".into(),
        name: "wasl".into(),
        value: mcp("marsad-agent"),
    }];
    let e = effective_config(
        &DriverId::new("claude"),
        &base(),
        &layers,
        &automatic,
        desc("claude").as_ref(),
    );
    assert_eq!(
        e.item(K::Skill, "review").unwrap().state,
        ItemState::Inherited
    );
    let reasons: Vec<&str> = e.ignored.iter().map(|i| i.item.name.as_str()).collect();
    assert_eq!(reasons, ["ghost", "review2", "wasl"]);
    let w = e.item(K::McpServer, "wasl").unwrap();
    assert_eq!(w.state, ItemState::Automatic);
    assert!(w.locked && w.active);
    assert_eq!(
        w.origin,
        Provenance::Automatic {
            provider: "orchestration".into()
        }
    );
    assert!(e
        .agent_config()
        .mcp_servers
        .iter()
        .any(|s| s.name == "wasl"));
}
