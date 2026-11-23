export const money = (v: number): string => "$" + v.toFixed(2);

/** "now", "1m", "3h", "2d" (the Inbox "Last" column). */
export function agoShort(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.round(h / 24)}d`;
}
export const agoLong = (ms: number, now = Date.now()): string => {
  const a = agoShort(ms, now);
  return a === "now" ? "just now" : `${a} ago`;
};

export function basename(path?: string): string {
  if (!path) return "";
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] ?? "";
}

export function clockTime(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** A moment of a conversation from some other day: "Nov 20, 14:02" (with the year when it is not this one). */
export function dayTime(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const month = d.toLocaleDateString("en-US", { month: "short" });
  return `${month} ${d.getDate()}${d.getFullYear() === new Date(now).getFullYear() ? "" : `, ${d.getFullYear()}`}, ${clockTime(ms)}`;
}

export function compactNumber(n: number): string {
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1000) return Math.round(n / 1000) + "k";
  return String(n);
}

/** How long something took or has been running: "41s", "3m 05s", "1h 02m". */
export function elapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}
