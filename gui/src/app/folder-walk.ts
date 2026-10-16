// Walking one machine's folders to choose one (`world-gui.md`): the places
// a choice starts from (its home and disks), then each folder's folders a
// level at a time, by name only, read by that machine's node; or a path
// typed straight in. Serves the machines and folders a conversation is
// pointed at, and the folders the agents page searches for logins. A folder
// macOS keeps private from Fastctx says so, and on that Mac itself goes to
// where it is allowed.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Folders } from "../world/focus";

export interface FolderWalkWords {
  up: string;
  reading: string;
  noFolders: string;
  /** Where a folder's path can be typed. */
  path: string;
  choose: string;
  back: string;
  /** A folder macOS keeps private, and going to allow Fastctx in. */
  privacy: string;
  privacyGo: string;
}

export interface FolderWalkOptions {
  words: FolderWalkWords;
  /** The machine, as its head names it. */
  machine: string;
  folders(path: string | null): Promise<Folders>;
  chosen(path: string): void;
  /** Back where the walk was opened from; none where the walk is all there
   *  is, as in a sheet of its own. */
  back?: () => void;
  /** Goes to where Fastctx is allowed in: only where the machine walked is
   *  this one. */
  allow?: () => void;
}

export class FolderWalk {
  readonly element: HTMLElement;
  private path: string | null = null;
  private listing: Folders | null = null;
  private error: string | null = null;
  /** The folder failed is one macOS keeps private. */
  private keptPrivate = false;
  /** The latest ask; an answer to an earlier one is dropped. */
  private asks = 0;

  constructor(private readonly options: FolderWalkOptions) {
    this.element = document.createElement("div");
    this.element.className = "folder-walk";
    this.element.addEventListener("click", (event) => {
      const target = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-walk]");
      if (!target || target.disabled) return;
      switch (target.dataset.walk) {
        case "into":
          this.go(target.dataset.path ?? null);
          break;
        case "up":
          this.go(this.listing?.parent ?? null);
          break;
        case "back":
          this.options.back?.();
          break;
        case "pick":
          if (this.listing?.path) this.options.chosen(this.listing.path);
          break;
        case "allow":
          this.options.allow?.();
          break;
      }
    });
    // The walk may stand inside the input's own form, so a typed path goes
    // on Enter here and never submits anything.
    this.element.addEventListener("keydown", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>(".folder-walk-path");
      if (!input || event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      const typed = input.value.trim();
      if (typed) this.go(typed);
    });
  }

  /** Asks the machine for the folders in `path`, or its places to start. */
  go(path: string | null): void {
    const asked = ++this.asks;
    this.path = path;
    this.error = null;
    this.keptPrivate = false;
    this.listing = null;
    this.draw();
    this.options
      .folders(path)
      .then((listing) => {
        if (this.asks !== asked) return;
        this.listing = listing;
        this.path = listing.path;
      })
      .catch((error: unknown) => {
        if (this.asks !== asked) return;
        const failure = typeof error === "object" && error !== null ? (error as { message?: unknown; code?: unknown }) : null;
        this.error = error instanceof Error ? error.message : typeof failure?.message === "string" ? failure.message : String(error);
        this.keptPrivate = failure?.code === "mac_privacy";
      })
      .finally(() => {
        if (this.asks === asked) this.draw();
      });
  }

  private draw(): void {
    const { words, machine, back } = this.options;
    const listing = this.listing;
    let rows = "";
    if (this.path !== null) rows += `<button type="button" class="pop-row folder-walk-up" data-walk="up">${glyph("back", 14)}<span class="pop-label">${escape(words.up)}</span></button>`;
    if (this.keptPrivate) {
      const go = this.options.allow ? `<button type="button" class="text-button quiet-button" data-walk="allow">${escape(words.privacyGo)}</button>` : "";
      rows += `<div class="walk-privacy" title="${escape(this.error ?? "")}"><span>${escape(words.privacy)}</span>${go}</div>`;
    } else if (this.error) rows += `<span class="choice-none">${escape(this.error)}</span>`;
    else if (!listing) rows += `<span class="choice-none quiet">${escape(words.reading)}</span>`;
    else if (!listing.folders.length) rows += `<span class="choice-none quiet">${escape(words.noFolders)}</span>`;
    else
      rows += listing.folders
        .map(
          (folder) =>
            `<button type="button" class="pop-row${folder.hidden ? " quiet" : ""}" data-walk="into" data-path="${escape(folder.path)}">${glyph("folder", 16)}<span class="pop-label">${escape(folder.name)}</span></button>`,
        )
        .join("");
    const where = listing?.path ?? this.path ?? "";
    const head = back
      ? `<div class="folder-walk-head"><button type="button" class="icon-button" data-walk="back" aria-label="${escape(words.back)}" title="${escape(words.back)}">${glyph("back", 18)}</button><span class="folder-walk-machine">${escape(machine)}</span></div>`
      : "";
    this.element.innerHTML = `${head}<div class="folder-walk-go"><input class="folder-walk-path" type="text" value="${escape(where)}" placeholder="${escape(words.path)}" aria-label="${escape(words.path)}" spellcheck="false" autocomplete="off" autocapitalize="off" enterkeyhint="go"></div><div class="pop-list folder-walk-list">${rows}</div><div class="folder-walk-foot"><button type="button" class="sheet-go" data-walk="pick"${listing?.path ? "" : " disabled"}>${escape(words.choose)}</button></div>`;
    this.element.querySelector<HTMLElement>(".folder-walk-list")?.scrollTo({ top: 0 });
  }
}
