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
/** `account: null` is the machine's own use; `with_machine` counts it as
 *  the account's too. */
export interface CallPerson {
  account: HubAccount | null;
  with_machine?: boolean;
}
export interface UsageCallsQuery {
  person?: CallPerson | null;
  model?: string | null;
  source?: CallSource | null;
  outcome?: CallOutcome | null;
  conversation?: ResidentId | null;
  turn?: string | null;
  before?: CallCursor | null;
  /** Rows that ended at or after this moment, and before `until`. */
  from?: string | null;
  until?: string | null;
  /** The page asked for by its number, from 0; the answer then counts all. */
  page?: number | null;
}
export interface UsageCall {
  cursor: CallCursor;
  who: HubAccount | null;
  model: string | null;
  /** The reasoning effort the request was sent with; absent where none was. */
  effort?: string | null;
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
/** `total` is every row selected, where a page was asked for by number;
 *  an older Fastctx leaves it out. */
export interface UsageCallsPage { rows: UsageCall[]; next: CallCursor | null; total?: number | null }
