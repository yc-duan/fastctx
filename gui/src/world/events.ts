// A session's events as the World sends them: the JSON form of
// `fastctx_types::session::SessionEvent`. Every interface reads the same
// stream; this is its shape, and nothing here interprets it.

import type { Attachment, AttachmentSource } from "./attachments";
import type { PageSource } from "./pages";
import type { Focus } from "./focus";
import type { RunFacts } from "./run-facts";
import type { EndedHow, ExitStatus } from "./since";
import type { AgentLimits } from "./agents";
import type { HubAccount } from "./accounts";
import type { Address, JsonValue, MachineName, NoticeId, Timestamp } from "./wire";
export type { Address, MachineName, ResidentId } from "./wire";

// These types describe serialized output. A serde default alone does not
// make a field optional; only skip_serializing_if can omit one here.

export type Speaker =
  | { kind: "user"; machine: MachineName }
  | { kind: "resident"; address: Address }
  | { kind: "world" }
  /** An open page, saying what the user agreed it may (`pages.ts`). */
  | { kind: "page"; page: PageSource };

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
  /** The host's own agents: starting one, speaking to some, waiting for
   *  some, stopping them. `agents` are the ones it names, where it does. */
  | { as: "agent"; act: AgentAct; agents?: string[]; kind: string | null; task: string | null }
  | { as: "call" };

export type AgentAct = "start" | "message" | "resume" | "wait" | "stop" | "list";

/** Bytes named by their content: how many, and their SHA-256. */
export interface Fingerprint {
  size: number;
  sha256: string;
}

export type Content =
  | { type: "text"; text: string }
  | { type: "image"; mime: string; base64: string }
  | { type: "image_url"; url: string }
  /** An image kept where the session lives, asked for when it is shown,
   *  as a picture the user sent is. */
  | { type: "image_ref"; mime: string; content: Fingerprint; source: AttachmentSource }
  | { type: "audio_url"; url: string }
  | { type: "other"; block: JsonValue };

export interface Tool {
  run?: RunFacts;
  name: string;
  server: string | null;
  /** The machine it acted on, when the World's own tools say. */
  machine: MachineName | null;
  does: Does;
  input: JsonValue;
  output: Content[];
  status: ToolStatus;
  duration_ms: number | null;
}

export type What =
  | { kind: "message"; text: string }
  | { kind: "thinking"; text: string; duration_ms?: number }
  | { kind: "plan"; text: string }
  | ({ kind: "tool" } & Tool)
  | { kind: "compaction" }
  | { kind: "agent_activity"; agent: string; path: string; activity: string }
  | { kind: "hidden"; host_kind: string }
  | { kind: "other"; host_kind: string };

/** `draft` is the name it streamed under before its host kept it under
 *  `id`: whoever holds that item holds this one instead. */
export type Item = { id: string; draft?: string; started_at?: Timestamp; detail?: JsonValue } & What;

export type Part = "text" | "output";

/** Other(String) is untagged: unknown host endings remain bare strings. */
export type TurnHow = "completed" | "failed" | "interrupted" | string;

export interface TurnEnd {
  how: TurnHow;
  reason: string | null;
  error?: JsonValue;
  duration_ms: number | null;
  facts?: TurnFacts;
  last_message: string | null;
}

/** Host observations only; omitted fields have no reported value. */
export interface TurnFacts {
  models?: string[];
  efforts?: ({ kind: "actual" | "requested"; value: JsonValue })[];
  tokens?:
    | { host: "codex"; input_tokens: number; cached_input_tokens: number; cache_write_input_tokens?: number;
        output_tokens: number; reasoning_output_tokens: number; total_tokens: number }
    | { host: "claude"; input_tokens: number; cache_read_input_tokens: number;
        cache_creation_input_tokens: number; output_tokens: number };
  /** What the turn cost by the host's own ledger, in millionths of a
   *  dollar (Claude Code's). */
  cost?: number;
  /** The login's windows as last read when the turn ended; absent for an
   *  account the login only assists. */
  limits?: AgentLimits;
}

export type StepStatus = "done" | "doing" | "pending";

export interface Step {
  text: string;
  status: StepStatus;
  id?: string;
  active_form?: string;
  detail?: JsonValue;
}

export type HealthKind = "reconnecting" | "safety" | "rerouted" | "tools_missing" | "tool_server" | "warning" | "error";

