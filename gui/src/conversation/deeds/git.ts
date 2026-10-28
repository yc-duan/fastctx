// What git said of what it did, in the words it prints them: a commit made,
// the state of a working tree, a history, a push. Read only after a
// command that is that git command, so a line that merely looks like one
// is not taken for it.

import type { CommitDeed, FileState, Found, LogDeed, OutputLines, PushDeed, StatusDeed } from "./types";

const COMMIT = /^\[([^\]\s]+)(?: \(root-commit\))? ([0-9a-f]{7,40})\] (.*)$/;
const CHANGED = /^ (\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?/;

export function gitCommit(lines: OutputLines): Found<CommitDeed> | null {
  for (let index = 0; index < lines.length; index += 1) {
    const match = COMMIT.exec(lines[index] ?? "");
    if (!match) continue;
    const changed = CHANGED.exec(lines[index + 1] ?? "");
    return {
      at: index,
      deed: {
        kind: "commit",
        command: null,
        branch: match[1]!,
        hash: match[2]!,
        subject: match[3]!,
        files: changed ? Number(changed[1]) : null,
        added: changed ? Number(changed[2] ?? 0) : null,
        removed: changed ? Number(changed[3] ?? 0) : null,
      },
    };
  }
  return null;
}

const LONG_STATES: Record<string, FileState> = {
  modified: "modified",
  "new file": "added",
  deleted: "deleted",
  renamed: "renamed",
  typechange: "modified",
  "both modified": "conflict",
  "both added": "conflict",
  "deleted by us": "conflict",
  "deleted by them": "conflict",
  "added by us": "conflict",
  "added by them": "conflict",
};
const LONG_ENTRY = /^\t(modified|new file|deleted|renamed|typechange|both modified|both added|deleted by us|deleted by them|added by us|added by them):\s+(.+)$/;
const SHORT_ENTRY = /^([ MADRCUT?!])([ MADRCUT?!]) (.+)$/;

export function gitStatus(lines: OutputLines): Found<StatusDeed> | null {
  let at = -1;
  let branch: string | null = null;
  let untracked = false;
  // The short form's lines come together: the first line after them that is
  // not one ends them.
  let short = 0;
  const files: StatusDeed["files"] = [];
  const seen = new Set<string>();
  const add = (state: FileState, path: string) => {
    const key = `${state} ${path}`;
    if (seen.has(key)) return;
    seen.add(key);
    files.push({ state, path });
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line === null || line === undefined) continue;
    const on = /^(?:On branch (\S+)|HEAD detached at (\S+)|## ([^.\s]+))/.exec(line);
    if (on) {
      if (at < 0) at = index;
      branch = on[1] ?? on[2] ?? on[3] ?? null;
      continue;
    }
    if (/^(?:Changes to be committed|Changes not staged for commit|Unmerged paths):/.test(line)) {
      if (at < 0) at = index;
      untracked = false;
      continue;
    }
    if (line === "Untracked files:") {
      if (at < 0) at = index;
      untracked = true;
      continue;
    }
    if (/^nothing to commit/.test(line)) {
      if (at < 0) at = index;
      continue;
    }
    const long = LONG_ENTRY.exec(line);
    if (long) {
      add(LONG_STATES[long[1]!]!, long[2]!.replace(/ \(.*\)$/, ""));
      continue;
    }
    if (untracked && /^\t(.+)$/.test(line)) {
      add("untracked", line.slice(1));
      continue;
    }
    const entry = short >= 0 ? SHORT_ENTRY.exec(line) : null;
    if (entry && !(entry[1] === " " && entry[2] === " ")) {
      if (at < 0) at = index;
      short += 1;
      const [x, y] = [entry[1]!, entry[2]!];
      const state: FileState = x === "?" ? "untracked" : x === "U" || y === "U" || (x === "A" && y === "A") || (x === "D" && y === "D") ? "conflict" : x === "A" ? "added" : x === "D" || y === "D" ? "deleted" : x === "R" ? "renamed" : "modified";
      add(state, entry[3]!.replace(/^.* -> /, ""));
    } else if (short > 0) short = -1;
  }
  if (at < 0) return null;
  return { at, deed: { kind: "status", command: null, branch, files } };
}

const ONELINE = /^(?:[*|\\/ ]+)?([0-9a-f]{7,40}) (?:\([^)]*\) )?(.+)$/;
const FULL = /^commit ([0-9a-f]{40})\b/;

export function gitLog(lines: OutputLines): Found<LogDeed> | null {
  const commits: LogDeed["commits"] = [];
  let at = -1;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    const full = FULL.exec(line);
    if (full) {
      if (at < 0) at = index;
      // Its subject is the first indented line after its heading.
      let subject = "";
      for (let next = index + 1; next < lines.length && next < index + 8; next += 1) {
        const said = lines[next] ?? "";
        if (FULL.test(said)) break;
        if (/^ {4}\S/.test(said)) {
          subject = said.trim();
          break;
        }
      }
      commits.push({ hash: full[1]!.slice(0, 8), subject });
      continue;
    }
    const oneline = ONELINE.exec(line);
    if (oneline && (at < 0 || commits.length)) {
      if (at < 0) at = index;
      commits.push({ hash: oneline[1]!.slice(0, 8), subject: oneline[2]! });
    }
  }
  if (!commits.length) return null;
  return { at, deed: { kind: "log", command: null, commits } };
}

const TO = /^To (\S+)$/;
const REF = /^ ([ +\-*!=t]) (\[[^\]]+\]|\S+)\s+(\S+) -> (\S+)(?: \((.+)\))?$/;

export function gitPush(lines: OutputLines): Found<PushDeed> | null {
  let at = -1;
  let remote: string | null = null;
  let current = false;
  const refs: PushDeed["refs"] = [];
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    const to = TO.exec(line);
    if (to) {
      if (at < 0) at = index;
      remote = plainRemote(to[1]!);
      continue;
    }
    if (line === "Everything up-to-date") {
      if (at < 0) at = index;
      current = true;
      continue;
    }
    const ref = remote !== null ? REF.exec(line) : null;
    if (ref) refs.push({ from: ref[3]!, to: ref[4]!, ok: ref[1] !== "!", why: ref[1] === "!" ? (ref[5] ?? ref[2]!.replace(/^\[|\]$/g, "")) : null });
  }
  if (at < 0) return null;
  return { at, deed: { kind: "push", command: null, remote, refs, current } };
}

/** A remote as a person names it: its host and path, without the user it
 *  logs in as or the `.git` at its end. */
function plainRemote(url: string): string {
  return url
    .replace(/^[a-z+]+:\/\//, "")
    .replace(/^[^@/]+@/, "")
    .replace(/:(?=[^/\d])/, "/")
    .replace(/\.git$/, "");
}
