// The files agents changed (`gui-files`): one change of one file is the
// unit everything here is put together from, wherever it was learned of: a
// machine's activity book, a conversation's own calls. A file's changes
// over what is looked at are one row, its lines added and removed summed,
// as a list of changed files tells them everywhere; the rows gather by the
// machine holding them, or by the conversation that made them.
//
// A change names who made it (the session, its agent, the turn), so that
// every list can say so, and carries a way to read its lines only when it
// is opened. Changes are kept by their ids: a reading that brings some
// again changes nothing, and one that brings only what is new adds only
// that (`Changes`).

import type { DiffRow } from "../conversation/diff";

export type How = "added" | "changed" | "removed" | "copied";

/** One change of one file. */
export interface FileChange {
  /** Its own name among all changes, the same however often it is read. */
  id: string;
  /** Milliseconds since the epoch. */
  at: number;
  machine: string;
  path: string;
  how: How;
  added: number;
  removed: number;
  /** Its lines were not counted: bytes that are no text. */
  uncounted?: boolean;
  /** Seen changed in its repository after a command, not written through
   *  Fastctx: there are no lines of it to read. */
  seen: boolean;
  /** `resident@machine` of the session that made it. */
  session: string | null;
  agent: string | null;
  /** The machine its command was sent from, where that is another. */
  from: string | null;
  turn: string | null;
  /** Its lines, where they are kept; none where they never were. */
  rows?: (() => Promise<DiffRow[] | null>) | undefined;
}

/** A file changed, on its machine. */
export interface ChangedFile {
  /** Machine and path, a line between. */
  key: string;
  machine: string;
  path: string;
  /** Up to its last separator, that included; and what follows. */
  folder: string;
  name: string;
  /** What became of it: made, changed, gone, copied here. */
  how: How;
  added: number;
  removed: number;
  uncounted: boolean;
  /** Its changes, the latest first. */
  changes: FileChange[];
}

/** Files that stand together under one name. */
export interface Group {
  key: string;
  files: ChangedFile[];
  added: number;
  removed: number;
  /** When the latest of their changes was made. */
  last: number;
}

export interface MachineGroup extends Group {
  machine: string;
}

/** What one session changed. */
export interface TalkGroup extends Group {
  session: string | null;
  agent: string | null;
  /** The machines holding the files, in name order. */
  machines: string[];
}

export const fileKey = (machine: string, path: string) => `${machine}\n${path}`;

/** A path's folder, up to its last separator, and its name. */
export function split(path: string): { folder: string; name: string } {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1;
  return { folder: path.slice(0, cut), name: path.slice(cut) };
}

/** What became of a file whose changes, oldest first, were these: one made
 *  and still there is new; one whose last change removed it is gone; one
 *  only ever copied here is a copy; any other is changed. */
function became(hows: How[]): How {
  const last = hows[hows.length - 1]!;
  if (last === "removed") return "removed";
  if (hows[0] === "added") return "added";
  if (hows.every((how) => how === "copied")) return "copied";
  return "changed";
}

/** The files these changes changed, in the order of their machines and
 *  paths. */
export function filesOf(changes: readonly FileChange[]): ChangedFile[] {
  const files = new Map<string, ChangedFile>();
  for (const change of changes) {
    const key = fileKey(change.machine, change.path);
    let file = files.get(key);
    if (!file) files.set(key, (file = { key, machine: change.machine, path: change.path, ...split(change.path), how: change.how, added: 0, removed: 0, uncounted: false, changes: [] }));
    file.changes.push(change);
  }
  for (const file of files.values()) {
    file.changes.sort((a, b) => a.at - b.at);
    file.how = became(file.changes.map((change) => change.how));
    file.added = file.changes.reduce((sum, change) => sum + change.added, 0);
    file.removed = file.changes.reduce((sum, change) => sum + change.removed, 0);
    file.uncounted = file.changes.every((change) => change.uncounted === true);
    file.changes.reverse();
  }
  return [...files.values()].sort((a, b) => a.machine.localeCompare(b.machine) || a.path.localeCompare(b.path));
}

function gathered<T extends Group>(group: T): T {
  group.added = group.files.reduce((sum, file) => sum + file.added, 0);
  group.removed = group.files.reduce((sum, file) => sum + file.removed, 0);
  group.last = group.files.reduce((latest, file) => Math.max(latest, file.changes[0]?.at ?? 0), 0);
  return group;
}

/** The files by the machine holding them, the machines in name order. */
export function byMachine(changes: readonly FileChange[]): MachineGroup[] {
  const groups = new Map<string, MachineGroup>();
  for (const file of filesOf(changes)) {
    let group = groups.get(file.machine);
    if (!group) groups.set(file.machine, (group = { key: file.machine, machine: file.machine, files: [], added: 0, removed: 0, last: 0 }));
    group.files.push(file);
  }
  return [...groups.values()].map(gathered).sort((a, b) => a.machine.localeCompare(b.machine));
}

