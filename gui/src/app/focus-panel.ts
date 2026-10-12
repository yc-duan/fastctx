// Pointing a conversation at machines and folders (`world-gui.md`, 新对话
// and 对话): the machines its account may use, each chosen as a whole or
// with folders on it, a folder chosen by walking the machine's own folders
// a level at a time, or by typing its path. What is chosen is said to the
// agent as where its work is; it limits nothing.
//
// The same panel serves a conversation about to begin, above its input,
// and one under way, from the strip at the top of its page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { folderName, type Focus, type Folders } from "../world/focus";
import type { FocusWords } from "./words";

/** A machine the conversation's account may use. */
export interface FocusPlace {
  /** As written: what is kept, and what the agent writes after `on`. */
  written: string;
  /** Its own name, for asking it. */
  name: string;
  here: boolean;
  online: boolean | null;
}

export interface FocusPanelOptions {
  words: FocusWords;
  places(): FocusPlace[];
  folders(machine: string, path: string | null): Promise<Folders>;
  /** Each change, as it is made. */
  changed(focus: Focus): void;
  /** Where the panel has a button to close it with. */
  done?: () => void;
}

/** Walking one machine's folders. */
interface Walk {
  place: FocusPlace;
  path: string | null;
  listing: Folders | null;
  error: string | null;
  /** The latest ask; an answer to an earlier one is dropped. */
  asked: number;
}

export class FocusPanel {
  readonly element: HTMLElement;
  private focus: Focus;
  private walk: Walk | null = null;
  private asks = 0;

  constructor(
    private readonly options: FocusPanelOptions,
    focus: Focus,
  ) {
    this.focus = structuredClone(focus);
    this.element = document.createElement("div");
    this.element.className = "focus-panel";
    this.element.addEventListener("click", (event) => this.click(event));
    // The panel may stand inside the input's own form, so a typed path goes
    // on Enter here and never submits anything.
    this.element.addEventListener("keydown", (event) => {
      const input = (event.target as HTMLElement).closest<HTMLInputElement>(".focus-path");
      if (!input || event.key !== "Enter" || event.isComposing) return;
      event.preventDefault();
      event.stopPropagation();
      const typed = input.value.trim();
      if (this.walk && typed) this.go(typed);
    });
    this.draw();
  }

  get value(): Focus {
    return structuredClone(this.focus);
  }

  /** Starts again from `focus`, at the list of machines. */
  set(focus: Focus): void {
    this.focus = structuredClone(focus);
    this.walk = null;
    this.draw();
  }

  /** Drawn again for the machines as they stand now, unless a machine's
   *  folders are being walked. */
  refresh(): void {
    if (!this.walk) this.draw();
  }

  /** Back at the list of machines, as when it was opened. */
  reset(): void {
    if (!this.walk) return;
    this.walk = null;
    this.draw();
  }