export interface Health {
  kind: HealthKind;
  text: string;
}

export interface UserImage {
  source: AttachmentSource;
  name: string | null;
  media_type: string | null;
  /** The picture's bytes, where its machine read them. */
  content?: Fingerprint;
}

export interface UserMessage {
  /** The files that came with these words, when any did. */
  attachments?: Attachment[];
  /** Native pictures not already represented by the kept attachments. */
  images?: UserImage[];
  id: string;
  client_id: string | null;
  content: JsonValue[];
  hook_fragments: JsonValue[];
  presentation: JsonValue;
  detail: JsonValue;
}

export type SessionChange =
  | { kind: "run_facts_updated"; call_id: string; update: import("./run-facts").JobFacts }
  | { kind: "history_changed" }
  | { kind: "forked"; source: Address; thread: string; through: string | null }
  | { kind: "diff"; diff: string }
  | { kind: "terminal"; item: string; process: string; input: string }
  | { kind: "usage"; usage: JsonValue }
  | { kind: "status"; status: JsonValue; waiting_for_user?: boolean }
  | { kind: "metadata"; metadata: JsonValue }
  | { kind: "lifecycle"; state: string }
  | { kind: "goal"; goal: JsonValue }
  | { kind: "hook"; run: JsonValue }
  | { kind: "approval_review"; review: JsonValue; complete: boolean }
  | { kind: "strict_review"; notice: JsonValue }
  | { kind: "safety"; buffering: JsonValue }
  | { kind: "rerouted"; from: string; to: string; reason: string }
  | { kind: "verification"; verifications: JsonValue[] }
  | { kind: "error"; error: JsonValue; will_retry: boolean }
  | { kind: "warning"; text: string }
  | { kind: "settings"; settings: JsonValue }
  | { kind: "environment"; environment: JsonValue; connected: boolean }
  | { kind: "queue" }
  | { kind: "tool_server"; server: string; status: string; error: JsonValue; failure_reason: JsonValue }
  | { kind: "attachment"; attachment: JsonValue }
  | { kind: "request"; id: JsonValue; request: string; params: JsonValue }
  | { kind: "resolved"; id: JsonValue; resolution: JsonValue }
  | { kind: "voice"; event: string; data: JsonValue }
  /** What a Claude session asked of the model, and what came back: keyed by
   *  (turn, step); a later reading of one step replaces the earlier. */
  | { kind: "model_request"; step: number; model: string; effort: JsonValue }
  | { kind: "model_response"; step: number; usage: RequestUsage | null }
  | { kind: "model_switch"; from_model: string; to_model: string; source: string; requested_model: string | null; raw: JsonValue }
  | { kind: "message_usage"; items: string[]; response: string | null; model: string; api_error: boolean; usage: JsonValue }
  /** The commands it offers changed: ask for them again. */
  | { kind: "commands_changed" }
  /** The host's own agents: as it lists them now (Claude Code), one that
   *  started or more of what is known of it, what one did in its own loop,
   *  and how one's latest run ended. */
  | { kind: "agents"; agents: HostAgent[] }
  | { kind: "agent_started"; agent: HostAgentStart }
  | { kind: "agent_event"; agent_id: string; happened: Happened }
  | { kind: "agent_ended"; agent_id: string; end: HostAgentEnd }
  /** Turns removed from the session's history, as a rewind did. */
  | { kind: "history_cut"; removed_turns: string[] };

/** What one model request used, as the host counted it. */
export interface RequestUsage {
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_read_input_tokens: number;
  cache_creation_input_tokens: number;
}

/** One of Claude Code's own subagents, as it lists them. */
export interface HostAgent {
  id: string;
  description: string;
  type: string;
  status: string;
  parentId?: string;
  teammateId?: string;
  spawnedBy?: string;
  name?: string;
  [other: string]: JsonValue | undefined;
}

/** One of the host's own agents in a conversation, as one telling has it
 *  (`fastctx_types::session::HostAgentStart`): each adds what it knows. */
export interface HostAgentStart {
  id: string;
  /** The tool call that started it, in the loop that did. */
  call?: string;
  /** The agent whose loop started it; none for the conversation's own. */
  parent?: string;
  kind?: string;
  name?: string;
  task?: string;
  prompt?: string;
  model?: string;
  background: boolean;
  /** Told as it started: what it does comes as it happens. */
  live: boolean;
}

