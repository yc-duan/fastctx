// The files agents changed, put together from every machine's activity
// (`gui-activity-page`): each machine keeps a deed for every script `run`
// ran there, with the files it touched; a file's changes over the span are
// one row, its lines added and removed summed, as a list of changed files
// tells them.

import type { Deed, TouchHow } from "../world/book";

export const DAY_MS = 24 * 3_600_000;

/** A deed with the machine it ran on. */
export interface Placed extends Deed {
  machine: string;
}

/** Local midnight of the day holding `at`. */
export function dayOf(at: number): number {
  const date = new Date(at);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

/** One change of a file: when, by which run, how. */
export interface Change {
  at: number;
  deed: Placed;
  how: TouchHow;
  added: number;
  removed: number;
  /** Seen changed in its repository, rather than written by Fastctx. */
  seen: boolean;
}

/** A file changed in the span, on its machine. */
export interface FileChanged {
  machine: string;
  path: string;
  /** Up to its last separator, that included; and what follows. */
  folder: string;
  name: string;
  /** What became of it over the span: made, changed, gone, copied here. */
  how: TouchHow;
  added: number;
  removed: number;
  /** Its changes, the latest first. */
  changes: Change[];
}

export interface MachineChanged {
  machine: string;
  files: FileChanged[];
  added: number;
  removed: number;
}

/** What became of a file whose changes, oldest first, were these: one made
 *  in the span and still there is new; one whose last change removed it is
 *  gone; one only ever copied here is a copy; any other is changed. */
function became(hows: TouchHow[]): TouchHow {
  const last = hows[hows.length - 1]!;
  if (last === "removed") return "removed";
  if (hows[0] === "added") return "added";
  if (hows.every((how) => how === "copied")) return "copied";
  return "changed";
}

/** The files changed since `from`, by machine and then by path. */
export function changedFiles(deeds: Placed[], from: number): MachineChanged[] {
  const files = new Map<string, FileChanged>();
  for (const deed of deeds) {
    const at = Date.parse(deed.ended ?? deed.began);
    if (at < from) continue;
    for (const touched of deed.touched ?? []) {
      const key = `${deed.machine}\n${touched.path}`;
      let file = files.get(key);
      if (!file) {
        const cut = Math.max(touched.path.lastIndexOf("/"), touched.path.lastIndexOf("\\")) + 1;
        files.set(key, (file = { machine: deed.machine, path: touched.path, folder: touched.path.slice(0, cut), name: touched.path.slice(cut), how: touched.how, added: 0, removed: 0, changes: [] }));
      }
      file.changes.push({ at, deed, how: touched.how, added: touched.added ?? 0, removed: touched.removed ?? 0, seen: !!touched.seen });
    }
  }
  const machines = new Map<string, MachineChanged>();
  for (const file of files.values()) {
    file.changes.sort((a, b) => a.at - b.at);
    file.how = became(file.changes.map((change) => change.how));
    file.added = file.changes.reduce((sum, change) => sum + change.added, 0);
    file.removed = file.changes.reduce((sum, change) => sum + change.removed, 0);
    file.changes.reverse();
    let machine = machines.get(file.machine);
    if (!machine) machines.set(file.machine, (machine = { machine: file.machine, files: [], added: 0, removed: 0 }));
    machine.files.push(file);
    machine.added += file.added;
    machine.removed += file.removed;
  }
  const sorted = [...machines.values()].sort((a, b) => a.machine.localeCompare(b.machine));
  for (const machine of sorted) machine.files.sort((a, b) => a.path.localeCompare(b.path));
  return sorted;
}

/** The five squares beside a file's figures: how its lines divide between
 *  added and removed, a square for a fifth of them, grey where there are
 *  fewer lines than squares. */
export function squares(added: number, removed: number): ("add" | "del" | "none")[] {
  const total = added + removed;
  if (!total) return ["none", "none", "none", "none", "none"];
  const filled = Math.min(5, total);
  let green = Math.round((filled * added) / total);
  if (added && !green) green = 1;
  if (removed && green === filled) green = filled - 1;
  return [...Array<"add">(green).fill("add"), ...Array<"del">(filled - green).fill("del"), ...Array<"none">(5 - filled).fill("none")];
}
