import { createContext } from "react";
import type { SessionKey } from "../../bridge/wire";

/** Which session a transcript belongs to: the rows that show live facts of the daemon (a subagent's state) look them up by it. */
export interface TranscriptScopeInfo {
  key: SessionKey;
  host: string;
  id: string;
}

export const TranscriptCtx = createContext<TranscriptScopeInfo | null>(null);
