// The conversation's files, a page of the side (`gui-files`): the files it
// changed, as a list of changed files is read everywhere, each opening over
// the page to its changes; or, by the switch at its head, the folders it
// works in on each of its machines as trees, the changed files marked in
// them, any file opening on a tab to what it holds. Each machine's folder
// folds away under its head and opens again (the user, 2026-11-17: "每个
// 电脑得支持收起和展开"), kept so on this device; one folded as the page
// is drawn is not read until it is opened.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { figures, fileRow } from "../files/changed";
import { commonFolder, tally, within, type ChangedFile } from "../files/model";
import { FileTree, type Listing } from "../files/tree";
import type { ViewTab } from "../files/view";
import type { FilesWords } from "../files/words";
import type { SidePage } from "./pane";
import type { WhereShown } from "./types";

export interface SideFilesSeams {
  words: FilesWords;
  /** The machine the conversation's agent lives on: not said on its rows. */
  machine: string;
  /** A folder's listing on a machine; none where machines cannot list. */
  list: ((machine: string, path: string) => Promise<Listing>) | null;
  /** A machine's home: where its tree starts when the conversation names
   *  no folder there. */
  home(machine: string): Promise<string | null>;
  /** Opens a file over the page. */
  open(machine: string, path: string, tab: ViewTab | null): void;
  /** Goes to a machine's own page of files, at one of its folders; none
   *  where there is no such page to go to. */
  browse?: ((machine: string, folder: string | null) => void) | undefined;
  detail(error: unknown): string;
}

type Shows = "changes" | "folders";

/** Where this device keeps the folders folded away, and how many it keeps:
 *  the latest folded stay. */
const FOLDED = "side-files-folded";
const KEPT = 200;

