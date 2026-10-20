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
}

export interface UsageHour extends UsageSum {
  /** The hour's start, or the weekly boundary that split it. */
  at: Timestamp;
  /** Its place in the report's people. */
  person: number;
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

export interface UsageReport {
  people: UsagePerson[];
  hours: UsageHour[];
  week: UsageWeek;
  /** None to an account the login only assists. */
  readings: UsageReading[];
  /** Oldest first, the current week last; older nodes do not say. */
  weeks?: UsageSpan[];
}
