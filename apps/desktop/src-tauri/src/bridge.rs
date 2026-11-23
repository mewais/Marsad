//! The generic bridges the webview uses (see `apps/desktop/src/bridge/tauri.ts`):
//!
//! * `rpc(method, params, host?)` → result, or a typed error `{code, message, data?}` (the
//!   daemon's `RpcError`; `data` is the typed `MarsadError` when there is one). Methods under
//!   `desktop.` are answered natively (open a link/path, reveal, pick a folder); everything
//!   else goes to the local daemon (the hub), which routes by `host`.
//! * `rpc_subscribe(topic, host?, from?, channel)` → subscription id. Messages on the
//!   channel are [`SubMessage`]s: every event with its cursor, and the daemon link's state
//!   whenever it changes. The subscription is durable: after a reconnect or a lagged close it
//!   resumes from its last cursor natively, so the webview sees one continuous stream.
//! * `rpc_unsubscribe(id)`.
//! * `connection_state()` → [`ConnectionState`] (also pushed as the `connection` event).
//! * `reconnect_daemon()`: connect to the local daemon again now.
//!
//! Every command is async and holds no lock across an await (the subscription table is a
//! std mutex taken only for inserts/removals).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use marsad_client::{ClientError, MarsadClient};
use marsad_proto::{HostId, MarsadError, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tokio::sync::watch;
use wasit_control::{CallError, Cursor, LinkState};

use crate::connection::ConnectionState;

/// A failed `rpc` (what the webview's promise rejects with).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl BridgeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        BridgeError {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }
}

impl From<RpcError> for BridgeError {
    fn from(e: RpcError) -> Self {
        BridgeError {
            code: e.code,
            message: e.message,
            data: e.data,
        }
    }
}

impl From<ClientError> for BridgeError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Marsad(m) => m.into_rpc().into(),
            ClientError::Call(CallError::Rpc(r)) => r.into(),
            ClientError::Call(other) => {
                let code = match &other {
                    CallError::Timeout => "timeout",
                    CallError::NotConnected | CallError::ConnectionLost(_) => "not_connected",
                    _ => "transport",
                };
                BridgeError::new(code, other.to_string())
            }
        }
    }
}

impl From<MarsadError> for BridgeError {
    fn from(e: MarsadError) -> Self {
        e.into_rpc().into()
    }
}

/// One message on a subscription's channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubMessage {
    Event {
        cursor: String,
        payload: Value,
    },
    /// The link to the daemon changed (`connected`, `connecting`, `reconnecting`,
    /// `offline`); events resume from the last cursor once it is `connected` again.
    Link {
        state: &'static str,
    },
    /// The subscription ended for good (the daemon refused the topic, or it ended).
    End {
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<BridgeError>,
    },
}

/// The link state as the webview sees it.
pub fn link_word(s: &LinkState) -> &'static str {
    match s {
        LinkState::Connected { .. } => "connected",
        LinkState::Connecting | LinkState::Provisioning { .. } => "connecting",
        LinkState::Reconnecting { .. } => "reconnecting",
        _ => "offline",
    }
}

/// State shared by the commands.
pub struct Shell {
    pub client: MarsadClient,
    pub app: AppHandle,
    subs: Mutex<HashMap<u64, tokio::task::AbortHandle>>,
    next: AtomicU64,
    pub connection: watch::Sender<ConnectionState>,
}

impl Shell {
    pub fn new(client: MarsadClient, app: AppHandle) -> Self {
        Shell {
            client,
            app,
            subs: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            connection: watch::channel(ConnectionState::default()).0,
        }
    }

