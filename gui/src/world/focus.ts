// The machines and folders the user points a conversation at
// (`fastctx_types::focus`): said to the agent as where its work is, never a
// limit on what it may use. Folders are chosen on the machine holding them,
// one level at a time, by name only.

import { invoke } from "@tauri-apps/api/core";
import type { MachineName } from "./wire";

export interface FocusedMachine {
  /** As written: its name, or `name.hub` where two hubs share a name. */
  machine: string;
  /** Absolute paths as that machine writes them; none for the machine as a
   *  whole. */
  folders: string[];
}

export interface Focus {
  machines: FocusedMachine[];
}

export interface Folder {
  /** Its name, or for a place a choice starts from, its whole path. */
  name: string;
  path: string;
  hidden?: boolean;
}

export interface Folders {
  machine: MachineName;
  /** The folder listed; null for the places a choice starts from. */
  path: string | null;
  /** The folder holding it; null at a root. */
  parent: string | null;
  home: string;
  folders: Folder[];
}

/** The folders in a folder of `machine`; with no `path`, its home and
 *  disks. */
export function folders(machine: MachineName, path: string | null): Promise<Folders> {
  return invoke<Folders>("folders", { machineName: machine, path });
}

export const noFocus = (): Focus => ({ machines: [] });

/** Whether two choices point at the same machines and folders, in order. */
export function sameFocus(a: Focus, b: Focus): boolean {
  return JSON.stringify(a.machines) === JSON.stringify(b.machines);
}

/** A folder's last name, as a label: `app` for `C:/code/app`, the path
 *  itself for a root. */
export function folderName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const name = trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1);
  // A drive's root, `D:\`, is named by its whole path.
  return name && !/^[A-Za-z]:$/.test(name) ? name : path;
}
