// What the user does to a conversation besides speaking into it, and what
// a conversation offers (`fastctx_types::session::control`).

import type { Address, JsonValue } from "./wire";

export type Control =
  /** `path` names one skill among skills of the same name, as offered. */
  | { do: "command"; name: string; args: string; path?: string }
  | { do: "compact" }
  | { do: "model"; model: string | null; effort: string | null }
  | { do: "rename"; name: string }
  /** `side`: a side conversation, kept in this one's side panel. */
  | { do: "fork"; through: string | null; side?: boolean }
  | { do: "rewind"; from: string }
  | { do: "answer"; request: JsonValue; answer: JsonValue }
  /** Claude Code's own subagents, and the latest messages of one. */
  | { do: "agents" }
  | { do: "agent_messages"; agent_id: string }
  /** Withdraws a control still waiting for the turn under way to end. */
  | { do: "cancel_control"; id: string };

export type ControlKind = Control["do"];

/** What came of a control: done as the host confirms it; carried out with
 *  no word from the host on whether it took (`reported`, its words shown as
 *  they are and nothing claimed); refused, in the host's words, with the
 *  model the host is left on where its writer changed it before failing;
 *  the model selector to open, with the host's catalog; or the fork. */
export type Controlled =
  | { kind: "done"; said: string | null }
  /** `queued`: taken to do once the turn under way ends; its progress
   *  comes as the conversation's events (`PendingControl`). */
  | { kind: "reported"; said: string | null; queued?: string | null }
  | { kind: "refused"; said: string; current_model?: string | null }
  | { kind: "model_menu"; models: ModelOffer[]; model: string | null; effort: string | null; service_tier: string | null }
  | { kind: "forked"; resident: Address }
  /** The model and effort chosen for the next requests; what each request
   *  really used comes with its answer. */
  | { kind: "selected"; model: string | null; effort: string | null }
  /** Back to before a turn; the files it changed stay as they are. */
  | { kind: "rewound"; from: string | null; retained_messages: number; files_restored: boolean; compacted_turns_reachable: boolean }
  | { kind: "agents"; agents: JsonValue[] }
  | { kind: "agent_messages"; agent_id: string; messages: JsonValue[]; recent_only: boolean }
  /** A question asked on the side (`/btw`) and its answer, as the host gave it. */
  | { kind: "side_answer"; reply: JsonValue }
  /** The fork exists, but not all of it was carried over. */
  | { kind: "fork_incomplete"; resident: Address; why: string };

/** Whether a command takes words after its name. */
export type CommandArguments = "none" | "optional" | "required";

export interface CommandOffer {
  name: string;
  description: string;
  args: string | null;
  arguments: CommandArguments;
  source: string | null;
  /** Passed back unchanged with the command, where the host has one. */
  path?: string;
  /** The forms of its arguments a background session can carry out. */
  argument_forms?: string[];
}

export interface ModelOffer {
  id: string;
  name: string | null;
  description: string | null;
  efforts: string[];
  default_effort: string | null;
  default: boolean;
  service_tiers: ServiceTierOffer[];
  default_service_tier: string | null;
  /** The model's context window in tokens, when known. */
  context_window?: number | null;
}

/** A speed tier a model offers, such as Codex's Fast. */
export interface ServiceTierOffer {
  id: string;
  name: string;
  description: string;
}

export interface Usage {
  context: { used: number; window: number | null } | null;
  limits: { name: string; used_percent: number | null; resets_at: string | null }[];
  /** When the limits were read or last pushed; older nodes do not say. */
  limits_at?: string | null;
  cost_usd: number | null;
  raw: JsonValue;
}

/** A control the node took to do later, and where it stands. */
export type ControlState = "waiting" | "running" | "completed" | "failed" | "cancelled" | "uncertain";

export interface PendingControl {
  id: string;
  control: Control;
  at: string;
  state: ControlState;
  result?: Controlled | null;
  error?: string | null;
}

export interface Offers {
  /** The node takes controls to do once a turn ends, and their withdrawal. */
  control_queue?: boolean;
  /** The node tells each control in its conversation as it begins and ends:
   *  the conversation shows them, not the line beside the input. */
  controls_told?: boolean;
  /** Those it holds, waiting or being done. */
  pending_controls?: PendingControl[];
  controls: ControlKind[];
  commands: CommandOffer[];
  models: ModelOffer[];
  model: string | null;
  effort: string | null;
  /** The tier the next turn runs at, when one is chosen or reported. */
  service_tier: string | null;
  usage: Usage | null;
  /** The turns that can be gone back to or forked from, where the host
   *  says which; every turn where it says nothing. */
  checkpoints?: { turn: string; rewind: boolean; fork: boolean }[];
  history_limits?: { files_restored: boolean; compacted_turns_reachable: boolean; empty_prefix_allowed: boolean; complete_prefix: boolean };
  /** The host chose the model on its own for now, setting the choice aside. */
  automatic_model?: string;
}
