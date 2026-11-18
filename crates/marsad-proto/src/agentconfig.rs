//! Agent configuration (ARCHITECTURE §6): what each CLI already has (the base layer, from the
//! CLI itself), Marsad's override layers on top (Global → Host → Workstream node → Session),
//! automatic items appended by Marsad itself, and the pure [`effective_config`] that merges
//! them with per-item provenance and asks the driver's renderer what it cannot honour.
//!
//! Marsad mirrors the CLI and never edits its files: inherited items are not re-rendered (the
//! CLI loads them itself); only changes, additions and disables become launch-time
//! injection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wasit_control::HostId;
use wasit_core::{
    AgentConfig, ConfigItem, ConfigItemKind, ConfigItemRef, ConfigRenderer, ConfigSource,
    DisabledItem, DriverId, McpServerConfig, McpTransport, PluginRef, SkillRef, SpawnSpec,
    Unsupported,
};
use wasit_runtime::wire::{ResumeRequest, SessionId};

// ── layers ──────────────────────────────────────────────────────────────────

/// Where an override layer applies.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum LayerScope {
    Global,
    Host { host: HostId },
    WorkstreamNode { workstream: String, node: String },
    Session { host: HostId, session: SessionId },
}

/// A value for an item an override sets (changes an inherited one, or adds one).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemValue {
    McpServer {
        server: McpTransport,
    },
    /// A skill directory `<root>/<name>/SKILL.md`.
    Skill {
        root: String,
    },
    /// A plugin the CLI can load from a local path.
    Plugin {
        path: String,
    },
}

impl ItemValue {
    pub fn kind(&self) -> ConfigItemKind {
        match self {
            ItemValue::McpServer { .. } => ConfigItemKind::McpServer,
            ItemValue::Skill { .. } => ConfigItemKind::Skill,
            ItemValue::Plugin { .. } => ConfigItemKind::Plugin,
        }
    }
}

/// What an override does to one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OverrideAction {
    /// Switch the item off for launches in this scope.
    Disable,
    /// Change the inherited item, or add it if the CLI has none by that name.
    Set { value: ItemValue },
    /// Undo what earlier (broader) layers did to the item.
    Reset,
}

/// One override of one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ItemOverride {
    pub kind: ConfigItemKind,
    pub name: String,
    /// Only for this driver (`None` = every driver).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<DriverId>,
    #[serde(flatten)]
    pub action: OverrideAction,
}

/// One layer of overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OverrideLayer {
    #[serde(default)]
    pub items: Vec<ItemOverride>,
    /// Appended to the agent's instructions (layers concatenate, broadest first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_instructions: Option<String>,
    /// Environment for the agent process (later layers win per key).
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Tools the agent must not use (union over layers).
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
}

impl OverrideLayer {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
            && self.extra_instructions.is_none()
            && self.env.is_empty()
            && self.disallowed_tools.is_empty()
    }
}

/// An item Marsad itself injects into every launch (e.g. the orchestration MCP server). It is
/// applied after all layers and cannot be overridden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutomaticItem {
    /// Who adds it (shown to the user).
    pub provider: String,
    pub name: String,
    pub value: ItemValue,
}

// ── effective config ────────────────────────────────────────────────────────

/// Where an item (or the last change to it) comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum Provenance {
    /// The CLI's own configuration.
    Agent { source: ConfigSource },
    /// A Marsad override layer.
    Layer { scope: LayerScope },
    /// Injected by Marsad itself.
    Automatic { provider: String },
}

/// The item's effective state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// As the CLI has it (not rendered; the CLI loads it itself).
    Inherited,
    /// Inherited, but an override replaced its definition.
    Changed,
    /// Not in the CLI's config; added by an override.
    Added,
    /// Switched off by an override.
    Disabled,
    /// Injected by Marsad (locked).
    Automatic,
}

/// One item of the effective config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EffectiveItem {
    pub kind: ConfigItemKind,
    pub name: String,
    pub state: ItemState,
    /// The launched session will have the item (the CLI's own `enabled` for inherited items).
    pub active: bool,
    /// Where it comes from originally.
    pub origin: Provenance,
    /// The layer that last changed it, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_by: Option<Provenance>,
    /// The CLI's own definition, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ConfigItem>,
    /// The definition Marsad renders (changed / added / automatic items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<ItemValue>,
    /// Users cannot override it.
    pub locked: bool,
    /// Why the driver cannot honour what this item asks for (the launch does not fail).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported: Option<String>,
}

/// An override that had no effect, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct IgnoredOverride {
    pub scope: LayerScope,
    pub item: ConfigItemRef,
    pub reason: String,
}

/// A non-item setting with the layers that contributed to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Contribution<T> {
    pub value: T,
    pub scopes: Vec<LayerScope>,
}

