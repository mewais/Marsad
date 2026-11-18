import type { GateRequest } from "../bridge/wire";

/**
 * How a permission gate reads. Agents raise three different things as "permission":
 * a command execution (`raw_input.command`, or the CLI's shell tool), a question with options
 * (ACP permission-with-options whose input carries `questions` and no command: the "command" is
 * the question text), and any other tool call (edit a file, fetch a URL…).
 */
type Perm = Extract<GateRequest, { gate_type: "permission" }>;
export type PermissionKind = "command" | "question" | "tool";

const SHELL_TOOLS = new Set(["Bash", "shell", "execute", "bash"]);
const obj = (v: unknown): Record<string, unknown> | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

export function permissionKind(g: Perm): PermissionKind {
  const raw = obj(g.raw_input);
  const hasCommand = raw ? typeof raw.command === "string" || Array.isArray(raw.command) : false;
  if (hasCommand || (SHELL_TOOLS.has(g.tool_name) && !Array.isArray(raw?.questions))) return "command";
  if (Array.isArray(raw?.questions) || g.tool_name === "question") return "question";
  return "tool";
}

/** Headline: "Run a shell command" / "Question" / "Use <tool>". */
export function permissionTitle(g: Perm): string {
  const k = permissionKind(g);
  return k === "command" ? "Run a shell command" : k === "question" ? "Question" : `Use ${g.tool_name}`;
}

/** One-line form: `{ prefix, text }`, e.g. "Run" + "pnpm test"; a question has no prefix. */
export function permissionLine(g: Perm): { prefix: string; text: string; code: boolean } {
  const k = permissionKind(g);
  if (k === "command") return { prefix: "Run ", text: g.command, code: true };
  if (k === "question") return { prefix: "", text: g.command, code: false };
  return { prefix: `${g.tool_name}: `, text: g.command, code: true };
}

/** Short label for lists (session side panel). */
export function permissionShort(g: Perm, max = 28): string {
  const l = permissionLine(g);
  const t = l.text.length > max ? `${l.text.slice(0, max - 1)}…` : l.text;
  return `${l.prefix}${t}`;
}