    fn subs(&self) -> std::sync::MutexGuard<'_, HashMap<u64, tokio::task::AbortHandle>> {
        // Inserts and removals only; a poisoned table is still consistent.
        self.subs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn host(h: Option<String>) -> Option<HostId> {
    h.filter(|s| !s.is_empty()).map(HostId::new)
}

/// A resume cursor given as a string or a number.
fn cursor(from: Option<Value>) -> Option<Cursor> {
    match from? {
        Value::String(s) if !s.is_empty() => Some(Cursor::new(s)),
        Value::Number(n) => Some(Cursor::new(n.to_string())),
        _ => None,
    }
}

#[tauri::command]
pub async fn rpc(
    shell: State<'_, Shell>,
    method: String,
    params: Option<Value>,
    host: Option<String>,
) -> Result<Value, BridgeError> {
    let params = params.unwrap_or(Value::Null);
    if let Some(m) = method.strip_prefix("desktop.") {
        return crate::native::call(&shell.app, m, params).await;
    }
    let host = host_arg(host);
    Ok(shell
        .client
        .call_raw(host.as_ref(), &method, params)
        .await?)
}

fn host_arg(h: Option<String>) -> Option<HostId> {
    host(h)
}

#[tauri::command]
pub async fn rpc_subscribe(
    shell: State<'_, Shell>,
    topic: String,
    host: Option<String>,
    from: Option<Value>,
    channel: Channel<SubMessage>,
) -> Result<u64, BridgeError> {
    let id = shell.next.fetch_add(1, Ordering::Relaxed);
    let mut sub = shell
        .client
        .subscribe_raw(host_arg(host).as_ref(), topic.clone(), cursor(from));
    let mut link = shell.client.link().state();
    let task = tokio::spawn(async move {
        let mut last = link_word(&link.borrow_and_update());
        if channel.send(SubMessage::Link { state: last }).is_err() {
            return;
        }
        loop {
            let msg = tokio::select! {
                item = sub.next() => match item {
                    Some(Ok((c, payload))) => SubMessage::Event { cursor: c.0, payload },
                    Some(Err(e)) => SubMessage::End { error: Some(e.into()) },
                    None => SubMessage::End { error: None },
                },
                changed = link.changed() => {
                    if changed.is_err() {
                        SubMessage::End { error: Some(BridgeError::new("not_connected", "the client is gone")) }
                    } else {
                        let now = link_word(&link.borrow_and_update());
                        if now == last {
                            continue;
                        }
                        last = now;
                        SubMessage::Link { state: now }
                    }
                }
            };
            let end = matches!(msg, SubMessage::End { .. });
            if channel.send(msg).is_err() {
                // The webview dropped the channel (page reload).
                tracing::debug!(topic, "subscription channel closed");
                return;
            }
            if end {
                return;
            }
        }
    });
    shell.subs().insert(id, task.abort_handle());
    Ok(id)
}

#[tauri::command]
pub async fn rpc_unsubscribe(shell: State<'_, Shell>, id: u64) -> Result<(), BridgeError> {
    match shell.subs().remove(&id) {
        Some(h) => {
            h.abort();
            Ok(())
        }
        None => Err(BridgeError::new(
            "not_found",
            format!("no subscription {id}"),
        )),
    }
}

#[tauri::command]
pub async fn connection_state(shell: State<'_, Shell>) -> Result<ConnectionState, BridgeError> {
    Ok(shell.connection.borrow().clone())
}

/// Connect to the local daemon again now: skips the backoff and the parked state of a link
/// that gave up (the app's "Try again" when the daemon could not be started or reached).
#[tauri::command]
pub async fn reconnect_daemon(shell: State<'_, Shell>) -> Result<(), BridgeError> {
    shell.client.link().reconnect_now();
    Ok(())
}

/// Arguments of `desktop.open` / `desktop.reveal`.
#[derive(Debug, Deserialize)]
pub struct OpenArgs {
    pub target: String,
    /// The target is a path on this WSL distro (translated to its Windows form first).
    #[serde(default)]
    pub distro: Option<String>,
}

/// Arguments of `desktop.pick_folder`.
#[derive(Debug, Deserialize, Default)]
pub struct PickArgs {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub default_path: Option<String>,
    /// Browse a WSL distro: start in `\\wsl.localhost\<distro>\` (or the translated
    /// `default_path`) and answer with the Linux path.
    #[serde(default)]
    pub distro: Option<String>,
}

/// The result of `desktop.pick_folder`.
pub fn picked(path: Option<String>) -> Value {
    json!({ "path": path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_and_errors() {
        assert_eq!(cursor(Some(json!("a.3"))), Some(Cursor::new("a.3")));
        assert_eq!(cursor(Some(json!(65536))), Some(Cursor::new("65536")));
        assert_eq!(cursor(Some(json!(null))), None);
        let e: BridgeError = ClientError::Marsad(MarsadError::NotHub).into();
        assert_eq!(e.code, "not_hub");
        assert_eq!(e.data.unwrap()["marsad_error"], "not_hub");
        let e: BridgeError = ClientError::Call(CallError::Timeout).into();
        assert_eq!(e.code, "timeout");
        assert_eq!(link_word(&LinkState::Connecting), "connecting");
        assert_eq!(host(Some(String::new())), None);
    }
}
