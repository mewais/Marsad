//! Debug builds only: an end-to-end smoke check through the real path (webview → `rpc` /
//! `rpc_subscribe` → shell → daemon → events back on the channel).
//!
//! With `MARSAD_SMOKE=<result file>` (plus `MARSAD_SMOKE_SCENARIO=<fake CLI scenario>` and
//! `MARSAD_SMOKE_CWD=<dir>`), once the page has loaded the shell evaluates [`SCRIPT`] in the
//! webview. The script waits for the UI to render, subscribes to `fleet`, launches a
//! session (`sessions.launch`, driver `claude` = the fake CLI), sends a prompt, answers the
//! gate it raises and waits for the turn to complete — all through the same bridge the UI
//! uses — then calls `smoke_report`, which writes the report and exits the app (the session
//! is left in place so a screenshot shows it).

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tauri::{AppHandle, Webview};

use crate::bridge::BridgeError;

pub const ENV: &str = "MARSAD_SMOKE";

const SCRIPT: &str = r#"
(async () => {
  const T = window.__TAURI__ && window.__TAURI__.core;
  const steps = [];
  const step = (s) => steps.push([Math.round(performance.now()), s]);
  const finish = (ok, error) => T.invoke('smoke_report', { report: {
    ok, error: error ? String(error && error.message || JSON.stringify(error)) : null, steps,
    title: document.title,
    rendered: (document.getElementById('root') || document.body).innerText.slice(0, 400),
  }});
  try {
    if (!T) throw new Error('no __TAURI__ global');
    const within = (ms, what, p) => Promise.race([p, new Promise((_, rej) =>
      setTimeout(() => rej(new Error('timeout: ' + what)), ms))]);
    // 1. The UI rendered.
    await within(30000, 'ui rendered', new Promise((res) => {
      const root = document.getElementById('root');
      const ok = () => root && root.children.length > 0;
      if (ok()) return res();
      new MutationObserver((_, o) => { if (ok()) { o.disconnect(); res(); } })
        .observe(document.body, { childList: true, subtree: true });
    }));
    step('ui rendered');
    const conn = await T.invoke('connection_state');
    step('connection_state ' + conn.daemon);
    // 2. Typed errors come back as rejections.
    try { await T.invoke('rpc', { method: 'no.such.method', params: null, host: null }); throw new Error('no error'); }
    catch (e) { if (!e || e.code !== 'method_not_found') throw new Error('untyped error: ' + JSON.stringify(e)); }
    step('typed error ' + 'method_not_found');
    // 3. The fleet subscription.
    const sessions = new Map();
    let links = [];
    const waiters = [];
    const check = () => { for (let i = waiters.length - 1; i >= 0; i--) {
      const r = waiters[i].pred(); if (r) { waiters[i].res(r); waiters.splice(i, 1); } } };
    const waitFor = (what, pred) => within(60000, what, new Promise((res) => { waiters.push({ pred, res }); check(); }));
    const ch = new T.Channel();
    ch.onmessage = (m) => {
      if (m.type === 'link') links.push(m.state);
      if (m.type !== 'event') return;
      const e = m.payload;
      if (e.kind === 'reset') { sessions.clear(); for (const h of e.hosts) for (const s of h.sessions) sessions.set(s.id, s); }
      if (e.kind === 'sessions') for (const s of e.sessions) sessions.set(s.id, s);
      if (e.kind === 'session_upserted') sessions.set(e.session.id, e.session);
      if (e.kind === 'session_removed') sessions.delete(e.id);
      check();
    };
    const sub = await T.invoke('rpc_subscribe', { topic: 'fleet', host: null, from: null, channel: ch });
    await waitFor('fleet reset', () => links.includes('connected'));
    step('fleet subscribed ' + sub);
    // 4. Launch a session through the hub (the fake CLI plays the scenario).
    const launched = await T.invoke('rpc', { method: 'sessions.launch', host: null, params: {
      driver: 'claude', cwd: __CWD__, overrides: { env: [['WASIT_FAKE_SCENARIO', __SCENARIO__]] } } });
    const id = launched.id;
    step('launched ' + id);
    await waitFor('session idle in the fleet', () => { const s = sessions.get(id); return s && s.phase.state === 'idle'; });
    step('session idle (fleet)');
    // 5. Prompt → gate → answer → turn completed.
    await T.invoke('rpc', { method: 'session.command', host: null, params: { id, command: { command: 'prompt', text: 'go' } } });
    const gate = await waitFor('gate in the fleet', () => { const s = sessions.get(id); return s && s.pending_gates.length > 0 && s.pending_gates[0].gate; });
    step('gate ' + gate.gate_type + ' ' + gate.request_id);
    await T.invoke('rpc', { method: 'session.command', host: null, params: { id, command: {
      command: 'answer_gate', request_id: gate.request_id, decision: { action: 'approve' } } } });
    step('gate answered');
    await waitFor('turn completed', () => { const s = sessions.get(id); return s && s.turns_completed >= 1 && s.pending_gates.length === 0 && s.phase.state === 'idle'; });
    step('turn completed: ' + (sessions.get(id).last_turn && sessions.get(id).last_turn.reply));
    await T.invoke('rpc_unsubscribe', { id: sub });
    step('unsubscribed');
    await finish(true, null);
  } catch (e) {
    await finish(false, e);
  }
})();
"#;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Start the smoke script once, after the first page load (debug builds with `MARSAD_SMOKE`).
pub fn on_page_load(webview: &Webview) {
    if !cfg!(debug_assertions) || std::env::var_os(ENV).is_none() {
        return;
    }
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let q =
        |k: &str| serde_json::to_string(&std::env::var(k).unwrap_or_default()).unwrap_or_default();
    let script = SCRIPT
        .replace("__CWD__", &q("MARSAD_SMOKE_CWD"))
        .replace("__SCENARIO__", &q("MARSAD_SMOKE_SCENARIO"));
    if let Err(e) = webview.eval(&script) {
        tracing::error!(error = %e, "cannot start the smoke script");
    }
}

/// Write the smoke report and exit (debug builds with `MARSAD_SMOKE` only).
#[tauri::command]
pub async fn smoke_report(app: AppHandle, report: Value) -> Result<(), BridgeError> {
    let Some(path) = std::env::var_os(ENV).filter(|_| cfg!(debug_assertions)) else {
        return Err(BridgeError::new(
            "method_not_found",
            "smoke_report exists only in debug builds run with MARSAD_SMOKE",
        ));
    };
    let text = serde_json::to_string_pretty(&report)
        .map_err(|e| BridgeError::new("invalid_params", e.to_string()))?;
    tokio::fs::write(&path, text)
        .await
        .map_err(|e| BridgeError::new("io", e.to_string()))?;
    tracing::info!(path = %std::path::Path::new(&path).display(), "smoke report written");
    if std::env::var_os("MARSAD_SMOKE_KEEP_OPEN").is_none() {
        app.exit(0);
    }
    Ok(())
}
