import type { Shortcut } from "./registry";

/**
 * Every shortcut the app has, including those of screens that are not mounted right now, so the
 * "?" sheet and Settings → Keyboard are complete. Mounted shortcuts (the registry) win by id.
 */
type Entry = Pick<Shortcut, "id" | "keys" | "title" | "group">;
export const SHORTCUT_CATALOG: Entry[] = [
  { id: "palette", keys: ["mod+k"], title: "Command palette", group: "Global" },
  { id: "help", keys: ["?"], title: "Show shortcuts", group: "Global" },
  { id: "escape-overlay", keys: ["escape"], title: "Close the open menu, dialog, preview or editor (before anything else)", group: "Global" },
  { id: "new", keys: ["n"], title: "New session", group: "Global" },
  { id: "new-ws", keys: ["shift+n"], title: "New workstream", group: "Global" },
  { id: "go-inbox", keys: ["g", "i"], title: "Go to Inbox", group: "Global" },
  { id: "go-ws", keys: ["g", "w"], title: "Go to Workstreams", group: "Global" },
  { id: "go-hosts", keys: ["g", "h"], title: "Go to Hosts", group: "Global" },
  { id: "go-config", keys: ["g", "c"], title: "Go to Config", group: "Global" },
  { id: "go-settings", keys: ["g", "s"], title: "Go to Settings", group: "Global" },
  { id: "inbox-next", keys: ["j"], title: "Next session", group: "Inbox" },
  { id: "inbox-prev", keys: ["k"], title: "Previous session", group: "Inbox" },
  { id: "inbox-open", keys: ["enter"], title: "Open the selected session", group: "Inbox" },
  { id: "inbox-seen", keys: ["e"], title: "Mark seen", group: "Inbox" },
  { id: "inbox-filter", keys: ["/"], title: "Filter", group: "Inbox" },
  { id: "gate-digit-1", keys: ["1"], title: "Pick the agent's choice 1 (2, 3, 4 likewise)", group: "Answering a gate" },
  { id: "gate-allow", keys: ["a"], title: "Allow (the agent's first allow choice)", group: "Answering a gate" },
  { id: "gate-deny", keys: ["d"], title: "Deny", group: "Answering a gate" },
  { id: "gate-send", keys: ["enter"], title: "Send the answer", group: "Answering a gate" },
  { id: "session-interrupt", keys: ["escape"], title: "Interrupt the turn that is running", group: "Session" },
  { id: "session-blur", keys: ["escape"], title: "Leave the text field you are typing in", group: "Session" },
  { id: "transcript-find", keys: ["mod+f"], title: "Find in the transcript", group: "Session" },
  { id: "transcript-prev-prompt", keys: ["alt+arrowup"], title: "Previous prompt", group: "Session" },
  { id: "transcript-next-prompt", keys: ["alt+arrowdown"], title: "Next prompt", group: "Session" },
  { id: "session-back", keys: ["escape"], title: "Back to the Inbox (when no turn is running)", group: "Session" },
  { id: "ns-submit", keys: ["mod+enter"], title: "Start the session", group: "New session" },
  { id: "hosts-add", keys: ["a"], title: "Add host", group: "Hosts" },
  { id: "ws-review", keys: ["r"], title: "Review the waiting handoff", group: "Workstream" },
  { id: "ws-add", keys: ["a"], title: "Add agent (design)", group: "Workstream" },
  { id: "ws-fit", keys: ["f"], title: "Fit the graph to the window", group: "Workstream" },
  { id: "ws-cancel", keys: ["escape"], title: "Cancel drawing a connection", group: "Workstream" },
];

export function allShortcuts(registered: Entry[]): Entry[] {
  const byId = new Map<string, Entry>();
  for (const s of SHORTCUT_CATALOG) byId.set(`${s.group}:${s.id}`, s);
  for (const s of registered) if (!byId.has(`${s.group}:${s.id}`) && !/^gate-digit-[2-9]/.test(s.id)) byId.set(`${s.group}:${s.id}`, s);
  return [...byId.values()];
}
