// A machine's folders as a tree (`gui-files`): a folder opens under itself
// to what it holds, read from the machine only when it is opened; a file
// pressed is opened beside the tree. The files agents changed are marked
// where they stand, their lines after their names, and every folder
// holding one carries a dot, so what was changed is found from the top.
// Asked for the changed files only, the tree is put together from the
// changes alone, nothing read from the machine: folders that hold nothing
// but one folder are written on one row, as a list of changed files does.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { figures, mark } from "./changed";
import { plain, split, within, type ChangedFile } from "./model";
import type { FilesWords } from "./words";

export interface TreeEntry {
  name: string;
  path: string;
  kind: "folder" | "file";
  size?: number | undefined;
  /** Milliseconds since the epoch. */
  modified?: number | undefined;
  hidden?: boolean | undefined;
}

/** What a folder holds, as its machine lists it: folders first, each kind
 *  in name order. */
export interface Listing {
  path: string;
  parent: string | null;
  entries: TreeEntry[];
  /** How many more it holds than are listed. */
  more?: number | undefined;
}

export type TreeMode = "all" | "changed";

export interface TreeOptions {
  words: FilesWords;
  /** A folder's listing; none where the machine cannot list its files. */
  list: ((path: string) => Promise<Listing>) | null;
  /** A file pressed. */
  chosen(path: string): void;
  detail(error: unknown): string;
}

type Held = Listing | "reading" | { failed: string };

/** A folder of the tree put together from changes. */
interface Branch {
  /** What its row says: its own name, or several folders' on one row. */
  label: string;
  path: string;
  folders: Map<string, Branch>;
  files: ChangedFile[];
}

/** A path without what ends it. */
const bare = (path: string) => (path.length > 1 && /[\\/]$/.test(path) && !/^[A-Za-z]:[\\/]$/.test(path) ? path.slice(0, -1) : path);

export class FileTree {
  readonly element = document.createElement("div");
  private root: string | null = null;
  private mode: TreeMode = "all";
  private files: ChangedFile[] = [];
  /** Folders opened, by their plain paths; in the changed tree, those shut. */
  private readonly open = new Set<string>();
  private readonly shut = new Set<string>();
  private readonly listed = new Map<string, Held>();
  private selected: string | null = null;
  /** Folders just opened: what they hold comes in. */
  private fresh: string | null = null;
  /** Plain paths of the changed files, and of every folder holding one. */
  private marked = new Map<string, ChangedFile>();
  private holding = new Set<string>();
  private left = false;

