// How the machines of an account stand in syncing with a hub (design
// §28.9): the JSON form of `fastctx_types::sync_status`. What the hub last
// sampled rides in each hub's accounts (`synced`); the page asks for the
// whole of it, with what this computer sees itself. Times are milliseconds
// since the epoch, amounts bytes.

import { invoke } from "@tauri-apps/api/core";
import type { HubId } from "./accounts";

/** What an account's machines on a hub amount to, as last sampled. */
export interface AccountSync {
  account: string;
  /** Its machines that report, and those of them caught up. */
  devices: number;
  synced: number;
  /** How far behind the furthest behind is, in milliseconds. */
  behind?: number;
}

/** What kind of records is left to come. */
export type Part = "conversations" | "output" | "other";

/** One machine of the account, as the hub reckons it. */
export interface DeviceSync {
  machine: string;
  /** `computer`, `phone`, … */
  kind: string;
  online: boolean;
  /** Since when it is online, or offline. */
  since?: number;
  /** When it last reported; absent: it never did. */
  reported?: number;
  /** How far behind the latest change it stands, in milliseconds. */
  behind: number;
  /** Bytes of records left to come, by part; `about` when only about so. */
  left?: [Part, number][];
  about?: boolean;
  unpulled: number;
  /** The keys of some of the scopes it has not pulled whole, as it last
   *  named them; absent from a hub of before it told them. */
  unpulled_scopes?: string[];
  missing_objects: number;
  missing_bytes: number;
  unsent_frames: number;
  unsent_bytes: number;
  /** Bytes a second it took lately. */
  rate: number;
  synced: boolean;
}

/** How far behind a machine stood at `at`, and whether it was online. */
export interface LagPoint {
  machine: string;
  at: number;
  behind: number;
  online: boolean;
}

export interface HubTotals {
  conversations: number;
  files: number;
  file_bytes: number;
  record_bytes: number;
}

/** The account's sync status on a hub. */
export interface SyncStatus {
  at: number;
  hours: number;
  /** The step between points, in milliseconds. */
  step: number;
  devices: DeviceSync[];
  points: LagPoint[];
  totals: HubTotals;
}

/** What this computer sees itself, the hub reached or not. */
export interface LocalSync {
  machine: string;
  unsent_frames: number;
  unsent_bytes: number;
  /** Since when it is cut off from the hub, if it is. */
  cut_off?: number;
  /** Why, as its link to the hub last said. */
  why?: string;
  /** Why what it has not sent could not be counted. */
  unread?: string;
}

export interface SyncStatusReply {
  local: LocalSync;
  hub?: SyncStatus;
  /** Why the hub's part is missing. */
  unreached?: string;
}

/** How `account`'s machines stand with `hub` over the last `hours`; with
 *  `machine`, that machine's line in full, the others' left out. */
export function syncStatus(hub: HubId, account: string, hours: number, machine: string | null = null): Promise<SyncStatusReply> {
  return invoke("sync_status", { hub, account, hours, machine });
}

/** What the accounts logged in here amount to on every hub, summed: the
 *  machines that report and those of them caught up. */
export function syncedOf(hubs: { synced?: AccountSync[] }[] | undefined): { devices: number; synced: number } {
  let devices = 0;
  let synced = 0;
  for (const hub of hubs ?? []) {
    for (const one of hub.synced ?? []) {
      devices += one.devices;
      synced += one.synced;
    }
  }
  return { devices, synced };
}
