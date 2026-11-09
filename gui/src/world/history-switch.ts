// History's switch (`world-history.md`): kept on the hub with each account,
// or, for a computer in no network, on the computer. Off until turned on.

import { invoke } from "@tauri-apps/api/core";
import { inShell } from "./bridge";

/** Whether history is on for an account at a hub, or, with no account,
 *  for this computer while it is in no network. */
export function readHistorySwitch(hub: string | null, account: string | null): Promise<boolean> {
  if (!inShell()) return Promise.resolve(specimenHistory());
  return invoke<{ on: boolean }>("history_switch_read", { hub, account }).then((read) => read.on);
}

/** Turns history on or off for an account, or for this computer alone. */
export function writeHistorySwitch(hub: string | null, account: string | null, on: boolean): Promise<void> {
  if (!inShell()) {
    specimenOn = on;
    return Promise.resolve();
  }
  return invoke("history_switch_write", { hub, account, on });
}

/** The specimen's switch: on, unless `?history=off` starts it off, and as
 *  last written. */
export function specimenHistory(): boolean {
  return (specimenOn ??= new URLSearchParams(location.search).get("history") !== "off");
}

let specimenOn: boolean | null = null;
