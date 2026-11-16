// A machine's files (`gui-files`): its folders as a tree on one side, the
// file opened on the other. The tree shows everything the machine lists,
// or only what agents changed; the changed files are marked in both, so
// the page is where "what was changed here" and "what is there" are one
// look. It is come into from a changed file anywhere (the activity page, a
// conversation), which it opens at once among its folders, and from a
// machine's own page.
//
// The folders are read from the machine one at a time as they are opened;
// its changes are kept by their ids and read again now and then, a reading
// adding only what is new. On a narrow window the tree and the file take
// the page in turn.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Page } from "../app/shell";
import { fileRow, figures } from "./changed";
import { Changes, fileKey, filesOf, plain, split, tally, within, type ChangedFile, type FileChange } from "./model";
import { FileTree, type Listing, type TreeMode } from "./tree";
import { FileView, type ViewSeams, type ViewTab } from "./view";
import type { FilesWords } from "./words";
import "./files.css";

export interface FilesMachine {
  name: string;
  online: boolean;
}

/** Where a machine's tree can start: its home, and its disks. */
export interface Places {
  home: string | null;
  places: { name: string; path: string }[];
}

export interface FilesPageOptions {
  words: FilesWords;
  head(title: string, actions?: string): string;
  machine: string;
  path: string | null;
  tab: ViewTab | null;
  /** The folder its tree starts at, where no file is asked for. */
  folder?: string | null | undefined;
  machines(): FilesMachine[];
  places(machine: string): Promise<Places>;
  /** A folder's listing; none where machines cannot list their files. */
  list: ((machine: string, path: string) => Promise<Listing>) | null;
  /** A machine's changes of late: all of them, or what came since it was
   *  last asked; either fills the same book. */
  changes(machine: string): Promise<FileChange[]>;
  seams: ViewSeams;
  /** The address of a machine's files with one opened. */
  link(machine: string, path: string | null, tab: ViewTab | null): string;
  /** The page moved within itself: the window's address follows. */
  moved(to: string): void;
  now(): number;
}

/** How often a machine's changes are read again while its page is shown. */
const AGAIN_MS = 8000;
/** Machines from which the list is searched rather than read. */
const FIND_FROM = 9;
/** The files offered where none is opened. */
const LATELY = 12;

export class FilesPage implements Page {
  readonly element = document.createElement("section");
  readonly title: string;
  private machine: string;
  private path: string | null;
  private root: string | null = null;
  /** The folder the tree was asked to start at, where one was. */
  private folder: string | null;
  private mode: TreeMode;
  private places: Places = { home: null, places: [] };
  private readonly book = new Changes();
  private drawn = -1;
  private readonly tree: FileTree;
  private view: FileView | null = null;
  private readonly main: HTMLElement;
  private timer = 0;
  private left = false;
  /** Which machine's answers are still wanted. */
  private asked = 0;

