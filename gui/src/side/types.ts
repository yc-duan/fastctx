// What the panel beside a conversation draws: its goal, and who works with
// it. These are the interface's own shapes; what the World sends is read
// into them where it comes in.

export type GoalStatus = "active" | "paused" | "blocked" | "complete";

/** Who put a goal on pause: the user, the agent at the user's word, the
 *  World when the login's quota ran out, or the World stopping it for the
 *  user to decide: the goal's sentinel found a stop sound, no sentinel could
 *  look, or the assistance it ran under ended; `unwoken`, a standing goal
 *  that twice in a row stopped with nothing left to wake it. */
export type PausedBy = "user" | "agent" | "quota" | "world" | "unwoken";

export interface SentinelLook {
  /** When it looked, in milliseconds since the epoch. */
  at: number;
  /** Whether it found the stop it looked at sound. */
  sound: boolean;
  reason?: string;
  looked?: string;
  saw?: string;
  thinks?: string;
}

export interface GoalShown {
  /** The user's words. */
  objective: string;
  version: number;
  status: GoalStatus;
  /** How long it has been worked at, in milliseconds. */
  worked: number;
  /** Since when it is in this state, in milliseconds since the epoch. */
  since: number;
  /** `reason` is the World's own words for the stop, shown when pointed at. */
  paused?: { by: PausedBy; until: number | null; reason?: string | null } | null;
  /** The agent's account of it done or blocked, as Markdown. */
  statement?: string | null;
  sentinel?: SentinelLook | null;
  id?: string;
  /** Never done: kept going, waking on what it leaves to wake it. */
  standing?: boolean;
}

/** A goal no longer current: done, or put down when it was cleared or
 *  another was set after it. Moments in milliseconds since the epoch. */
export interface PastGoal {
  id: string;
  objective: string;
  done: boolean;
  standing: boolean;
  created: number;
  closed: number;
  statement: string | null;
}

/** How one stands; `unknown` where only its start is known, and it may
 *  have ended long ago or work still. */
export type NodeState = "working" | "done" | "failed" | "stopped" | "idle" | "unknown";

/** One that works with a conversation: a colleague it asked, a side
 *  conversation branched off it, or one of the host's own agents, under
 *  whoever sent it. */
export interface CollabNode {
  id: string;
  kind: "colleague" | "subagent" | "side";
  agent: "claude" | "codex";
  name: string;
  task: string;
  state: NodeState;
  /** A colleague's own goal, which it works at until it hands back. */
  goal?: GoalStatus | null;
  /** The asker's goal the colleague's goal branched from. */
  branchOf?: string | null;
  /** How long it has worked, in milliseconds. */
  took: number;
  steps: number | null;
  machine?: string;
  /** Why how it stands is not known, as the World said it. */
  why?: string | null;
  children: CollabNode[];
}
