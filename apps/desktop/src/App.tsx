/**
 * Marsad desktop UI root.
 *
 * Renders the state-router: AppShell + the active screen based on store nav state.
 * Screen placeholder components live in src/screens/ — D02 agents replace the bodies.
 */
import { VmProvider, useVm } from "./bridge/useVm";
import { ThemeProvider } from "./theme/ThemeProvider";
import { AppShell } from "./components/AppShell";
import { Overview } from "./screens/Overview";
import { SessionDetail } from "./screens/SessionDetail";
import { GateOverlay } from "./screens/GateOverlay";
import { NewSession } from "./screens/NewSession";
import { Settings } from "./screens/Settings";
import { Workspaces } from "./screens/Workspaces";

// ---------------------------------------------------------------------------
// Inner router (uses useVm which requires VmProvider above it)
// ---------------------------------------------------------------------------

function Router() {
  const { state } = useVm();

  function renderScreen() {
    switch (state.screen) {
      case "overview":
        return <Overview />;
      case "session":
        return <SessionDetail />;
      case "gate":
        return <GateOverlay />;
      case "new-session":
        return <NewSession />;
      case "settings":
        return <Settings />;
      case "workspaces":
        return <Workspaces />;
      default:
        return <Overview />;
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