function folded(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(FOLDED) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

/** The folder holding all of `files`: the one they share, a single file's
 *  own. */
function holding(files: ChangedFile[]): string {
  let common = files[0]!.folder;
  for (const file of files) {
    while (common && !file.folder.startsWith(common)) {
      const cut = Math.max(common.lastIndexOf("/", common.length - 2), common.lastIndexOf("\\", common.length - 2));
      common = cut < 0 ? "" : common.slice(0, cut + 1);
    }
  }
  return common.length > 1 ? common.replace(/[\\/]$/, "") : common;
}

export class SideFiles implements SidePage {
  readonly element = document.createElement("section");
  private shows: Shows = "changes";
  private files: ChangedFile[] = [];
  private where: WhereShown | null = null;
  /** The trees drawn, by machine and the folder each starts at. */
  private readonly trees = new Map<string, { machine: string; tree: FileTree }>();
  /** What the trees were last put together from. */
  private planted = "";
  private written = "";
  private readonly fold = folded();
  /** A folder's tree read for the first time: at once where its folder is
   *  open, else as it is first opened. */
  private readonly starts = new Map<string, () => void>();

  constructor(private readonly seams: SideFilesSeams) {
    const { words } = seams;
    this.element.className = "sf-page";
    this.element.innerHTML = `<header class="sf-head"><div class="segmented sf-shows" role="radiogroup" aria-label="${escape(words.side.shows)}"></div><span class="sf-sum"></span></header><div class="sf-changes"></div><div class="sf-folders" hidden></div>`;
    this.element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const shows = target.closest<HTMLElement>("[data-shows]")?.dataset.shows as Shows | undefined;
      if (shows) {
        if (shows === this.shows) return;
        this.shows = shows;
        this.draw();
        return;
      }
      const go = target.closest<HTMLElement>(".sf-root-go");
      if (go) return seams.browse?.(go.dataset.machine!, go.dataset.folder || null);
      const head = target.closest<HTMLElement>(".sf-root-head");
      if (head) return this.turn(head);
      const row = target.closest<HTMLElement>("[data-file]");
      if (row) seams.open(row.dataset.machine!, row.dataset.file!, "changes");
    });
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if ((event.key === "Enter" || event.key === " ") && target.matches(".cf-row")) {
        event.preventDefault();
        target.click();
      }
    });
    this.draw();
  }

  /** The files the conversation changed, as they now stand. */
  changed(files: ChangedFile[]): void {
    this.files = files;
    for (const { machine, tree } of this.trees.values()) tree.changed(files.filter((file) => file.machine === machine));
    this.draw();
  }

  /** Where the conversation works: the machines and folders of its trees. */
  place(where: WhereShown | null): void {
    this.where = where;
    if (this.shows === "folders") this.plant();
  }

  shown(): void {
    this.draw();
  }

  /** Folds a machine's folder away under its head, or opens it again,
   *  where it stands, as a card does. */
  private turn(head: HTMLElement): void {
    const section = head.closest<HTMLElement>(".sf-root")!;
    const key = section.dataset.root!;
    const shut = !this.fold.has(key);
    if (shut) this.fold.add(key);
    else this.fold.delete(key);
    try {
      localStorage.setItem(FOLDED, JSON.stringify([...this.fold].slice(-KEPT)));
    } catch {
      // Folding is this device's convenience; without storage it lasts the page.
    }
    if (!shut) this.start(key);
    section.classList.toggle("shut", shut);
    section.querySelector(":scope > .sh-fold")?.classList.toggle("open", !shut);
    head.setAttribute("aria-expanded", String(!shut));
  }


  private start(key: string): void {
    const start = this.starts.get(key);
    this.starts.delete(key);
    start?.();
  }

  private draw(): void {
    const { words, machine } = this.seams;
    const sum = tally(this.files);
    const segment = (id: Shows, said: string) => `<button type="button" class="segment${id === this.shows ? " chosen" : ""}" role="radio" aria-checked="${id === this.shows}" data-shows="${id}">${said}</button>`;
    this.element.querySelector<HTMLElement>(".sf-shows")!.innerHTML = `${segment("changes", `${escape(words.side.changes)}${sum.files ? `<small>${sum.files}</small>` : ""}`)}${segment("folders", escape(words.side.folders))}`;
    this.element.querySelector<HTMLElement>(".sf-sum")!.innerHTML = sum.files ? figures(sum.added, sum.removed) : "";
    const changes = this.element.querySelector<HTMLElement>(".sf-changes")!;
    const folders = this.element.querySelector<HTMLElement>(".sf-folders")!;
    changes.hidden = this.shows !== "changes";
    folders.hidden = this.shows !== "folders";
    if (this.shows === "folders") return this.plant();
    const under = commonFolder(this.files);
    const html = this.files.length
      ? `${under ? `<p class="sf-under" title="${escape(under)}"><bdi>${escape(under)}</bdi></p>` : ""}<ul class="cf-files">${this.files.map((file) => fileRow(file, words, { attrs: `data-file="${escape(file.path)}" data-machine="${escape(file.machine)}"`, where: file.machine !== machine, under })).join("")}</ul>`
      : `<p class="sf-none">${escape(words.side.none)}</p>`;
    if (html === this.written) return;
    this.written = html;
    changes.innerHTML = html;
  }

  /** Puts a tree for each folder the conversation works in, on each of its
   *  machines; where it names none on a machine, the machine's home. With
   *  no way to list a machine's files, the changed files alone are the
   *  tree. */
  private plant(): void {
    const { words, list } = this.seams;
    const holder = this.element.querySelector<HTMLElement>(".sf-folders")!;
    const machines = this.where?.machines.length ? this.where.machines.map((machine) => ({ name: machine.name, folders: machine.folders })) : [{ name: this.seams.machine, folders: [] as string[] }];
    // Machines it changed files on without being pointed at them.
    for (const file of this.files) if (!machines.some((machine) => machine.name === file.machine)) machines.push({ name: file.machine, folders: [] });
    const sign = JSON.stringify([machines, list === null]);
    if (sign === this.planted) return;
    this.planted = sign;
    for (const { tree } of this.trees.values()) tree.leave();
    this.trees.clear();
    this.starts.clear();
    holder.replaceChildren();
    for (const machine of machines) {
      // Where it changed files outside the folders it was pointed at, the
      // folder holding those is a tree of its own.
      const outside = this.files.filter((file) => file.machine === machine.name && !machine.folders.some((folder) => within(folder, file.path)));
      const beyond = machine.folders.length && outside.length ? [holding(outside)] : [];
      const roots: (string | null)[] = list === null ? [null] : machine.folders.length ? [...machine.folders, ...beyond] : [null];
      for (const root of roots) {
        const key = `${machine.name}\n${root ?? ""}`;
        const shut = this.fold.has(key);
        const section = document.createElement("section");
        section.className = `sf-root${shut ? " shut" : ""}`;
        section.dataset.root = key;
        // Its head folds it; the mark at the head's end goes to the
        // machine's own page of files, at this folder.
        const browse = this.seams.browse ? `<button class="icon-button sf-root-go" type="button" data-machine="${escape(machine.name)}" aria-label="${escape(words.side.browse)}" title="${escape(words.side.browse)}">${glyph("expand", 16)}</button>` : "";
        section.innerHTML = `<div class="sf-root-top"><button class="sf-root-head" type="button" aria-expanded="${!shut}"><span class="sh-turn">${glyph("chevron", 18)}</span>${glyph("machine", 16)}<b>${escape(machine.name)}</b><span><bdi></bdi></span></button>${browse}</div><div class="sh-fold${shut ? "" : " open"}"><div class="sh-fold-inner"></div></div>`;
        const going = section.querySelector<HTMLElement>(".sf-root-go");
        const tree = new FileTree({ words, list: list ? (path) => list(machine.name, path) : null, chosen: (path) => this.seams.open(machine.name, path, null), detail: this.seams.detail });
        tree.changed(this.files.filter((file) => file.machine === machine.name));
        section.querySelector(".sh-fold-inner")!.append(tree.element);
        holder.append(section);
        const named = section.querySelector<HTMLElement>("bdi")!;
        const start = (folder: string | null) => {
          named.textContent = folder ?? "";
          named.parentElement!.title = folder ?? "";
          if (going) going.dataset.folder = folder ?? "";
          tree.show(folder, list === null ? "changed" : "all");
        };
        this.trees.set(key, { machine: machine.name, tree });
        // Its folder is said in its head at once; its tree is read once it
        // is open.
        const named0 = root ?? "";
        named.textContent = named0;
        named.parentElement!.title = named0;
        if (going) going.dataset.folder = named0;
        this.starts.set(key, () => {
          if (root !== null || list === null) start(root);
          else void this.seams.home(machine.name).then((home) => this.trees.get(key)?.tree === tree && start(home), () => start(null));
        });
        if (!shut) this.start(key);
      }
    }
  }
}
