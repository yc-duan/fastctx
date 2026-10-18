// The folders a machine searches for logins, as the agents page shows them
// under the agent's tab (`world-gui.md`, 智能体页): each folder a row with
// how its search goes, what it found of this agent below it written
// relative to it, and a way to take each up; then 「添加文件夹…」. A search
// shows what it finds as it goes; a long list shows its first rows and
// opens the rest on asking.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { folderName } from "../world/focus";
import type { Folder, FolderSearch, FoundIn } from "../world/logins";
import type { AgentLoginsWords } from "./agent-logins-words";

type Agent = "codex" | "claude";

/** Rows a folder shows before the rest is asked for. */
const SHOWN = 8;

export interface FoldersView {
  machine: string;
  agent: Agent;
  /** Whether the user may change what the machine searches. */
  mine: boolean;
  folders: Folder[];
  words: AgentLoginsWords;
  now: Date;
  when(at: Date, now: Date): string;
  /** Folders whose whole list is open. */
  open: ReadonlySet<string>;
  /** Directories being taken up, by their full path. */
  taking: ReadonlySet<string>;
  /** Rows just added, shown as new for a moment, each with when. */
  fresh: ReadonlyMap<string, number>;
  notes: ReadonlyMap<string, { text: string; detail: string }>;
  /** A folder is being chosen. */
  choosing: boolean;
}

/** The folders, and the way to choose another. */
export function foldersSection(view: FoldersView): string {
  const { words, folders, mine, machine } = view;
  const rows = folders.map((folder) => folderRow(view, folder)).join("");
  const add = mine
    ? `<button class="text-button quiet-button logins-add-open" type="button" data-act="add-folder" data-machine="${escape(machine)}"${view.choosing ? " disabled" : ""}>${glyph("plus", 14)}<span>${escape(words.addFolder)}</span></button>`
    : "";
  if (!rows && !add) return "";
  return `<div class="logins-folders">${rows ? `<h3 class="logins-group">${escape(words.folders)}</h3><ul class="logins-folder-list">${rows}</ul>` : ""}${add}</div>`;
}

/** The rest of a row's class, and its style, for a row marked new: its
 *  fading goes on from where it was however often the page is drawn. */
export function freshness(fresh: ReadonlyMap<string, number>, key: string): string {
  const at = fresh.get(key);
  return at === undefined ? `"` : ` fresh" style="animation-delay: -${Math.max(0, Date.now() - at)}ms"`;
}

/** What the folders found of `agent`. */
export function foundOf(folders: Folder[], agent: Agent): number {
  return folders.reduce((sum, folder) => sum + (folder.search?.found.filter((one) => one.agent === agent).length ?? 0), 0);
}

/** Whether any folder's search runs. */
export function searching(folders: Folder[]): boolean {
  return folders.some((folder) => folder.search?.stage.state === "running");
}