  private change(): void {
    this.options.changed(this.value);
    this.draw();
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!target || target.disabled) return;
    const machine = target.dataset.machine ?? "";
    switch (target.dataset.act) {
      case "toggle": {
        const at = this.focus.machines.findIndex((one) => one.machine === machine);
        if (at >= 0) this.focus.machines.splice(at, 1);
        else this.focus.machines.push({ machine, folders: [] });
        this.change();
        break;
      }
      case "remove": {
        const chosen = this.focus.machines.find((one) => one.machine === machine);
        if (chosen) chosen.folders = chosen.folders.filter((folder) => folder !== target.dataset.path);
        this.change();
        break;
      }
      case "browse": {
        const place = this.placesShown().find((one) => one.written === machine);
        if (!place) return;
        this.walk = { place, path: null, listing: null, error: null, asked: 0 };
        this.go(null);
        break;
      }
      case "into":
        this.go(target.dataset.path ?? null);
        break;
      case "up":
        this.go(this.walk?.listing?.parent ?? null);
        break;
      case "back":
        this.walk = null;
        this.draw();
        break;
      case "pick": {
        const walk = this.walk;
        const path = walk?.listing?.path;
        if (!walk || !path) return;
        let chosen = this.focus.machines.find((one) => one.machine === walk.place.written);
        if (!chosen) {
          chosen = { machine: walk.place.written, folders: [] };
          this.focus.machines.push(chosen);
        }
        if (!chosen.folders.includes(path)) chosen.folders.push(path);
        this.walk = null;
        this.change();
        break;
      }
      case "done":
        this.options.done?.();
        break;
    }
  }

  /** Asks the machine for the folders in `path`, or its places to start. */
  private go(path: string | null): void {
    const walk = this.walk;
    if (!walk) return;
    const asked = ++this.asks;
    walk.asked = asked;
    walk.path = path;
    walk.error = null;
    walk.listing = null;
    this.draw();
    this.options
      .folders(walk.place.name, path)
      .then((listing) => {
        if (this.walk !== walk || walk.asked !== asked) return;
        walk.listing = listing;
        walk.path = listing.path;
      })
      .catch((error: unknown) => {
        if (this.walk !== walk || walk.asked !== asked) return;
        walk.error = error instanceof Error ? error.message : String(error);
      })
      .finally(() => {
        if (this.walk === walk && walk.asked === asked) this.draw();
      });
  }

  /** The machines it may use, and any chosen before that it no longer may. */
  private placesShown(): FocusPlace[] {
    const places = this.options.places();
    const gone = this.focus.machines
      .filter((chosen) => !places.some((place) => place.written === chosen.machine))
      .map((chosen) => ({ written: chosen.machine, name: chosen.machine.split(".")[0]!, here: false, online: false }));
    return [...places, ...gone];
  }

  private draw(): void {
    const html = this.walk ? this.walking(this.walk) : this.machines();
    this.element.classList.toggle("walking", !!this.walk);
    this.element.innerHTML = html;
    if (this.walk) this.element.querySelector<HTMLElement>(".focus-list")?.scrollTo({ top: 0 });
  }

  private machines(): string {
    const { words } = this.options;
    const places = this.placesShown();
    const rows = places
      .map((place) => {
        const chosen = this.focus.machines.find((one) => one.machine === place.written);
        const note = place.here ? words.here : place.online === false ? words.offline : "";
        const folders = chosen
          ? `<div class="focus-folders">${chosen.folders
              .map(
                (folder) =>
                  `<span class="focus-folder" title="${escape(folder)}">${glyph("folder", 14)}<span class="focus-folder-name">${escape(folderName(folder))}</span><button type="button" class="focus-remove" data-act="remove" data-machine="${escape(place.written)}" data-path="${escape(folder)}" aria-label="${escape(`${words.remove} ${folder}`)}">${glyph("close", 12)}</button></span>`,
              )
              .join("")}<button type="button" class="focus-add" data-act="browse" data-machine="${escape(place.written)}"${place.online === false ? " disabled" : ""}>${glyph("plus", 14)}<span>${escape(words.addFolder)}</span></button></div>`
          : "";
        return `<div class="focus-machine${chosen ? " chosen" : ""}"><button type="button" class="focus-toggle" data-act="toggle" data-machine="${escape(place.written)}" aria-pressed="${!!chosen}">${glyph("machine", 16)}<span class="focus-name">${escape(place.written)}</span>${note ? `<span class="choice-note">${escape(note)}</span>` : ""}${chosen ? glyph("check", 14) : ""}</button>${folders}</div>`;
      })
      .join("");
    const body = rows || `<span class="choice-none quiet">${escape(words.noMachine)}</span>`;
    const done = this.options.done ? `<div class="focus-foot"><button type="button" class="sheet-go focus-done" data-act="done">${escape(words.done)}</button></div>` : "";
    return `<h3 class="picker-title">${escape(words.title)}</h3><div class="focus-machines">${body}</div>${done}`;
  }

  private walking(walk: Walk): string {
    const { words } = this.options;
    const listing = walk.listing;
    let rows = "";
    if (walk.path !== null) rows += `<button type="button" class="pop-row focus-up" data-act="up">${glyph("back", 14)}<span class="pop-label">${escape(words.up)}</span></button>`;
    if (walk.error) rows += `<span class="choice-none">${escape(walk.error)}</span>`;
    else if (!listing) rows += `<span class="choice-none quiet">${escape(words.reading)}</span>`;
    else if (!listing.folders.length) rows += `<span class="choice-none quiet">${escape(words.noFolders)}</span>`;
    else
      rows += listing.folders
        .map(
          (folder) =>
            `<button type="button" class="pop-row${folder.hidden ? " quiet" : ""}" data-act="into" data-path="${escape(folder.path)}">${glyph("folder", 16)}<span class="pop-label">${escape(folder.name)}</span></button>`,
        )
        .join("");
    const where = listing?.path ?? walk.path ?? "";
    return `<div class="focus-walk-head"><button type="button" class="icon-button" data-act="back" aria-label="${escape(words.back)}" title="${escape(words.back)}">${glyph("back", 18)}</button><span class="focus-name">${escape(walk.place.written)}</span></div><div class="focus-go"><input class="focus-path" type="text" value="${escape(where)}" placeholder="${escape(words.path)}" aria-label="${escape(words.path)}" spellcheck="false" autocomplete="off" autocapitalize="off" enterkeyhint="go"></div><div class="pop-list focus-list">${rows}</div><div class="focus-foot"><button type="button" class="sheet-go focus-done" data-act="pick"${listing?.path ? "" : " disabled"}>${escape(words.choose)}</button></div>`;
  }
}

/** What a chip shows of a choice: its first machine or folder, and how many
 *  more; said whole to a screen reader. Null when nothing is chosen. */
export function focusSummary(focus: Focus, words: FocusWords): { html: string; said: string } | null {
  const items = focus.machines.flatMap((chosen): { machine: string; folder: string | null }[] =>
    chosen.folders.length ? chosen.folders.map((folder) => ({ machine: chosen.machine, folder: folderName(folder) })) : [{ machine: chosen.machine, folder: null }],
  );
  const first = items[0];
  if (!first) return null;
  const said = items.map((item) => (item.folder ? `${item.machine} · ${item.folder}` : item.machine)).join(", ");
  const more = items.length > 1 ? `<span class="focus-more">${escape(words.more(items.length - 1))}</span>` : "";
  const html = `<span class="talk-chip-name">${escape(first.folder ?? first.machine)}</span>${first.folder ? `<span class="talk-chip-sub">${escape(first.machine)}</span>` : ""}${more}`;
  return { html, said };
}
