/**
 * The wire, as the UI sees it: every payload type is generated from the Rust crates
 * (`./generated`, read-only here) and re-exported from this module, plus the method and topic
 * names from `marsad-proto` (`methods`, `topics`) and `wasit-runtime::wire`.
 */
export type * from "./generated";
export type { JsonValue } from "./generated/serde_json/JsonValue";
import type { GateRequest, HostId, SessionId } from "./generated";
import { methods as generatedMethods, topicPrefixes, topics as generatedTopics } from "./generated/names";

export type GateOf<K extends GateRequest["gate_type"]> = Extract<GateRequest, { gate_type: K }>;
export type PhaseKind = import("./generated").Phase["state"];

/** UI-only: a session is addressed by host + id. */
export type SessionKey = string;
export const sessionKey = (host: HostId, id: SessionId): SessionKey => `${host}/${id}`;
export function splitKey(key: SessionKey): { host: HostId; id: SessionId } {
  const i = key.indexOf("/");
  return { host: key.slice(0, i), id: key.slice(i + 1) };
}

/** Method names, generated from the Rust constants (`marsad-ts-export`, `generated/names.ts`). */
export const methods = generatedMethods;

/** Topic names (generated), plus the per-object topics. */
export const topics = {
  ...generatedTopics,
  sessionEvents: (id: SessionId) => `${topicPrefixes.SESSION_EVENTS}${id}`,
  sessionTrail: (id: SessionId) => `${topicPrefixes.SESSION_TRAIL}${id}`,
  ptyOutput: (id: string) => `${topicPrefixes.PTY_OUTPUT}${id}`,
} as const;