/** A path below `dir`, written as `dir` writes its own. */
export function joinPath(dir: string, path: string): string {
  const separator = dir.includes("\\") ? "\\" : "/";
  const below = separator === "\\" ? path.replace(/\//g, "\\") : path;
  return /[\\/]$/.test(dir) ? `${dir}${below}` : `${dir}${separator}${below}`;
}

function folderRow(view: FoldersView, folder: Folder): string {
  const { words, machine, mine } = view;
  const at = `data-machine="${escape(machine)}" data-dir="${escape(folder.dir)}"`;
  const name = folderName(folder.dir);
  const parent = parentOf(folder.dir);
  const found = (folder.search?.found ?? []).filter((one) => one.agent === view.agent);
  const note = view.notes.get(`f:${machine}:${folder.dir}`);
  const fresh = freshness(view.fresh, `f:${machine}:${folder.dir}`);
  const forget = mine
    ? `<button class="icon-button logins-folder-forget" type="button" data-act="forget" ${at} aria-label="${escape(words.forget)}" title="${escape(words.forget)}">${glyph("close", 14)}</button>`
    : "";
  // The whole path, and how far the last search went, a hover away.
  const stage = folder.search?.stage;
  const said = folder.search && stage && stage.state !== "running" ? words.searched(folder.search.directories, folder.search.unreadable, view.when(new Date(stage.at), view.now)) : "";
  const title = said ? `${folder.dir}\n${said}` : folder.dir;
  return `<li class="logins-folder${fresh}>
    <div class="logins-folder-head">
      <span class="logins-folder-mark">${glyph("folder", 16)}</span>
      <span class="logins-folder-what" title="${escape(title)}"><span class="logins-folder-name">${escape(name)}</span>${parent ? `<span class="logins-folder-where">${escape(parent)}</span>` : ""}</span>
      ${state(view, folder.search, found.length, at)}
      ${forget}
    </div>
    ${found.length ? foundRows(view, folder, found) : ""}
    ${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
  </li>`;
}

/** How the folder's search goes, in a few words: while it runs, how many
 *  folders it has seen and the way to stop it; once it ended, only what
 *  the list below does not say. */
function state(view: FoldersView, search: FolderSearch | undefined, found: number, at: string): string {
  const { words, mine } = view;
  if (!search) return `<span class="logins-folder-state">${escape(words.notSearched)}</span>`;
  const stage = search.stage;
  if (stage.state === "running") {
    const stop = mine ? `<button class="text-button quiet-button logins-folder-stop" type="button" data-act="stop-search" ${at}>${escape(words.stop)}</button>` : "";
    return `<span class="logins-folder-state running"><span class="pulse" aria-hidden="true"></span><span class="logins-folder-count">${escape(words.searching(search.directories))}</span></span>${stop}`;
  }
  if (stage.state === "failed") return `<span class="logins-folder-state wrong" title="${escape(stage.reason)}">${escape(words.searchFailed)}</span>`;
  if (stage.state === "stopped") return `<span class="logins-folder-state">${escape(words.stopped)}</span>`;
  return found ? "" : `<span class="logins-folder-state">${escape(words.noneBelow)}</span>`;
}

function foundRows(view: FoldersView, folder: Folder, found: FoundIn[]): string {
  const { words, machine, mine, agent } = view;
  const open = view.open.has(`${machine}:${folder.dir}`) || found.length <= SHOWN + 1;
  const shown = open ? found : found.slice(0, SHOWN);
  const rows = shown
    .map((one) => {
      const full = joinPath(folder.dir, one.path);
      const taking = view.taking.has(full);
      const fresh = freshness(view.fresh, `d:${machine}:${full}`);
      const take = mine
        ? `<button class="text-button logins-apply" type="button" data-act="take" data-machine="${escape(machine)}" data-dir="${escape(full)}" data-folder="${escape(folder.dir)}"${taking ? " disabled" : ""}>${taking ? `<span class="pulse" aria-hidden="true"></span>` : ""}${escape(words.apply)}</button>`
        : "";
      return `<li class="logins-found-row${fresh}>${glyph(agent, 16)}<span class="logins-found-path" title="${escape(`${full}\n${one.traces.join("\n")}`)}">${escape(one.path)}</span>${take}</li>`;
    })
    .join("");
  const more = open
    ? ""
    : `<li><button class="text-button quiet-button logins-found-more" type="button" data-act="more" data-machine="${escape(machine)}" data-dir="${escape(folder.dir)}">${escape(words.moreFound(found.length - SHOWN))}</button></li>`;
  return `<ul class="logins-found">${rows}${more}</ul>`;
}

/** The folder holding `dir`, as it writes it; none for a root. */
function parentOf(dir: string): string {
  const trimmed = dir.replace(/[\\/]+$/, "");
  const at = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (at <= 0) return "";
  const parent = trimmed.slice(0, at);
  // `C:` is the root `C:\`.
  return /^[A-Za-z]:$/.test(parent) ? `${parent}\\` : parent;
}
