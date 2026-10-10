// What the user does to a conversation besides speaking into it, and what
// a conversation offers (`fastctx_types::session::control`).

import type { Address, JsonValue } from "./wire";

export type Control =
  /** `path` names one skill among skills of the same name, as offered. */
  | { do: "command"; name: string; args: string; path?: string }
  | { do: "compact" }
  | { do: "model"; model: string | null; effort: string | null }
  | { do: "rename"; name: string }
  | { do: "fork"; through: string | null }
  | { do: "rewind"; from: string }
  | { do: "answer"; request: JsonValue; answer: JsonValue };

export type ControlKind = Control["do"];

/** What came of a control: done as the host confirms it; carried out with
 *  no word from the host on whether it took (`reported`, its words shown as
 *  they are and nothing claimed); refused, in the host's words, with the
 *  model the host is left on where its writer changed it before failing;
 *  the model selector to open, with the host's catalog; or the fork. */
export type Controlled =
  | { kind: "done"; said: string | null }
  | { kind: "reported"; said: string | null }
  | { kind: "refused"; said: string; current_model?: string | null }
  | { kind: "model_menu"; models: ModelOffer[]; model: string | null; effort: string | null; service_tier: string | null }
  | { kind: "forked"; resident: Address };

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
  cost_usd: number | null;
  raw: JsonValue;
}

export interface Offers {
  controls: ControlKind[];
  commands: CommandOffer[];
  models: ModelOffer[];
  model: string | null;
  effort: string | null;
  /** The tier the next turn runs at, when one is chosen or reported. */
  service_tier: string | null;
  usage: Usage | null;
}
