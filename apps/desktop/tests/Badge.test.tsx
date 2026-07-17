/**
 * Badge — liveness-first display (the two-axis projection).
 * A not-running session reads "Paused" and overrides its activity phase entirely
 * (its gate/error is wasted on pause); a live session shows its phase as usual.
 */
import { describe, it, expect } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import { ThemeProvider } from "../src/theme/ThemeProvider";
import { Badge } from "../src/components/Badge";
import type { Phase } from "../src/bridge/types";

function r(el: React.ReactElement) {
  return render(<ThemeProvider initialTheme="dark">{el}</ThemeProvider>);
}

describe("Badge — liveness-first (Paused overrides phase)", () => {
  it("reads PAUSED when live===false, regardless of the activity phase", () => {
    for (const state of ["Done", "Working", "Needs", "Error"] as Phase[]) {
      r(<Badge state={state} live={false} />);
      expect(screen.getByText(/^paused$/i)).toBeTruthy();
      cleanup();
    }
  });

  it("shows the activity phase when live===true", () => {
    r(<Badge state="Done" live={true} />);
    expect(screen.getByText(/^done$/i)).toBeTruthy();
    cleanup();
    r(<Badge state="Working" live={true} />);
    expect(screen.getByText(/^working$/i)).toBeTruthy();
  });

  it("treats missing live as live (shows the phase)", () => {
    r(<Badge state="Done" />);
    expect(screen.getByText(/^done$/i)).toBeTruthy();
  });

  it("pause wins over a cancel reason (paused Done+Cancelled → Paused, not Cancelled)", () => {
    r(<Badge state="Done" endReason="Cancelled" live={false} />);
    expect(screen.getByText(/^paused$/i)).toBeTruthy();
    expect(screen.queryByText(/cancelled/i)).toBeNull();
  });

  it("a live cancelled turn still reads Cancelled", () => {
    r(<Badge state="Done" endReason="Cancelled" live={true} />);
    expect(screen.getByText(/cancelled/i)).toBeTruthy();
  });
});