  constructor(private readonly options: FilesPageOptions) {
    const { words } = options;
    this.title = words.title;
    this.machine = options.machine;
    this.path = options.path;
    this.folder = options.folder ?? null;
    this.mode = options.list === null || options.tab === "changes" ? "changed" : "all";
    this.element.className = "page fb-page";
    this.element.innerHTML = `${options.head(escape(words.title))}<div class="fb-body">
      <aside class="fb-side">
        <div class="fb-bar"><div class="fb-machine"></div><div class="segmented fb-modes" role="radiogroup" aria-label="${escape(words.tree.shows)}"></div></div>
        <nav class="fb-crumbs" aria-label="${escape(words.tree.places)}"></nav>
        <div class="fb-tree scroller"></div>
      </aside>
      <main class="fb-main"></main>
    </div>`;
    this.main = this.element.querySelector<HTMLElement>(".fb-main")!;
    this.tree = new FileTree({
      words,
      list: options.list ? (path) => options.list!(this.machine, path) : null,
      chosen: (path) => this.open(path, this.book.of(this.machine, path).length && this.mode === "changed" ? "changes" : null),
      detail: options.seams.detail,
    });
    this.element.querySelector(".fb-tree")!.append(this.tree.element);
    this.element.addEventListener("click", (event) => this.pressed(event));
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if (event.key === "Escape") this.shutMenu();
      if ((event.key === "Enter" || event.key === " ") && target.matches(".cf-row")) {
        event.preventDefault();
        target.click();
      }
    });
    this.element.addEventListener("input", (event) => {
      if ((event.target as HTMLElement).matches(".fb-find")) this.drawMenuList();
    });
    document.addEventListener("pointerdown", this.outside);
    this.drawBar();
    void this.enter(options.tab);
    this.timer = window.setInterval(() => void this.readChanges(), AGAIN_MS);
  }

  leave(): void {
    this.left = true;
    clearInterval(this.timer);
    document.removeEventListener("pointerdown", this.outside);
    this.tree.leave();
    this.view?.leave();
  }

  /** Another file of this page asked for from outside it; or, with no
   *  file, a folder for its tree to start at. */
  go(machine: string, path: string | null, tab: ViewTab | null, folder: string | null = null): void {
    this.folder = folder;
    if (machine !== this.machine) {
      this.machine = machine;
      this.path = path;
      void this.enter(tab);
      return;
    }
    if (path !== null) return this.open(path, tab, false);
    this.close();
    if (folder === null || this.root === null) return;
    this.mode = this.options.list === null ? "changed" : "all";
    this.root = folder;
    this.drawModes();
    this.drawCrumbs();
    this.tree.show(this.mode === "changed" ? null : folder, this.mode);
  }

  shows(machine: string): boolean {
    return machine === this.machine;
  }

  private readonly outside = (event: PointerEvent) => {
    if (!(event.target as HTMLElement).closest?.(".fb-machine")) this.shutMenu();
  };

  /** Comes onto the machine: where its tree starts, what was changed
   *  there, and the file asked for. */
  private async enter(tab: ViewTab | null): Promise<void> {
    const asked = (this.asked += 1);
    this.view?.leave();
    this.view = null;
    this.root = null;
    this.places = { home: null, places: [] };
    this.book.replace([]);
    this.drawn = -1;
    this.drawBar();
    this.drawCrumbs();
    this.tree.changed([]);
    this.tree.show(null, this.mode);
    this.drawMain();
    const [places] = await Promise.all([this.options.places(this.machine).catch(() => ({ home: null, places: [] }) as Places), this.readChanges()]);
    if (this.left || asked !== this.asked) return;
    this.places = places;
    this.root = this.rootFor(this.path);
    this.drawCrumbs();
    this.tree.show(this.mode === "changed" ? null : this.root, this.mode);
    if (this.path !== null) this.open(this.path, tab, false);
    else this.drawMain();
  }

  /** The place a tree showing `path` starts at: the machine's home where
   *  the file lies in it, else the disk holding it, else its own folder. */
  private rootFor(path: string | null): string | null {
    const { home, places } = this.places;
    if (path === null) return this.folder ?? home ?? places[0]?.path ?? null;
    if (home && within(home, path)) return home;
    const holding = places.filter((place) => within(place.path, path)).sort((a, b) => b.path.length - a.path.length)[0];
    return holding?.path ?? (split(path).folder || home);
  }

  private async readChanges(): Promise<void> {
    const machine = this.machine;
    let changes: FileChange[];
    try {
      changes = await this.options.changes(machine);
    } catch {
      return;
    }
    if (this.left || machine !== this.machine) return;
    this.book.take(changes);
    if (this.book.revision === this.drawn) return;
    this.drawn = this.book.revision;
    const files = this.files();
    this.tree.changed(files);
    if (this.path !== null) this.view?.update(this.book.of(machine, this.path));
    else this.drawMain();
    this.drawModes();
  }

  private files(): ChangedFile[] {
    return filesOf(this.book.all().filter((change) => change.machine === this.machine));
  }

  // The bar over the tree.

  private drawBar(): void {
    const online = this.options.machines().find((machine) => machine.name === this.machine)?.online !== false;
    this.element.querySelector<HTMLElement>(".fb-machine")!.innerHTML = `<button class="fb-pick" type="button" aria-haspopup="listbox" aria-expanded="false" title="${escape(this.options.words.page.machines)}">${glyph("desktop", 16)}<b>${escape(this.machine)}</b>${
      online ? "" : `<small>${escape(this.options.words.page.offline)}</small>`
    }${glyph("chevron", 12)}</button>`;
    this.drawModes();
  }

  private drawModes(): void {
    const { words, list } = this.options;
    const modes = this.element.querySelector<HTMLElement>(".fb-modes")!;
    const count = this.files().length;
    const mode = (id: TreeMode, said: string, off = false) => `<button type="button" class="segment${id === this.mode ? " chosen" : ""}" role="radio" aria-checked="${id === this.mode}" data-mode="${id}"${off ? " disabled" : ""}>${said}</button>`;
    modes.innerHTML = `${mode("all", escape(words.tree.all), list === null)}${mode("changed", `${escape(words.tree.changed)}${count ? `<small>${count}</small>` : ""}`)}`;
  }

  /** Where the tree starts: the machine's home, or one of its disks. */
  private drawCrumbs(): void {
    const crumbs = this.element.querySelector<HTMLElement>(".fb-crumbs")!;
    const { words } = this.options;
    if (this.mode === "changed" || this.root === null) {
      crumbs.innerHTML = "";
      crumbs.hidden = true;
      return;
    }
    crumbs.hidden = false;
    const { home, places } = this.places;
    const starts = [...(home ? [{ name: words.tree.home, path: home, mark: "home" as const }] : []), ...places.map((place) => ({ ...place, mark: "disk" as const }))];
    const chips = starts.map((start) => `<button type="button" class="fb-place${plain(start.path) === plain(this.root!) ? " chosen" : ""}" data-root="${escape(start.path)}" title="${escape(start.path)}">${glyph(start.mark, 14)}<span>${escape(start.name)}</span></button>`).join("");
    crumbs.innerHTML = `<div class="fb-places">${chips}</div>`;
  }

  // The machines to choose among.

  private shutMenu(): void {
    this.element.querySelector(".fb-menu")?.remove();
    this.element.querySelector(".fb-pick")?.setAttribute("aria-expanded", "false");
  }

  private openMenu(): void {
    const holder = this.element.querySelector<HTMLElement>(".fb-machine")!;
    const machines = this.options.machines();
    const menu = document.createElement("div");
    menu.className = "fb-menu";
    menu.innerHTML = `${machines.length >= FIND_FROM ? `<input class="fb-find" type="search" placeholder="${escape(this.options.words.page.machines)}" aria-label="${escape(this.options.words.page.machines)}">` : ""}<div class="fb-menu-list scroller" role="listbox"></div>`;
    holder.append(menu);
    holder.querySelector(".fb-pick")!.setAttribute("aria-expanded", "true");
    this.drawMenuList();
    menu.querySelector<HTMLInputElement>(".fb-find")?.focus();
  }

  private drawMenuList(): void {
    const list = this.element.querySelector<HTMLElement>(".fb-menu-list");
    if (!list) return;
    const asked = (this.element.querySelector<HTMLInputElement>(".fb-find")?.value ?? "").trim().toLowerCase();
    const machines = this.options
      .machines()
      .filter((machine) => !asked || machine.name.toLowerCase().includes(asked))
      .sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
    list.innerHTML = machines
      .map((machine) => `<button type="button" role="option" aria-selected="${machine.name === this.machine}" class="fb-option${machine.name === this.machine ? " chosen" : ""}${machine.online ? "" : " away"}" data-machine="${escape(machine.name)}">${glyph("desktop", 16)}<span>${escape(machine.name)}</span>${machine.online ? "" : `<small>${escape(this.options.words.page.offline)}</small>`}</button>`)
      .join("");
  }

  // The file opened.

  private open(path: string, tab: ViewTab | null, told = true): void {
    const { options } = this;
    this.path = path;
    this.view?.leave();
    const changes = this.book.of(this.machine, path);
    this.view = new FileView({ words: options.words, seams: options.seams, machine: this.machine, path, changes, tab: tab ?? undefined, now: options.now });
    const back = document.createElement("button");
    back.type = "button";
    back.className = "text-button page-back fb-back";
    back.innerHTML = `${glyph("back", 18)}<span>${escape(options.words.tree.label)}</span>`;
    this.main.replaceChildren(back, this.view.element);
    this.element.classList.add("fb-reading");
    // A file outside the folder shown brings the tree to where it lies.
    if (this.mode === "all" && this.root !== null && !within(this.root, path)) {
      this.root = this.rootFor(path);
      this.drawCrumbs();
      this.tree.show(this.root, this.mode);
    }
    this.tree.select(path);
    if (told) options.moved(options.link(this.machine, path, tab));
  }

  private close(): void {
    this.path = null;
    this.view?.leave();
    this.view = null;
    this.element.classList.remove("fb-reading");
    this.tree.select(null);
    this.drawMain();
  }

  /** With no file opened: the files changed of late, to begin from. */
  private drawMain(): void {
    if (this.view) return;
    const { words } = this.options;
    const files = this.files()
      .sort((a, b) => (b.changes[0]?.at ?? 0) - (a.changes[0]?.at ?? 0))
      .slice(0, LATELY);
    if (!files.length) {
      this.main.innerHTML = `<div class="fb-empty"><p>${escape(words.page.choose)}</p></div>`;
      return;
    }
    const sum = tally(this.files());
    this.main.innerHTML = `<div class="fb-lately scroller"><p class="fb-lately-head"><b>${escape(words.page.lately)}</b><small>${escape(words.files(sum.files))}</small>${figures(sum.added, sum.removed)}</p><ul class="cf-files">${files
      .map((file) => fileRow(file, words, { attrs: `data-open="${escape(file.path)}"`, who: true, named: this.options.seams.named }))
      .join("")}</ul></div>`;
  }

  private pressed(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    if (target.closest(".fb-pick")) {
      if (this.element.querySelector(".fb-menu")) this.shutMenu();
      else this.openMenu();
      return;
    }
    const machine = target.closest<HTMLElement>("[data-machine]")?.dataset.machine;
    if (machine) {
      this.shutMenu();
      if (machine === this.machine) return;
      this.machine = machine;
      this.path = null;
      this.element.classList.remove("fb-reading");
      this.options.moved(this.options.link(machine, null, null));
      void this.enter(null);
      return;
    }
    const mode = target.closest<HTMLElement>("[data-mode]")?.dataset.mode as TreeMode | undefined;
    if (mode) {
      if (mode === this.mode) return;
      this.mode = mode;
      this.drawModes();
      this.drawCrumbs();
      this.tree.show(mode === "changed" ? null : this.root, mode);
      if (this.path !== null) this.tree.select(this.path);
      return;
    }
    const root = target.closest<HTMLElement>("[data-root]")?.dataset.root;
    if (root) {
      this.root = root;
      this.drawCrumbs();
      this.tree.show(root, this.mode);
      if (this.path !== null && within(root, this.path)) this.tree.select(this.path);
      return;
    }
    if (target.closest(".fb-back")) {
      this.close();
      this.options.moved(this.options.link(this.machine, null, null));
      return;
    }
    const open = target.closest<HTMLElement>("[data-open]")?.dataset.open;
    if (open) this.open(open, "changes");
  }
}

export { fileKey };
