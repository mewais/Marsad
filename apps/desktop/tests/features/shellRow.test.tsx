import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SessionEvent } from "../../src/bridge/wire";
import { TranscriptItem } from "../../src/features/session/TranscriptView";
import { TranscriptCtx } from "../../src/features/session/transcriptContext";
import { ExpansionCtx, createExpansion } from "../../src/features/session/expansion";
import { groupToolRuns, type RowItem } from "../../src/features/session/toolRuns";
import { createTranscriptFold } from "../../src/features/session/transcript";
import { MAIN } from "../../src/state/scope";

let n = 0;
const ev = (e: Record<string, unknown>) => ({ cursor: ++n * 65536, t: n, ...e }) as unknown as SessionEvent;
const agent = (event: unknown) => ev({ kind: "agent", event });
const rowsOf = (events: SessionEvent[]): RowItem[] => groupToolRuns(createTranscriptFold(MAIN).update(events));

const KEY = "h1/s1";
const CTX = { key: KEY, host: "h1", id: "s1" };

function renderRow(row: RowItem) {
  return render(
    <TranscriptCtx.Provider value={CTX}>
      <TranscriptItem item={row} />
    </TranscriptCtx.Provider>,
  );
}

const shell = (command: string, output: string, extra: Record<string, unknown> = {}) =>
  agent({ type: "local_shell", command, cwd: "/project", exit_code: 0, output, output_truncated: false, origin: "host", ...extra });

describe("shell row", () => {
  it("folds a local_shell event into a shell row", () => {
    const row = rowsOf([shell("npm test", "ok\n")])[0];
    expect(row.kind).toBe("shell");
    expect(row).toMatchObject({ command: "npm test", cwd: "/project", exitCode: 0, output: "ok\n", outputTruncated: false, origin: "host" });
  });

  it("renders a compact header with command, folder and exit code", () => {
    renderRow(rowsOf([shell("npm test", "ok\n")])[0]);
    const header = screen.getByRole("button", { name: /\$ npm test/ });
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(within(header).getByText("npm test")).toBeInTheDocument();
    expect(within(header).getByText("/project")).toBeInTheDocument();
    expect(screen.getByTestId("shell-exit")).toHaveTextContent("exit 0");
  });

  it("shows 'stopped' when no exit code was reported", () => {
    renderRow(rowsOf([shell("npm test", "", { exit_code: undefined })])[0]);
    expect(screen.getByTestId("shell-exit")).toHaveTextContent("stopped");
  });

  it("expands to show monospace output and a truncated note", async () => {
    const user = userEvent.setup();
    renderRow(rowsOf([shell("cat log", "line1\nline2\n", { output_truncated: true })])[0]);
    await user.click(screen.getByRole("button", { name: /\$ cat log/ }));
    expect(screen.getByTestId("shell-output")).toHaveTextContent("line1 line2");
    expect(screen.getByTestId("shell-truncated")).toHaveTextContent("Output truncated");
  });

  it("does not show a truncated note when the output was not truncated", async () => {
    const user = userEvent.setup();
    renderRow(rowsOf([shell("echo hi", "hi\n")])[0]);
    await user.click(screen.getByRole("button", { name: /\$ echo hi/ }));
    expect(screen.queryByTestId("shell-truncated")).toBeNull();
  });

  it("keeps an expanded output open when the row remounts", async () => {
    const user = userEvent.setup();
    const expansion = createExpansion();
    const row = rowsOf([shell("npm run build", "built\n")])[0];
    const mount = () =>
      render(
        <ExpansionCtx.Provider value={expansion}>
          <TranscriptCtx.Provider value={CTX}>
            <TranscriptItem item={row} />
          </TranscriptCtx.Provider>
        </ExpansionCtx.Provider>,
      );
    const first = mount();
    await user.click(screen.getByRole("button", { name: /\$ npm run build/ }));
    expect(screen.getByTestId("shell-output")).toHaveTextContent("built");
    first.unmount();

    mount();
    expect(screen.getByRole("button", { name: /\$ npm run build/ })).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByTestId("shell-output")).toHaveTextContent("built");
  });
});
