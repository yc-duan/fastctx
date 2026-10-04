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

export type Controlled = { kind: "done"; said: string | null } | { kind: "forked"; resident: Address };

export interface CommandOffer {
  name: string;
  description: string;
  args: string | null;
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
  usage: Usage | null;
}