/// The merged configuration for one launch, with provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EffectiveConfig {
    pub driver: DriverId,
    /// Sorted by kind, then name.
    pub items: Vec<EffectiveItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_instructions: Option<Contribution<String>>,
    /// Final environment, each variable with the layer that set it.
    #[serde(default)]
    pub env: Vec<(String, String, LayerScope)>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    #[serde(default)]
    pub ignored: Vec<IgnoredOverride>,
    /// Unsupported settings that are not items (e.g. `tool_policy`, `extra_instructions`).
    #[serde(default)]
    pub unsupported: Vec<Unsupported>,
}

impl EffectiveConfig {
    /// The launch-time config handed to the runtime: only what differs from what the CLI
    /// loads itself.
    pub fn agent_config(&self) -> AgentConfig {
        let mut cfg = AgentConfig::default();
        for it in &self.items {
            match (it.state, &it.value) {
                (ItemState::Changed | ItemState::Added | ItemState::Automatic, Some(v)) => {
                    match v {
                        ItemValue::McpServer { server } => cfg.mcp_servers.push(McpServerConfig {
                            name: it.name.clone(),
                            transport: server.clone(),
                            // Only Marsad's own automatic servers do their own gating.
                            trusted: it.state == ItemState::Automatic,
                        }),
                        ItemValue::Skill { root } => cfg.skills.push(SkillRef {
                            name: it.name.clone(),
                            root: root.clone(),
                        }),
                        ItemValue::Plugin { path } => cfg.plugins.push(PluginRef {
                            name: it.name.clone(),
                            path: path.clone(),
                        }),
                    }
                }
                (ItemState::Disabled, _) if it.base.is_some() => cfg.disabled.push(DisabledItem {
                    kind: it.kind,
                    name: it.name.clone(),
                    path: it.base.as_ref().and_then(|b| b.path.clone()),
                }),
                _ => {}
            }
        }
        cfg.extra_instructions = self.extra_instructions.as_ref().map(|c| c.value.clone());
        cfg.env = self
            .env
            .iter()
            .map(|(k, v, _)| (k.clone(), v.clone()))
            .collect();
        cfg.tool_policy.disallowed_tools = self.disallowed_tools.clone();
        cfg
    }

    pub fn item(&self, kind: ConfigItemKind, name: &str) -> Option<&EffectiveItem> {
        self.items.iter().find(|i| i.kind == kind && i.name == name)
    }
}

/// Merge `base` (what the CLI has), `layers` (broadest first) and `automatic` items into the
/// effective config for `driver`, then ask `renderer` (the driver's) which items it cannot
/// honour. Pure: no I/O; the renderer only fills a scratch [`SpawnSpec`].
pub fn effective_config(
    driver: &DriverId,
    base: &[ConfigItem],
    layers: &[(LayerScope, OverrideLayer)],
    automatic: &[AutomaticItem],
    renderer: &dyn ConfigRenderer,
) -> EffectiveConfig {
    let mut items: BTreeMap<(ConfigItemKind, String), EffectiveItem> = BTreeMap::new();
    let inherited = |b: &ConfigItem| EffectiveItem {
        kind: b.kind,
        name: b.name.clone(),
        state: ItemState::Inherited,
        active: b.enabled,
        origin: Provenance::Agent {
            source: b.source.clone(),
        },
        changed_by: None,
        base: Some(b.clone()),
        value: None,
        locked: false,
        unsupported: None,
    };
    for b in base {
        // A CLI may list a name twice (e.g. user and project scope); the first one wins, as
        // the probes list the narrower scopes last only where the CLI does the same.
        items
            .entry((b.kind, b.name.clone()))
            .or_insert_with(|| inherited(b));
    }
    let mut ignored = Vec::new();
    let mut instructions: Option<Contribution<String>> = None;
    let mut env: Vec<(String, String, LayerScope)> = Vec::new();
    let mut tools: Vec<String> = Vec::new();

    for (scope, layer) in layers {
        let by = Provenance::Layer {
            scope: scope.clone(),
        };
        for ov in &layer.items {
            if ov.driver.as_ref().is_some_and(|d| d != driver) {
                continue;
            }
            let key = (ov.kind, ov.name.clone());
            let ignore = |reason: &str| IgnoredOverride {
                scope: scope.clone(),
                item: ConfigItemRef::new(ov.kind, ov.name.clone()),
                reason: reason.to_string(),
            };
            match &ov.action {
                OverrideAction::Disable => match items.get_mut(&key) {
                    Some(it) => {
                        it.state = ItemState::Disabled;
                        it.active = false;
                        it.changed_by = Some(by.clone());
                    }
                    None => ignored.push(ignore("there is no such item to disable")),
                },
                OverrideAction::Reset => {
                    match base.iter().find(|b| b.kind == ov.kind && b.name == ov.name) {
                        Some(b) => {
                            items.insert(key, inherited(b));
                        }
                        None => {
                            items.remove(&key);
                        }
                    }
                }
                OverrideAction::Set { value } => {
                    if value.kind() != ov.kind {
                        ignored.push(ignore("the value is for a different kind of item"));
                        continue;
                    }
                    let b = base.iter().find(|b| b.kind == ov.kind && b.name == ov.name);
                    items.insert(
                        key,
                        EffectiveItem {
                            kind: ov.kind,
                            name: ov.name.clone(),
                            state: if b.is_some() {
                                ItemState::Changed
                            } else {
                                ItemState::Added
                            },
                            active: true,
                            origin: match b {
                                Some(b) => Provenance::Agent {
                                    source: b.source.clone(),
                                },
                                None => by.clone(),
                            },
                            changed_by: Some(by.clone()),
                            base: b.cloned(),
                            value: Some(value.clone()),
                            locked: false,
                            unsupported: None,
                        },
                    );
                }
            }
        }
        if let Some(text) = layer
            .extra_instructions
            .as_deref()
            .filter(|t| !t.is_empty())
        {
            let c = instructions.get_or_insert_with(|| Contribution {
                value: String::new(),
                scopes: Vec::new(),
            });
            if !c.value.is_empty() {
                c.value.push_str("\n\n");
            }
            c.value.push_str(text);
            c.scopes.push(scope.clone());
        }
        for (k, v) in &layer.env {
            env.retain(|(ek, _, _)| ek != k);
            env.push((k.clone(), v.clone(), scope.clone()));
        }
        for t in &layer.disallowed_tools {
            if !tools.contains(t) {
                tools.push(t.clone());
            }
        }
    }

    for a in automatic {
        let kind = a.value.kind();
        let b = base.iter().find(|b| b.kind == kind && b.name == a.name);
        items.insert(
            (kind, a.name.clone()),
            EffectiveItem {
                kind,
                name: a.name.clone(),
                state: ItemState::Automatic,
                active: true,
                origin: Provenance::Automatic {
                    provider: a.provider.clone(),
                },
                changed_by: None,
                base: b.cloned(),
                value: Some(a.value.clone()),
                locked: true,
                unsupported: None,
            },
        );
    }

    let mut eff = EffectiveConfig {
        driver: driver.clone(),
        items: items.into_values().collect(),
        extra_instructions: instructions,
        env,
        disallowed_tools: tools,
        ignored,
        unsupported: Vec::new(),
    };
    // Ask the driver what it cannot honour. The session directory is a placeholder: renderers
    // only reference it in generated paths.
    let mut spec = SpawnSpec {
        session_dir: Some("/marsad/session".into()),
        ..SpawnSpec::default()
    };
    for u in renderer.render_config(&eff.agent_config(), &mut spec) {
        match eff
            .items
            .iter_mut()
            .find(|i| ConfigItemRef::new(i.kind, i.name.clone()).key() == u.item)
        {
            Some(it) => it.unsupported = Some(u.reason),
            None => eff.unsupported.push(u),
        }
    }
    eff
}

