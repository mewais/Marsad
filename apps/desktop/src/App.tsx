/**
 * Marsad desktop UI root.
 *
 * Renders the state-router: AppShell + the active screen based on store nav state.
 * Screen placeholder components live in src/screens/ — D02 agents replace the bodies.
 */
import { VmProvider, useVm } from "./bridge/useVm";
import { ThemeProvider } from "./theme/ThemeProvider";
import { AppShell } from "./components/AppShell";
import { SessionDetail } from "./screens/SessionDetail";
import { GateOverlay } from "./screens/GateOverlay";
import { NewWorkstreamModal } from "./screens/NewWorkstreamModal";
import { NodeModal } from "./screens/NodeModal";
import { Settings } from "./screens/Settings";
import { BoardScreen } from "./screens/BoardScreen";
import { GraphScreenConnected } from "./screens/graph/GraphScreenConnected";

// ---------------------------------------------------------------------------
// Inner router (uses useVm which requires VmProvider above it)
// ---------------------------------------------------------------------------

// Workstreams redesign (SLICE A): Board is the home screen. The old Overview
// screen and the entire Workspaces UI are removed from navigation — Overview
// as a nav destination is gone (its component file is kept only because
// several unrelated unit suites render it directly to exercise session-card
// logic; see src/screens/Overview.tsx). Workspaces.tsx is deleted outright.
function Router() {
  const { state } = useVm();

  function renderScreen() {
    switch (state.screen) {
      case "session":
        return <SessionDetail />;
      case "gate":
        return <GateOverlay />;
      case "new-workstream":
        return <NewWorkstreamModal />;
      case "node-modal":
        return <NodeModal />;
      case "settings":
        return <Settings />;
      case "graph":
        return <GraphScreenConnected />;
      case "board":
      default:
        return <BoardScreen />;
    }
  }

  return <AppShell>{renderScreen()}</AppShell>;
}

// ---------------------------------------------------------------------------
// App root
// ---------------------------------------------------------------------------

export function App() {
  return (
    <ThemeProvider initialTheme="dark" initialGlass={false}>
      <VmProvider>
        <Router />
      </VmProvider>
    </ThemeProvider>
  );
}
