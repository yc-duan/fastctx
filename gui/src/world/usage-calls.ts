import type { HubAccount } from "./accounts";
import type { Tokens } from "./usage";
import type { Timestamp, ResidentId } from "./wire";

export type CallSource = "main" | "agent" | "background";
export type CallOutcome = "completed" | "failed" | "interrupted" | "held" | "unknown";
export type CallPhase = "before_response" | "streaming" | "finished" | "unknown";
export interface CallTiming {
  started_at: Timestamp | null;
  first_at: Timestamp | null;
  ended_at: Timestamp;
  approximate: boolean;
}
export interface CallCursor { at: Timestamp; key: string }
export interface UsageCallsQuery {
  person?: { account: HubAccount | null } | null;
  model?: string | null;
  source?: CallSource | null;
  outcome?: CallOutcome | null;
  conversation?: ResidentId | null;
  turn?: string | null;
  before?: CallCursor | null;
}
export interface UsageCall {
  cursor: CallCursor;
  who: HubAccount | null;
  model: string | null;
  tokens: Tokens | null;
  /** Millionths of a dollar, as the ledger keeps it. */
  usd: number | null;
  source: CallSource;
  agent: string | null;
  timing: CallTiming;
  outcome: CallOutcome;
  phase: CallPhase;
  /** Null for another person's conversation, even to the machine owner. */
  conversation: ResidentId | null;
  turn: string | null;
}
export interface UsageCallsPage { rows: UsageCall[]; next: CallCursor | null }