export interface HostAgentEnd {
  how: TurnHow;
  reason?: string;
  duration_ms?: number;
  tokens?: number;
  tools?: number;
  answer?: string;
  /** The few words of its task, for a notice read apart from its start. */
  task?: string;
  /** The conversation was told of it here, by the host's own notice. */
  told?: boolean;
}

/** What one of the host's own agents did, as its host keeps it. */
export interface AgentActivity {
  agent?: HostAgentStart;
  events: SessionEvent[];
  unavailable?: string;
}

/** What the World tells a resident, as facts (`fastctx_types::residents::Notice`):
 *  an interface shows these, never the words written for the model. */
export type Notice =
  | {
      kind: "job_ended";
      job: string;
      machine: MachineName;
      request?: NoticeId;
      follower?: MachineName;
      how: EndedHow;
      status: ExitStatus | null;
      at: Timestamp;
      output: string;
      stderr?: string;
      last?: { text: string; whole: boolean };
      /** Who stopped it, when someone did: `the user` when the user did. */
      stop?: { by: string; reason: string };
    }
  | { kind: "answer"; colleague: Address; label: string; turn: string; end: TurnEnd }
  | { kind: "health"; colleague: Address; label: string; health: Health }
  /** A question a colleague working toward a goal asked the conversation
   *  that asked it; it waits for that one's next turn. */
  | { kind: "question"; colleague: Address; id: string; text: string }
  /** Words from another conversation. A colleague's goal report and its
   *  questions carry what an interface draws of them beside the words its
   *  requester's model reads. */
  | { kind: "words"; from: Speaker; text: string; goal_report?: GoalReport; goal_question?: GoalQuestion }
  | { kind: "attached"; message: JsonValue; text: string }
  | { kind: "world"; fact: WorldFact; text: string };

/** What a colleague handed back when its goal settled. */
export interface GoalReport {
  id: NoticeId;
  /** The goal run it ends; a quota notice ends none. */
  completion?: NoticeId;
  colleague: Address;
  account: HubAccount;
  /** Older senders leave it out. */
  shown?: {
    label: string;
    status: "active" | "complete" | "blocked" | "paused";
    started_at: Timestamp;
    ended_at: Timestamp;
    resume_at?: Timestamp;
    /** The colleague's conclusion, as it wrote it. */
    text: string;
  };
}

/** A question a colleague asked without stopping its work. */
export interface GoalQuestion {
  id: string;
  colleague: Address;
  account?: HubAccount;
  /** The questions as its host asked them; older senders leave them out. */
  questions?: JsonValue;
}

export type WorldFact =
  | { fact: "delivery_failed"; id: string }
  | { fact: "desk_taken"; by: Address }
  | { fact: "desk_outside" }
  | { fact: "desk_free"; by: Address }
  | { fact: "moved"; to?: string }
  | { fact: "focus"; focus: Focus };

/** Who the World records as having stopped a job the user stopped. */
export const STOPPED_BY_USER = "the user";

export type Happened =
  | { event: "turn_started"; turn: string }
  | {
      event: "heard";
      delivery: NoticeId | null;
      turn: string | null;
      from: Speaker | null;
      text: string;
      message?: UserMessage;
      /** The World's telling or a colleague's answer, as facts. */
      notice?: Notice;
    }
  | { event: "began"; turn: string; item: Item }
  | { event: "grew"; revision?: number; turn: string; item: string; part: Part; text: string }
  | { event: "finished"; turn: string; item: Item }
  | { event: "updated"; turn: string; item: Item }
  | { event: "changed"; turn: string | null; change: SessionChange }
  | { event: "steps"; turn: string; steps: Step[]; note: string | null }
  | { event: "context"; used: number; window: number | null }
  | { event: "titled"; title: string }
  | { event: "turn_ended"; turn: string; end: TurnEnd }
  | { event: "health"; turn: string | null; health: Health }
  | { event: "delivery_uncertain"; delivery: NoticeId; why: string }
  | { event: "realtime"; item: JsonValue; complete: boolean }
  | { event: "realtime_grew"; item: string; text: string };

export type SessionEvent = { at: Timestamp } & Happened;

export type Watched =
  | { kind: "history"; events: SessionEvent[] }
  | { kind: "live"; event: SessionEvent }
  | { kind: "unchanged"; version: string }
  | { kind: "segments"; version: string; layout: string[]; segments: { id: string; events: SessionEvent[] }[]; complete: boolean };