// ── requests ────────────────────────────────────────────────────────────────

/// `agentconfig.base`: what the CLI itself has configured on the daemon's host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BaseRequest {
    pub driver: DriverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Re-run the CLI's probes instead of using the cached result.
    #[serde(default)]
    pub refresh: bool,
}

/// Result of `agentconfig.base`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BaseConfig {
    pub driver: DriverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub items: Vec<ConfigItem>,
    /// Probes were still running when the bounded wait ended (items may be incomplete).
    pub refreshing: bool,
    /// Probe failures (each also leaves its items out).
    #[serde(default)]
    pub errors: Vec<String>,
    pub refreshed_at_ms: u64,
}

/// A node of a workstream (its layer applies between Host and Session).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorkstreamNodeRef {
    pub workstream: String,
    pub node: String,
}

/// `agentconfig.effective` (hub): preview the config a launch would get.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EffectiveRequest {
    /// Target host (`None` = the hub's own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostId>,
    pub driver: DriverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream: Option<WorkstreamNodeRef>,
    /// The stored session layer of an existing session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// An unsaved session layer (applied last among layers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<OverrideLayer>,
}

/// `sessions.launch` (hub): create a session with the effective config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LaunchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostId>,
    pub driver: DriverId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_prompt: Option<String>,
    #[serde(default)]
    pub resume: ResumeRequest,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workstream: Option<WorkstreamNodeRef>,
    /// The session's own layer; stored under `LayerScope::Session` once created.
    #[serde(default)]
    pub overrides: OverrideLayer,
}

impl LaunchRequest {
    pub fn new(driver: impl Into<DriverId>) -> Self {
        LaunchRequest {
            host: None,
            driver: driver.into(),
            cwd: None,
            model: None,
            mode: None,
            first_prompt: None,
            resume: ResumeRequest::New,
            labels: BTreeMap::new(),
            workstream: None,
            overrides: OverrideLayer::default(),
        }
    }
}

/// Result of `sessions.launch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LaunchReply {
    pub host: HostId,
    pub id: SessionId,
    pub effective: EffectiveConfig,
    /// What the runtime's renderer reported at launch (the same reasons as in `effective`).
    #[serde(default)]
    pub unsupported: Vec<Unsupported>,
}

#[cfg(test)]
mod tests;
