// The account's sentinels (`world-senses.md`): where they live, the model
// they judge with, and the search the ones that may go online use. The
// settings are an account record on the hub; the keys never travel with
// them: only whether each is set comes back, and a key is handed to the
// machine the sentinels live on alone.

import { invoke } from "@tauri-apps/api/core";
import type { SessionEvent, WokeWhy } from "./events";

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
    /** Which of its lives it is: its record is kept under it. */
    generation: number;
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

/** What a sentinel did, read from its home: each wake a turn. */
export interface SentinelRecord {
  version: string;
  /** As the version held: nothing is sent. */
  unchanged?: boolean;
  events: SessionEvent[];
  /** It woke more often than is sent; the earliest wakes are left out. */
  earlier?: boolean;
  /** How it stands as its home last kept it, ended or not. */
  summary?: SentinelSummary;
  /** When it next looks or is woken, in milliseconds since the epoch. */
  next?: number;
}

/** Its record, unless it is still as the version `held`. */
export function sentinelRecord(id: string, home: string, generation: number, held: string | null): Promise<SentinelRecord> {
  return invoke<{ kind: "record"; record: SentinelRecord }>("sentinel_record", { id, home, generation, held }).then((reply) => reply.record);
}

/** Why a sentinel woke (`fastctx_types::residents::Woke`). */
export type SentinelWoke = WokeWhy;

/** One time a sentinel woke, and what it took. */
export interface SentinelWake {
  /** In milliseconds since the epoch. */
  at: number;
  why: SentinelWoke;
  /** Tokens it read (cached ones included), wrote, and read from the cache. */
  input: number;
  output: number;
  cached: number;
  requests: number;
  elapsed_ms: number;
  failed?: boolean;
}

/** One sentinel as its home keeps it, ended or not, with its last days' wakes. */
export interface SentinelWatched {
  summary: SentinelSummary;
  wakes: SentinelWake[];
  earlier?: boolean;
}

export interface SentinelOverview {
  watched: SentinelWatched[];
  /** The homes that could not be asked, with why, in the kernel's words. */
  unavailable: string[];
}

/** Every sentinel of the accounts this device acts for, ended or not. */
export function sentinelOverview(): Promise<SentinelOverview> {
  return invoke<{ kind: "overview" } & SentinelOverview>("sentinel_overview").then(({ watched, unavailable }) => ({ watched, unavailable }));
}
