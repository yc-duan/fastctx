// The account's sentinels (`world-senses.md`): where they live, the model
// they judge with, and the search the ones that may go online use. The
// settings are an account record on the hub; the keys never travel with
// them: only whether each is set comes back, and a key is handed to the
// machine the sentinels live on alone.

import { invoke } from "@tauri-apps/api/core";

export type ModelProtocol = "open_ai" | "anthropic";

export interface ModelSettings {
  protocol: ModelProtocol;
  /** The whole address, `/chat/completions` or `/messages` included. */
  endpoint: string;
  model: string;
  output_limit?: "max_completion_tokens" | "max_tokens";
}

export interface SearchSettings {
  provider: "tavily";
  endpoint: string;
}

export interface SentinelSettings {
  home: string | null;
  model: ModelSettings | null;
  search: SearchSettings | null;
}

export type HomeStatus =
  | { kind: "unset" | "forbidden" | "offline" | "ready" | "too_old" }
  | { kind: "unavailable"; reason: string };

export interface SentinelSettingsView {
  version: number | null;
  settings: SentinelSettings;
  home_status: HomeStatus;
  /** Whether each key is set on the home; none where that cannot be known. */
  keys: { model: boolean; search: boolean } | null;
  key_error: string | null;
}

export type SettingsReply = { kind: "view"; view: SentinelSettingsView } | { kind: "conflict"; current_version: number | null };

export interface SentinelSeam {
  read(hub: string, account: string): Promise<SentinelSettingsView>;
  write(hub: string, account: string, expected: number | null, settings: SentinelSettings): Promise<SettingsReply>;
  /** Hands a key to the machine the sentinels live on, and to no other. */
  setKey(hub: string, account: string, key: "model" | "search", value: string): Promise<void>;
}

export const sentinels: SentinelSeam = {
  read: (hub, account) => invoke<SentinelSettingsView>("sentinel_settings_read", { hub, account }),
  write: (hub, account, expected, settings) => invoke<SettingsReply>("sentinel_settings_write", { hub, account, change: { expected, settings } }),
  setKey: (hub, account, key, value) => invoke<void>("sentinel_key_set", { hub, account, key, value }),
};

export type SentinelPhase = "starting" | "watching" | "satisfied" | "unable" | "withdrawn" | "unavailable";

/** One sentinel a conversation left, as its home answered. */
export interface SentinelSummary {
  definition: {
    id: string;
    home: string;
    condition: string;
    kind: "local" | "web" | "goal";
    owner: { machine: string; resident: string };
    /** When it wakes its conversation, in milliseconds since the epoch. */
    at?: number[];
  };
  phase: SentinelPhase;
  /** How it watches, in its own words. */
  plan: string;
  latest: { at: number; text: string } | null;
  reason: string | null;
}

/** The sentinels the conversation at `owner` left, still watching. */
export function sentinelList(owner: { machine: string; resident: string }): Promise<SentinelSummary[]> {
  return invoke<{ kind: "listed"; sentinels: SentinelSummary[] }>("sentinel_list", { owner }).then((listed) => listed.sentinels);
}

/** Every sentinel of the accounts this device acts for: what a project's
 *  page draws as the eyes its conversations keep. */
export function sentinelListAll(): Promise<SentinelSummary[]> {
  return invoke<{ kind: "listed"; sentinels: SentinelSummary[] }>("sentinel_list", {}).then((listed) => listed.sentinels);
}

/** Withdraws one; its definition is gone even while its home has yet to confirm. */
export function sentinelStop(id: string, home: string): Promise<void> {
  return invoke("sentinel_stop", { id, home }).then(() => undefined);
}
