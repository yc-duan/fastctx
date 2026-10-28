// A conversation's goal, read and written through the host's World
// (`world-goal.md`). The goal is one account record; only its live view is
// pushed, the goal's sentinel's last reading of a stop with it.

import { invoke } from "@tauri-apps/api/core";
import { observe } from "./delivery";
import type { GoalShown, GoalStatus, PausedBy, SentinelLook } from "../side/types";
import type { ResidentId, Timestamp } from "./wire";

type StopKind = "user" | "claimed" | "quota" | "assistance" | "review" | "retry_exhausted" | "context";

interface GoalStop {
  kind: StopKind;
  reason: string;
  resume_at?: Timestamp | null;
}

type Verdict =
  | { verdict: "sound"; reason: string }
  | { verdict: "advice"; looked: string; saw: string; thinks: string }
  | { verdict: "failed"; reason: string };

interface GoalRecord {
  objective: string;
  status: GoalStatus;
  created_at: Timestamp;
  changed_at: Timestamp;
  stop?: GoalStop | null;
  /** The agent's account of it done or blocked. */
  statement?: string | null;
  runtime?: { last_review?: { at: Timestamp; verdict: Verdict } | null };
}

interface GoalPending {
  action: "pause" | "end";
  reason: string;
  at: Timestamp;
}

export interface GoalView {
  version: number | null;
  goal: GoalRecord | null;
  pending?: GoalPending | null;
}

export type GoalUpdated = { result: "goal"; view: GoalView } | { result: "conflict"; current: number | null };

/** A change to the goal: its words anew, or how it stands. */
export type GoalChange = { change: "open" | "edit"; objective: string } | { change: "pause" | "resume" | "clear" };

export type GoalPiece =
  | { kind: "snapshot"; view: GoalView }
  | { kind: "interrupted"; error: string; since: number };

export function goalRead(owner: ResidentId): Promise<GoalView> {
  return invoke<GoalView>("goal_read", { residentId: owner });
}

export function goalUpdate(
  owner: ResidentId,
  operationId: string,
  expected: number | null,
  change: GoalChange,
): Promise<GoalUpdated> {
  return invoke<GoalUpdated>("goal_update", { residentId: owner, operationId, expected, change });
}

/** Follows the goal: its view now (resolved here), then after each change. */
export function watchGoal(owner: ResidentId, onPiece: (piece: GoalPiece) => void): Promise<{ id: number; view: GoalView }> {
  return observe<GoalPiece, { id: number; view: GoalView }>("goal_watch", { residentId: owner }, onPiece);
}

/** Which stop paused the goal, in the interface's terms: the agent pauses
 *  only at the user's word; a review or an ended assistance is the World's,
 *  left for the user to decide. */
function pausedBy(kind: StopKind | undefined): PausedBy {
  if (kind === "user") return "user";
  if (kind === "quota") return "quota";
  if (kind === "review" || kind === "assistance") return "world";
  return "agent";
}

/** What the goal's sentinel last made of a stop; nothing where it could not judge. */
function lookOf(goal: GoalRecord): SentinelLook | null {
  const review = goal.runtime?.last_review;
  const at = review ? Date.parse(review.at) : NaN;
  if (!review || !Number.isFinite(at)) return null;
  const verdict = review.verdict;
  if (verdict.verdict === "sound") return { at, sound: true, reason: verdict.reason };
  if (verdict.verdict === "advice") return { at, sound: false, looked: verdict.looked, saw: verdict.saw, thinks: verdict.thinks };
  return null;
}

/** The goal as the panel shows it, from the World's view. A locally observed
 *  stop awaiting its record is shown as paused, not as the old running state. */
export function goalShown(view: GoalView): GoalShown | null {
  const goal = view.goal;
  if (!goal) return null;
  const pendingPause = view.pending?.action === "pause";
  const status: GoalStatus = pendingPause ? "paused" : goal.status;
  const created = Date.parse(goal.created_at);
  const changed = Date.parse(goal.changed_at);
  const since = Number.isFinite(changed) ? changed : created;
  // The World keeps no running total: worked is the time from setting it to
  // its present state, and an active one adds the stretch under way.
  const shown: GoalShown = {
    objective: goal.objective,
    version: view.version ?? 0,
    status,
    worked: Number.isFinite(created) && Number.isFinite(since) ? Math.max(0, since - created) : 0,
    since,
    paused: null,
    statement: goal.statement ?? null,
    sentinel: lookOf(goal),
  };
  if (status === "paused") {
    const until = goal.stop?.resume_at ? Date.parse(goal.stop.resume_at) : NaN;
    shown.paused = {
      by: pendingPause ? "agent" : pausedBy(goal.stop?.kind),
      until: Number.isFinite(until) ? until : null,
      reason: view.pending?.reason ?? goal.stop?.reason ?? null,
    };
  }
  return shown;
}
