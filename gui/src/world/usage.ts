// What an agent's login was used for (`world-agent-logins.md`, 用量), as its
// machine's node answers it: the accounts its conversations worked for, the
// tokens and Claude Code's own cost by the hour over a year, what the
// current week went to in all and for each of them, the login's reset weeks
// with who used how much in each, and the login's windows as read through
// that week. Amounts of dollars are in millionths of a dollar.

import type { HubAccount } from "./accounts";
import type { LimitWindow } from "./agents";
import type { Timestamp } from "./wire";

export interface UsageSum {
  tokens: number;
  /** Absent where the agent keeps no cost ledger (Codex). */
  usd?: number;
}

export interface UsagePerson {
  /** Null is the user of the machine itself: its own sessions work for
   *  whoever is logged in there. */
  account: HubAccount | null;
  /** What it used in the week, as its weekly allowance counts it. */
  week: UsageSum;
  /** What it used over the last 24 hours, as its daily allowance counts
   *  it; older nodes do not say. */
  day?: UsageSum;
  /** What it used on Fable models this week; older nodes do not say. */
  fable?: UsageSum;
  /** What its week went to; older nodes do not say. */
  went?: WeekUse;
  /** What its week counts as against the week's figure, in millionths of
   *  a quota dollar, after the resets made by hand: all its models, and the
   *  part of that which is Fable's. Where the report has a reckoning. */
  counted?: { week: number; fable: number };
}

export interface UsageHour extends UsageSum {
  /** The hour's start, or the weekly boundary that split it. */
  at: Timestamp;
  /** Its place in the report's people. */
  person: number;
  /** What of its dollars went to Fable models; older nodes do not say. */
  fable?: number;
}

export interface Tokens {
  input?: number;
  cache_read?: number;
  cache_write?: number;
  output?: number;
  reasoning?: number;
}

export interface KindsUsd {
  input: number;
  cache_read: number;
  cache_write: number;
  output: number;
}

export interface ModelUse extends UsageSum {
  model: string;
}

export interface WeekUse {
  models: ModelUse[];
  tokens: Tokens;
  kinds_usd?: KindsUsd;
}

export interface UsageWeek extends WeekUse {
  start: Timestamp | null;
  end: Timestamp | null;
  /** Whether the week is the login's own seven-day window; otherwise the
   *  seven days up to now. */
  window: boolean;
}

/** One week of the login's own seven-day window, as its readings recorded
 *  it, and what each person used in it. */
export interface UsageSpan {
  start: Timestamp;
  end: Timestamp;
  /** How much of the seven-day window the account had used by the week's
   *  highest reading, in tenths of a percent; absent to an account the login
   *  only assists. */
  used_tenths?: number;
  /** Each person's use, and their part of the window's, shared out by what
   *  each used. */
  used: (UsageSum & { person: number; quota_tenths?: number })[];
}

export interface UsageReading {
  at: Timestamp;
  windows: LimitWindow[];
}

/** A weekly accounting boundary recorded by the login's machine. */
export interface UsageReset {
  window: string;
  since: Timestamp;
  confirmed_at: Timestamp;
  resets_at: Timestamp;
  before_used_tenths?: number;
  used_tenths: number;
  source: "automatic" | "scheduled" | "recorded";
}

export interface UsageReport {
  people: UsagePerson[];
  hours: UsageHour[];
  week: UsageWeek;
  /** None to an account the login only assists. */
  readings: UsageReading[];
  /** Oldest first, the current week last; older nodes do not say. */
  weeks?: UsageSpan[];
  /** Only the login's owner receives this field, even when it is empty. */
  resets?: UsageReset[];
  /** What a Claude Code login's week is reckoned to hold; absent for
   *  Codex and from a node of before. */
  reckoning?: UsageReckoning;
}

/** What a login's week is reckoned to hold, in millionths of a quota
 *  dollar: what its owner expects, how far a week's figure may stand from
 *  that, and this week's figure, which stands all week. */
export interface UsageReckoning {
  base: number;
  /** Percent either way. */
  swing: number;
  week: number;
  /** The weeks measured, oldest first; to the owner and to one who manages
   *  the login with them. */
  weeks?: UsageMeasured[];
  /** The resets made by hand, newest first; to the owner alone. */
  resets?: UsageHandReset[];
}

/** What one week came to: the ledger's quota dollars between its first
 *  reading and its last, over how far the reading rose. */
export interface UsageMeasured {
  start: Timestamp;
  end: Timestamp;
  spent: number;
  rise_tenths: number;
  /** Absent where the reading rose too little to say. */
  came_to?: number;
}

/** A week's usage reset by hand: what everyone used until then counts as
 *  `to_tenths` over `from_tenths` of what it was. */
export interface UsageHandReset {
  id: string;
  at: Timestamp;
  week_end: Timestamp;
  /** The week's figure then, and what everyone had used of the week and
   *  what that counts as since, in millionths of a quota dollar. */
  figure: number;
  from: number;
  to: number;
  from_tenths: number;
  to_tenths: number;
  /** The account that made it; absent for the machine's own user. */
  by?: string;
  undone?: Timestamp;
}
