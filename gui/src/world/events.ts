// A session's events as the World sends them: the JSON form of
// `fastctx_types::session::SessionEvent`. Every interface reads the same
// stream; this is its shape, and nothing here interprets it.

export type MachineName = string;
export type ResidentId = string;

export interface Address {
  machine: MachineName;
  resident: ResidentId;
}

export type Speaker =
  | { kind: "user"; machine: MachineName }
  | { kind: "resident"; address: Address }
  | { kind: "world" };

export type ToolStatus = "running" | "done" | "failed" | "declined" | "cut" | "unknown";

export type ChangeHow = "added" | "deleted" | "updated" | "replaced" | { moved: { to: string } };

export interface Change {
  path: string;
  how: ChangeHow;
  /** A unified diff of this change. */
  diff: string;
}

export type Does =
  | { as: "command"; command: string; cwd: string | null; exit: number | null; job: string | null }
  | { as: "edit"; changes: Change[] }
  | { as: "search"; query: string }
  | { as: "image"; path: string | null }
  | { as: "colleague"; to: string | null; task: string | null }
  | { as: "call" };

export type Content =
  | { type: "text"; text: string }
  | { type: "image"; mime: string; base64: string }
  | { type: "image_url"; url: string }
  | { type: "audio_url"; url: string }
  | { type: "other"; block: unknown };

export interface Tool {
  name: string;
  server: string | null;
  /** The machine it acted on, when the World's own tools say. */
  machine: MachineName | null;
  does: Does;
  input: unknown;
  output: Content[];
  status: ToolStatus;
  duration_ms: number | null;
}

export type What =
  | { kind: "message"; text: string }
  | { kind: "thinking"; text: string }
  | { kind: "plan"; text: string }
  | ({ kind: "tool" } & Tool)
  | { kind: "compaction" }
  | { kind: "agent_activity"; agent: string; path: string; activity: string }
  | { kind: "hidden"; host_kind: string }
  | { kind: "other"; host_kind: string };

export type Item = { id: string; started_at?: string; detail?: unknown } & What;

export type TurnHow = "completed" | "failed" | "interrupted" | string;

export interface TurnEnd {
  how: TurnHow;
  reason: string | null;
  error?: unknown;
  duration_ms: number | null;
  last_message: string | null;
}

export type StepStatus = "done" | "doing" | "pending";

export interface Step {
  text: string;
  status: StepStatus;
}

export interface Health {
  kind: string;
  text: string;
}

export type Happened =
  | { event: "turn_started"; turn: string }
  | { event: "heard"; delivery?: string | null; turn: string | null; from: Speaker | null; text: string }
  | { event: "began"; turn: string; item: Item }
  | { event: "grew"; revision?: number; turn: string; item: string; part: "text" | string; text: string }
  | { event: "finished"; turn: string; item: Item }
  | { event: "updated"; turn: string; item: Item }
  | { event: "changed"; turn: string | null; change: { kind: string } & Record<string, unknown> }
  | { event: "steps"; turn: string; steps: Step[]; note: string | null }
  | { event: "context"; used: number; window: number | null }
  | { event: "titled"; title: string }
  | { event: "turn_ended"; turn: string; end: TurnEnd }
  | { event: "health"; turn: string | null; health: Health }
  | { event: "delivery_uncertain"; why: string }
  | { event: "realtime"; item: unknown; complete: boolean }
  | { event: "realtime_grew"; item: string; text: string };

export type SessionEvent = { at: string } & Happened;

/** Who a resident is, as the World knows it (`about`). */
export interface Resident {
  id: ResidentId;
  machine: MachineName;
  agent: "codex" | "claude" | string;
  model: string | null;
  /** Asked for by another resident, with its few words of task. */
  colleagueOf?: { address: Address; label: string };
  town: string | null;
}
