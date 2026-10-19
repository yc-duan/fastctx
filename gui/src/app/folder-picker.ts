// Choosing a folder on a machine, as a window of its own, laid out the way
// every system's folder picker is (`world-gui.md`, 新对话): the machine's
// places to start from down the side (its home and disks, and any places
// already named on it), the path across the top, a step at a time and each
// step pressable, or typed straight in; the folders of where it stands in
// the middle; at the foot, what will be chosen, and the choice. The folders
// are that machine's, read a level at a time by its own node, names only.
//
// With a pointer a folder is chosen with a press and opened with two, or
// with Enter, the arrows walking the list and Backspace going up a level;
// under a finger a tap opens a folder, as phones' pickers do, and the
// button at the foot takes the folder the walk stands in.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { composing } from "../typing";
import type { Folder, Folders } from "../world/focus";
import { Sheet } from "./confirm";

export interface FolderPickerWords {
  title(machine: string): string;
  close: string;
  /** The machine's home, among its places to start from. */
  home: string;
  up: string;
  reading: string;
  noFolders: string;
  /** Where a folder's path can be typed. */
  path: string;
  /** A folder that could not be read; the machine's own words go under the pointer. */
  unreadable: string;
  /** A folder macOS keeps private, and going to allow Fastctx in. */
  privacy: string;
  privacyGo: string;
  cancel: string;
  choose: string;
}

export interface FolderPickerOptions {
  /** The machine, as the page names it. */
  machine: string;
  folders(path: string | null): Promise<Folders>;
  chosen(path: string): void;
  /** Places already named on this machine, shown with its home and disks. */
  marks?: { name: string; path: string }[];
  /** Where it opens; the machine's home where none is given. */
  start?: string | null;
  /** Goes to where Fastctx is allowed in: only where the machine is this one. */
  allow?: () => void;
}

const coarse = () => matchMedia("(pointer: coarse)").matches;

/** A path's steps from its root, each with the path it stands for:
 *  `C:/Users/ada` is C:, Users and ada; `/srv/mark50` is /, srv and mark50. */
export function pathSteps(path: string): { name: string; path: string }[] {
  const sep = path.includes("\\") && !path.includes("/") ? "\\" : "/";
  const steps: { name: string; path: string }[] = [];
  let parts = path.split(/[\\/]+/).filter(Boolean);
  let at = "";
  if (/^[\\/]{2}/.test(path) && parts.length >= 2) {
    // A share: its server and name are its root.
    at = `${sep}${sep}${parts[0]}${sep}${parts[1]}`;
    steps.push({ name: at, path: at });
    parts = parts.slice(2);
  } else if (/^[\\/]/.test(path)) {
    at = sep;
    steps.push({ name: sep, path: sep });
  }
  for (const part of parts) {
    if (!at) at = /^[A-Za-z]:$/.test(part) ? `${part}${sep}` : part;
    else at = at.endsWith(sep) ? `${at}${part}` : `${at}${sep}${part}`;
    steps.push({ name: part, path: at });
  }
  return steps;
}

export class FolderPicker {
  private readonly sheet = new Sheet("folder-picker", true);
  private options: FolderPickerOptions | null = null;
  /** Where the walk stands: a folder, or null for the machine's places. */
  private path: string | null = null;
  private listing: Folders | null = null;
  /** The machine's places to start from, and its home among them. */
  private roots: Folder[] = [];
  private home: string | null = null;
  /** The folder pressed once, the one the foot's button takes. */
  private selected: string | null = null;
  private error: string | null = null;
  private keptPrivate = false;
  /** The latest ask; an answer to an earlier one is dropped. */
  private asks = 0;
  private bar!: HTMLElement;
  private crumbs!: HTMLElement;
  private field!: HTMLInputElement;
  private starts!: HTMLElement;
  private list!: HTMLElement;
  private target!: HTMLElement;
  private pick!: HTMLButtonElement;
  /** Typed letters, for going to the folder they begin. */
  private typed = "";
  private typedAt = 0;

