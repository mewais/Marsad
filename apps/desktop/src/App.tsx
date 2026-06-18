// Marsad desktop UI root.
//
// TODO(D02 code-agent): implement the full UI faithful to
// _extracted/design_handoff_marsad/Marsad.dc.html — Overview, Session detail
// (Activity/Changes/Config/Usage/Terminal), Gate overlay, New-session, Settings
// (Notifications/Hosts/Defaults/General), Workspaces, theme + glass. Wire data to
// marsad-core via the Tauri bridge (see ../src-tauri and the `bridge` module).
//
// This placeholder renders the wordmark so the app boots during scaffolding.
export function App() {
  return (
    <div style={{ fontFamily: "'Hanken Grotesk', system-ui, sans-serif" }}>
      <h1>
        Marsad <span style={{ color: "#5f6979" }}>—</span> مرصد
      </h1>
      <p>Scaffolding. UI not yet implemented — see apps/desktop/AGENTS.md.</p>
    </div>
  );
}