  constructor(private readonly options: TreeOptions) {
    this.element.className = "ft-tree";
    this.element.setAttribute("role", "tree");
    this.element.setAttribute("aria-label", options.words.tree.label);
    this.element.addEventListener("click", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".ft-row");
      if (row) this.press(row);
    });
    this.element.addEventListener("keydown", (event) => this.key(event));
  }

  leave(): void {
    this.left = true;
  }

  /** Whether the folder it is to show is not read yet, or not yet known:
   *  it has no rows of its own so far. */
  get reading(): boolean {
    if (this.mode !== "all" || !this.options.list) return false;
    return this.root === null || (this.listed.get(plain(this.root)) ?? "reading") === "reading";
  }

  /** Shows the folder `root` (none: nothing yet), all it holds or only the
   *  files changed in it. */
  show(root: string | null, mode: TreeMode): void {
    const moved = root !== this.root;
    this.root = root;
    this.mode = mode;
    if (moved && root !== null && mode === "all") this.read(root);
    this.draw();
  }

  /** The machine's changed files, as they now stand. */
  changed(files: ChangedFile[]): void {
    this.files = files;
    this.marked = new Map(files.map((file) => [plain(file.path), file]));
    this.holding = new Set();
    for (const file of files) {
      let folder = bare(split(file.path).folder);
      while (folder) {
        const key = plain(folder);
        if (this.holding.has(key)) break;
        this.holding.add(key);
        const up = bare(split(folder).folder);
        if (up === folder) break;
        folder = up;
      }
    }
    this.draw();
  }

  /** Marks `path` as the file opened, its folders opened down to it. */
  select(path: string | null): void {
    this.selected = path === null ? null : plain(path);
    if (path !== null && this.root !== null && this.mode === "all" && within(this.root, path)) {
      let folder = bare(split(path).folder);
      const chain: string[] = [];
      while (folder && within(this.root, folder) && plain(folder) !== plain(this.root)) {
        chain.push(folder);
        const up = bare(split(folder).folder);
        if (up === folder) break;
        folder = up;
      }
      for (const at of chain) {
        this.open.add(plain(at));
        if (!this.listed.has(plain(at))) this.read(at);
      }
    }
    if (path !== null) for (const key of [...this.shut]) if (within(key, path)) this.shut.delete(key);
    this.draw();
    this.element.querySelector(".ft-row.chosen")?.scrollIntoView({ block: "nearest" });
  }

  private read(path: string): void {
    const key = plain(path);
    const list = this.options.list;
    if (!list) return;
    if (!this.listed.has(key)) this.listed.set(key, "reading");
    void list(path).then(
      (listing) => {
        if (this.left) return;
        this.listed.set(key, listing);
        this.draw();
        if (this.selected) this.element.querySelector(".ft-row.chosen")?.scrollIntoView({ block: "nearest" });
      },
      (error: unknown) => {
        if (this.left) return;
        this.listed.set(key, { failed: this.options.detail(error) });
        this.draw();
      },
    );
  }

  private press(row: HTMLElement): void {
    const path = row.dataset.path!;
    if (row.dataset.kind === "file") {
      this.options.chosen(path);
      return;
    }
    const key = plain(path);
    if (this.mode === "changed") {
      if (this.shut.has(key)) this.shut.delete(key);
      else this.shut.add(key);
    } else if (this.open.has(key)) this.open.delete(key);
    else {
      this.open.add(key);
      this.fresh = key;
      if (!this.listed.has(key) || typeof this.listed.get(key) !== "string") this.read(path);
    }
    this.draw(key);
    this.fresh = null;
  }

  private key(event: KeyboardEvent): void {
    const row = (event.target as HTMLElement).closest<HTMLElement>(".ft-row");
    if (!row) return;
    const rows = [...this.element.querySelectorAll<HTMLElement>(".ft-row")];
    const at = rows.indexOf(row);
    const folder = row.dataset.kind === "folder";
    const opened = row.getAttribute("aria-expanded") === "true";
    const go = (to: HTMLElement | undefined) => {
      if (!to) return;
      event.preventDefault();
      to.focus();
    };
    switch (event.key) {
      case "ArrowDown":
        return go(rows[at + 1]);
      case "ArrowUp":
        return go(rows[at - 1]);
      case "ArrowRight":
        if (folder && !opened) {
          event.preventDefault();
          this.press(row);
        } else if (folder) go(rows[at + 1]);
        return;
      case "ArrowLeft":
        if (folder && opened) {
          event.preventDefault();
          this.press(row);
        } else {
          const depth = Number(row.style.getPropertyValue("--depth"));
          go(rows.slice(0, at).reverse().find((before) => Number(before.style.getPropertyValue("--depth")) < depth));
        }
        return;
      case "Enter":
      case " ":
        event.preventDefault();
        this.press(row);
        return;
      default:
    }
  }

  /** Draws the rows; `focus` is a folder whose row keeps the keys. */
  private draw(focus?: string): void {
    const { words } = this.options;
    let html: string;
    if (this.mode === "changed") html = this.changedRows();
    else if (this.root === null) html = "";
    else if (!this.options.list) html = `<p class="ft-note">${escape(words.tree.unlisted)}</p>`;
    else html = this.rows(this.root, 0);
    this.element.innerHTML = html;
    const first = this.element.querySelector<HTMLElement>(".ft-row.chosen") ?? this.element.querySelector<HTMLElement>(".ft-row");
    if (first) first.tabIndex = 0;
    if (focus) {
      const row = [...this.element.querySelectorAll<HTMLElement>(".ft-row")].find((at) => plain(at.dataset.path!) === focus);
      if (row) {
        if (first) first.tabIndex = -1;
        row.tabIndex = 0;
        row.focus({ preventScroll: true });
      }
    }
  }

  private note(text: string, depth: number, title = ""): string {
    return `<p class="ft-note" style="--depth:${depth}"${title ? ` title="${escape(title)}"` : ""}>${escape(text)}</p>`;
  }

  /** What the folder at `path` holds, as rows `depth` deep. */
  private rows(path: string, depth: number): string {
    const { words } = this.options;
    const key = plain(path);
    const held = this.listed.get(key);
    if (held === undefined || held === "reading") return this.note(words.tree.reading, depth);
    if ("failed" in held) return this.note(words.tree.failed, depth, held.failed);
    if (!held.entries.length) return this.note(words.tree.empty, depth);
    const coming = this.fresh === key ? " ft-in" : "";
    const rows = held.entries.map((entry) => {
      const at = plain(entry.path);
      if (entry.kind === "folder") {
        const opened = this.open.has(at);
        return `${this.row({ kind: "folder", path: entry.path, name: entry.name, depth, opened, dot: this.holding.has(at), dim: !!entry.hidden, coming })}${opened ? this.rows(entry.path, depth + 1) : ""}`;
      }
      return this.row({ kind: "file", path: entry.path, name: entry.name, depth, file: this.marked.get(at), dim: !!entry.hidden, coming });
    });
    return `${rows.join("")}${held.more ? this.note(words.tree.more(held.more), depth) : ""}`;
  }

  private row(at: { kind: "folder" | "file"; path: string; name: string; depth: number; opened?: boolean; dot?: boolean; file?: ChangedFile | undefined; dim?: boolean; coming?: string }): string {
    const { words } = this.options;
    const chosen = at.kind === "file" && this.selected === plain(at.path);
    const file = at.file;
    const lead =
      at.kind === "folder"
        ? `<span class="ft-twist">${glyph("chevron", 16)}</span><span class="ft-icon">${glyph(at.opened ? "folder-open" : "folder", 16)}</span>`
        : `<span class="ft-twist"></span><span class="ft-icon">${file ? mark(file.how, words.how[file.how]) : glyph("file", 16)}</span>`;
    const tail = file ? (file.uncounted ? "" : figures(file.added, file.removed)) : at.dot ? `<span class="ft-dot" aria-hidden="true"></span>` : "";
    return `<div class="ft-row ft-${at.kind}${at.opened ? " open" : ""}${chosen ? " chosen" : ""}${file ? ` ft-${file.how}` : ""}${at.dim ? " ft-dim" : ""}${at.coming ?? ""}" role="treeitem" tabindex="-1" aria-level="${at.depth + 1}"${
      at.kind === "folder" ? ` aria-expanded="${!!at.opened}"` : ` aria-selected="${chosen}"`
    } data-kind="${at.kind}" data-path="${escape(at.path)}" style="--depth:${at.depth}" title="${escape(at.path)}">${lead}<span class="ft-name">${escape(at.name)}</span>${tail}</div>`;
  }

  /** The tree of the changed files alone. */
  private changedRows(): string {
    const { words } = this.options;
    const root = this.root;
    const files = root === null ? this.files : this.files.filter((file) => within(root, file.path));
    if (!files.length) return `<p class="ft-note">${escape(words.tree.unchanged)}</p>`;
    const top: Branch = { label: "", path: root ?? "", folders: new Map(), files: [] };
    const base = root === null ? null : bare(root);
    for (const file of files) {
      const folder = bare(file.folder);
      // The folders between the one shown and the file's own; with no
      // folder shown, all of them from the top.
      const rest = base === null ? folder : folder.slice(base.length);
      const parts = rest.split(/[\\/]/).filter(Boolean);
      const slash = file.path.includes("\\") && !file.path.includes("/") ? "\\" : "/";
      let branch = top;
      let walked = base ?? "";
      for (const part of parts) {
        walked = walked === "" && base === null && !rest.startsWith(slash) ? part : `${walked}${slash}${part}`;
        let next = branch.folders.get(part);
        if (!next) branch.folders.set(part, (next = { label: part, path: walked, folders: new Map(), files: [] }));
        branch = next;
      }
      branch.files.push(file);
    }
    return this.branchRows(top, 0);
  }

  private branchRows(branch: Branch, depth: number): string {
    const folders = [...branch.folders.values()].sort((a, b) => a.label.localeCompare(b.label)).map((folder) => {
      // A folder holding nothing but one folder shares its row with it.
      let at = folder;
      let label = folder.label;
      while (at.files.length === 0 && at.folders.size === 1) {
        at = at.folders.values().next().value!;
        label = `${label}/${at.label}`;
      }
      const opened = !this.shut.has(plain(at.path));
      return `${this.row({ kind: "folder", path: at.path, name: label, depth, opened })}${opened ? this.branchRows(at, depth + 1) : ""}`;
    });
    const files = [...branch.files].sort((a, b) => a.name.localeCompare(b.name)).map((file) => this.row({ kind: "file", path: file.path, name: file.name, depth, file }));
    return `${folders.join("")}${files.join("")}`;
  }
}