  constructor(private readonly words: FolderPickerWords) {
    const body = this.sheet.body;
    body.addEventListener("click", (event) => this.click(event));
    body.addEventListener("dblclick", (event) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".fp-row");
      if (row?.dataset.path) this.go(row.dataset.path);
    });
    body.addEventListener("keydown", (event) => this.key(event));
    this.sheet.dialog.addEventListener("close", () => {
      this.options = null;
      this.asks += 1;
    });
  }

  /** Opens on `options.machine`; the folder chosen goes to `options.chosen`
   *  as the window closes. */
  open(options: FolderPickerOptions): void {
    const { words } = this;
    this.options = options;
    this.path = null;
    this.listing = null;
    this.roots = [];
    this.home = null;
    this.selected = null;
    this.error = null;
    this.keptPrivate = false;
    this.sheet.body.innerHTML = `${this.sheet.head(words.title(options.machine), words.close)}
      <div class="fp-bar">
        <button type="button" class="icon-button fp-up" data-fp="up" aria-label="${escape(words.up)}" title="${escape(words.up)}">${glyph("back", 18)}</button>
        <div class="fp-where"><nav class="fp-crumbs" aria-label="${escape(words.path)}"></nav><input class="fp-path" type="text" hidden aria-label="${escape(words.path)}" placeholder="${escape(words.path)}" spellcheck="false" autocomplete="off" autocapitalize="off" enterkeyhint="go"></div>
      </div>
      <div class="fp-main"><nav class="fp-starts"></nav><div class="fp-list" role="listbox" aria-label="${escape(options.machine)}"></div></div>
      <div class="fp-foot"><span class="fp-target"></span><button type="button" class="text-button quiet-button fp-cancel" data-fp="cancel">${escape(words.cancel)}</button><button type="button" class="sheet-go fp-pick" data-fp="pick" disabled>${escape(words.choose)}</button></div>`;
    this.bar = this.sheet.body.querySelector(".fp-bar")!;
    this.crumbs = this.sheet.body.querySelector(".fp-crumbs")!;
    this.field = this.sheet.body.querySelector(".fp-path")!;
    this.starts = this.sheet.body.querySelector(".fp-starts")!;
    this.list = this.sheet.body.querySelector(".fp-list")!;
    this.target = this.sheet.body.querySelector(".fp-target")!;
    this.pick = this.sheet.body.querySelector(".fp-pick")!;
    this.field.addEventListener("blur", () => this.editing(false));
    this.draw();
    this.sheet.show();
    // Its places first, then where it opens: the folder asked for, else home.
    const asked = ++this.asks;
    options
      .folders(null)
      .then((places) => {
        if (asked !== this.asks) return;
        this.roots = places.folders;
        this.home = places.home;
        this.go(options.start ?? places.home ?? null);
      })
      .catch((error: unknown) => {
        if (asked !== this.asks) return;
        this.failed(error);
        this.draw();
      });
  }

  /** Asks the machine for the folders in `path`, or its places to start. */
  private go(path: string | null): void {
    const options = this.options;
    if (!options) return;
    const asked = ++this.asks;
    this.path = path;
    this.listing = null;
    this.selected = null;
    this.error = null;
    this.keptPrivate = false;
    this.editing(false);
    this.draw();
    if (path === null && this.roots.length) {
      this.listing = { machine: options.machine, path: null, parent: null, home: this.home ?? "", folders: this.roots };
      this.draw();
      this.list.querySelector<HTMLElement>(".fp-row")?.focus({ preventScroll: true });
      return;
    }
    options
      .folders(path)
      .then((listing) => {
        if (asked !== this.asks) return;
        this.listing = listing;
        this.path = listing.path;
      })
      .catch((error: unknown) => {
        if (asked !== this.asks) return;
        this.failed(error);
      })
      .finally(() => {
        if (asked !== this.asks) return;
        this.draw();
        if (!coarse()) this.list.focus({ preventScroll: true });
      });
  }

  private failed(error: unknown): void {
    const failure = typeof error === "object" && error !== null ? (error as { message?: unknown; code?: unknown }) : null;
    this.error = error instanceof Error ? error.message : typeof failure?.message === "string" ? failure.message : String(error);
    this.keptPrivate = failure?.code === "mac_privacy";
  }

  /** What the foot's button takes: the folder pressed, else the one the
   *  walk stands in. */
  private chosenPath(): string | null {
    return this.selected ?? this.listing?.path ?? null;
  }

  private draw(): void {
    const options = this.options;
    if (!options) return;
    const { words } = this;
    const listing = this.listing;
    // The path, a step at a time, from the machine itself.
    const here = listing?.path ?? this.path;
    const steps = here ? pathSteps(here) : [];
    const last = steps.length - 1;
    this.crumbs.innerHTML =
      `<button type="button" class="fp-crumb fp-machine${here ? "" : " current"}" data-fp="go" data-path="">${glyph("machine", 14)}<span>${escape(options.machine)}</span></button>` +
      steps
        .map(
          (step, index) =>
            `<span class="fp-sep" aria-hidden="true">${glyph("chevron", 14)}</span><button type="button" class="fp-crumb${index === last ? " current" : ""}" data-fp="go" data-path="${escape(step.path)}"${index === last ? ' aria-current="location"' : ""}>${escape(step.name)}</button>`,
        )
        .join("");
    // Keep the end of a long path in sight.
    this.crumbs.scrollLeft = this.crumbs.scrollWidth;
    this.bar.querySelector<HTMLButtonElement>(".fp-up")!.disabled = here === null;
    // The places to start from, the one stood in washed.
    const start = (path: string, name: string, mark: Parameters<typeof glyph>[0]) =>
      `<button type="button" class="fp-start${path === here ? " current" : ""}" data-fp="go" data-path="${escape(path)}" title="${escape(path)}">${glyph(mark, 16)}<span>${escape(name)}</span></button>`;
    const marks = (options.marks ?? []).map((mark) => start(mark.path, mark.name, "folder"));
    const roots = this.roots.map((root) => (root.path === this.home ? start(root.path, words.home, "home") : start(root.path, root.name.replace(/[\\/]+$/, "") || root.name, "disk")));
    this.starts.innerHTML = [...roots, ...marks].join("");
    // The folders where it stands.
    let rows: string;
    if (this.keptPrivate) {
      const go = options.allow ? `<button type="button" class="text-button quiet-button" data-fp="allow">${escape(words.privacyGo)}</button>` : "";
      rows = `<div class="fp-note" title="${escape(this.error ?? "")}"><span>${escape(words.privacy)}</span>${go}</div>`;
    } else if (this.error) rows = `<p class="fp-note" title="${escape(this.error)}">${escape(words.unreadable)}</p>`;
    else if (!listing) rows = `<p class="fp-note quiet">${escape(words.reading)}</p>`;
    else if (!listing.folders.length) rows = `<p class="fp-note quiet">${escape(words.noFolders)}</p>`;
    else
      rows = listing.folders
        .map((folder) => {
          const mark = listing.path === null ? (folder.path === this.home ? "home" : "disk") : "folder";
          const name = listing.path === null && folder.path === this.home ? words.home : folder.name;
          const on = folder.path === this.selected;
          return `<button type="button" class="fp-row${folder.hidden ? " quiet" : ""}${on ? " chosen" : ""}" role="option" aria-selected="${on}" data-path="${escape(folder.path)}" title="${escape(folder.path)}" tabindex="-1">${glyph(mark, 16)}<span class="fp-name">${escape(name)}</span></button>`;
        })
        .join("");
    this.list.innerHTML = rows;
    this.list.tabIndex = listing?.folders.length ? 0 : -1;
    const chosen = this.chosenPath();
    this.target.textContent = chosen ?? "";
    this.target.title = chosen ?? "";
    this.pick.disabled = !chosen;
  }

  /** Marks one folder as the one the foot takes. */
  private select(path: string | null, focus = true): void {
    this.selected = path;
    for (const row of this.list.querySelectorAll<HTMLElement>(".fp-row")) {
      const on = row.dataset.path === path;
      row.classList.toggle("chosen", on);
      row.setAttribute("aria-selected", String(on));
      if (on && focus) {
        row.focus({ preventScroll: true });
        row.scrollIntoView({ block: "nearest" });
      }
    }
    const chosen = this.chosenPath();
    this.target.textContent = chosen ?? "";
    this.target.title = chosen ?? "";
    this.pick.disabled = !chosen;
  }

  /** The path across the top turned into a field to type in, or back. */
  private editing(on: boolean): void {
    if (!this.field) return;
    if (on === !this.field.hidden) return;
    this.field.hidden = !on;
    this.crumbs.hidden = on;
    if (!on) return;
    this.field.value = this.listing?.path ?? this.path ?? "";
    this.field.focus();
    this.field.select();
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    const row = target.closest<HTMLElement>(".fp-row");
    if (row?.dataset.path) {
      // Under a finger a tap opens it; with a pointer a press chooses it.
      if (coarse()) this.go(row.dataset.path);
      else this.select(row.dataset.path);
      return;
    }
    const act = target.closest<HTMLButtonElement>("[data-fp]");
    if (act && !act.disabled) {
      switch (act.dataset.fp) {
        case "go":
          this.go(act.dataset.path || null);
          return;
        case "up":
          this.go(this.listing?.parent ?? null);
          return;
        case "cancel":
          this.sheet.close();
          return;
        case "allow":
          this.options?.allow?.();
          return;
        case "pick": {
          const chosen = this.chosenPath();
          const options = this.options;
          if (!chosen || !options) return;
          this.sheet.close();
          options.chosen(chosen);
          return;
        }
      }
    }
    // A press on the path's empty room makes it a field to type in.
    if (target === this.crumbs || target.classList.contains("fp-where")) this.editing(true);
  }

  private key(event: KeyboardEvent): void {
    if (composing(event)) return;
    const target = event.target as HTMLElement;
    if (target === this.field) {
      if (event.key === "Enter") {
        // Never the form's own submit: the window stays open.
        event.preventDefault();
        const typed = this.field.value.trim();
        if (typed) this.go(typed);
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        this.editing(false);
      }
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "l") {
      event.preventDefault();
      this.editing(true);
      return;
    }
    if (!this.list.contains(target)) return;
    const rows = [...this.list.querySelectorAll<HTMLElement>(".fp-row")];
    const at = rows.findIndex((row) => row.dataset.path === this.selected);
    const move = (index: number) => {
      const row = rows[Math.max(0, Math.min(rows.length - 1, index))];
      if (row) this.select(row.dataset.path ?? null);
    };
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        move(at + 1);
        return;
      case "ArrowUp":
        event.preventDefault();
        move(at < 0 ? 0 : at - 1);
        return;
      case "Home":
        event.preventDefault();
        move(0);
        return;
      case "End":
        event.preventDefault();
        move(rows.length - 1);
        return;
      case "Enter":
        event.preventDefault();
        if (this.selected) this.go(this.selected);
        return;
      case "Backspace":
        event.preventDefault();
        if (this.path !== null) this.go(this.listing?.parent ?? null);
        return;
    }
    // Letters typed go to the first folder they begin.
    if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const now = performance.now();
      this.typed = now - this.typedAt < 700 ? this.typed + event.key.toLowerCase() : event.key.toLowerCase();
      this.typedAt = now;
      const found = rows.find((row) => (row.querySelector(".fp-name")?.textContent ?? "").toLowerCase().startsWith(this.typed));
      if (found) this.select(found.dataset.path ?? null);
    }
  }
}