/** The files by the session that changed them, the one that changed
 *  something last first. A file two sessions changed stands under both,
 *  each with its own lines. */
export function byTalk(changes: readonly FileChange[]): TalkGroup[] {
  const sorted = new Map<string, FileChange[]>();
  for (const change of changes) {
    // What no session is known for is its machine's own, by the agent.
    const key = change.session ?? `?${change.agent ?? ""}@${change.machine}`;
    const held = sorted.get(key);
    if (held) held.push(change);
    else sorted.set(key, [change]);
  }
  const groups: TalkGroup[] = [];
  for (const [key, own] of sorted) {
    const files = filesOf(own);
    groups.push(gathered({ key, session: own[0]!.session, agent: own[0]!.agent, machines: [...new Set(files.map((file) => file.machine))].sort((a, b) => a.localeCompare(b)), files, added: 0, removed: 0, last: 0 }));
  }
  return groups.sort((a, b) => b.last - a.last);
}

/** How many files, and their lines. */
export function tally(files: readonly ChangedFile[]): { files: number; added: number; removed: number } {
  return { files: files.length, added: files.reduce((sum, file) => sum + file.added, 0), removed: files.reduce((sum, file) => sum + file.removed, 0) };
}

/** The sessions that changed a file, the latest first, each once. */
export function makers(file: ChangedFile): { session: string | null; agent: string | null; from: string | null }[] {
  const seen = new Set<string>();
  const out: { session: string | null; agent: string | null; from: string | null }[] = [];
  for (const change of file.changes) {
    const key = change.session ?? `?${change.agent ?? ""}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push({ session: change.session, agent: change.agent, from: change.from });
  }
  return out;
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

/** Changes kept by their ids, so that a reading adds only what it brings
 *  that is new: the same change read again is the one already held, and a
 *  feed that tells only what came since it was last asked fills the same
 *  book a whole reading would. `revision` moves when what is held does. */
export class Changes {
  private readonly held = new Map<string, FileChange>();
  revision = 0;

  /** Takes what a reading brought; true where it changed what is held. */
  take(changes: readonly FileChange[]): boolean {
    let moved = false;
    for (const change of changes) {
      const kept = this.held.get(change.id);
      if (kept && kept.at === change.at && kept.added === change.added && kept.removed === change.removed && kept.how === change.how) continue;
      this.held.set(change.id, change);
      moved = true;
    }
    if (moved) this.revision += 1;
    return moved;
  }

  /** Holds exactly these: for a source that can only be read whole. */
  replace(changes: readonly FileChange[]): boolean {
    const ids = new Set(changes.map((change) => change.id));
    let moved = false;
    for (const id of [...this.held.keys()]) {
      if (ids.has(id)) continue;
      this.held.delete(id);
      moved = true;
    }
    const took = this.take(changes);
    if (moved && !took) this.revision += 1;
    return moved || took;
  }

  all(): FileChange[] {
    return [...this.held.values()];
  }

  /** A file's changes, the latest first. */
  of(machine: string, path: string): FileChange[] {
    return this.all().filter((change) => change.machine === machine && change.path === path).sort((a, b) => b.at - a.at);
  }
}

/** A path as it is compared: separators one way, a drive's letter and what
 *  follows it without regard to case, as Windows reads them. */
export function plain(path: string): string {
  const slashed = path.replace(/\\/g, "/");
  return /^[A-Za-z]:/.test(slashed) ? slashed.toLowerCase() : slashed;
}

/** Whether `path` lies in the folder `root`, or is it. */
export function within(root: string, path: string): boolean {
  const base = plain(root).replace(/\/+$/, "");
  const at = plain(path);
  return at === base || at.startsWith(`${base}/`);
}

/** The folder all of `files` lie in, to be left out of their rows; empty
 *  where they share none, lie on several machines, or are a single file
 *  (whose folder says where it is). */
export function commonFolder(files: readonly { machine: string; folder: string }[]): string {
  if (files.length < 2 || files.some((file) => file.machine !== files[0]!.machine)) return "";
  let common = files[0]!.folder;
  for (const file of files) {
    while (common && !file.folder.startsWith(common)) {
      const cut = Math.max(common.lastIndexOf("/", common.length - 2), common.lastIndexOf("\\", common.length - 2));
      common = cut < 0 ? "" : common.slice(0, cut + 1);
    }
  }
  // A root alone, or a drive, says nothing worth leaving out.
  return common.split(/[\\/]/).filter(Boolean).length < 2 ? "" : common;
}
