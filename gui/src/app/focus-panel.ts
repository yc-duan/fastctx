// Pointing a conversation at machines and folders (`world-gui.md`, 新对话
// and 对话): the machines its account may use, each chosen as a whole or
// with folders on it, a folder chosen by walking the machine's own folders
// (`folder-walk.ts`). What is chosen is said to the agent as where its work
// is; it limits nothing.
//
// The same panel serves a conversation about to begin, above its input,
// and one under way, from the strip at the top of its page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { folderName, type Focus, type Folders } from "../world/focus";
import { FolderWalk } from "./folder-walk";
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

export class FocusPanel {
  readonly element: HTMLElement;
  private focus: Focus;
  /** Walking one machine's folders, in the panel's place. */
  private walk: FolderWalk | null = null;

  constructor(
    private readonly options: FocusPanelOptions,
    focus: Focus,
  ) {
    this.focus = structuredClone(focus);
    this.element = document.createElement("div");
    this.element.className = "focus-panel";
    this.element.addEventListener("click", (event) => this.click(event));
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
        if (place) this.browse(place);
        break;
      }
      case "done":
        this.options.done?.();
        break;
    }
  }

  /** Walks `place`'s folders in the panel's place, until one is chosen or
   *  the walk goes back. */
  private browse(place: FocusPlace): void {
    const walk = new FolderWalk({
      words: this.options.words,
      machine: place.written,
      folders: (path) => this.options.folders(place.name, path),
      chosen: (path) => {
        let chosen = this.focus.machines.find((one) => one.machine === place.written);
        if (!chosen) {
          chosen = { machine: place.written, folders: [] };
          this.focus.machines.push(chosen);
        }
        if (!chosen.folders.includes(path)) chosen.folders.push(path);
        this.walk = null;
        this.change();
      },
      back: () => {
        this.walk = null;
        this.draw();
      },
    });
    this.walk = walk;
    this.draw();
    walk.go(null);
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
    if (this.walk) {
      this.element.replaceChildren(this.walk.element);
      return;
    }
    this.element.innerHTML = this.machines();
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
