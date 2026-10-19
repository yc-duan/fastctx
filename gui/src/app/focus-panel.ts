// Pointing a conversation at machines and folders (`world-gui.md`, 新对话
// and 对话): the machines its account may use, each chosen as a whole or
// with folders on it. What is chosen is said to the agent as where its work
// is; it limits nothing.
//
// The machines chosen stand first, each with its folders as small solid
// chips and a way to add one, which opens that machine's folders in a
// window of their own (`folder-picker.ts`). Under them, every machine it may
// use, in its owner's groups (`machine-choice.ts`); one away may still be
// chosen, for the agent is only told. A field narrows them to the machines
// whose name, or whose group's, holds what is typed.
//
// The same panel serves a conversation about to begin, risen over its input,
// and one under way, from the strip at the top of its page; the field stands
// where the panel is reached from (`focus-panel.css`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { folderName, type Focus, type Folders } from "../world/focus";
import { allowPrivacy } from "../world/files";
import { FolderPicker } from "./folder-picker";
import { foldChoice, keptFolds, machineChoice } from "./machine-choice";
import type { FocusWords } from "./words";
import { redrawKeepingFocus } from "./focus";

/** A machine the conversation's account may use. */
export interface FocusPlace {
  /** As written: what is kept, and what the agent writes after `on`. */
  written: string;
  /** Its own name, for asking it. */
  name: string;
  here: boolean;
  online: boolean | null;
  /** The group its owner put it in. */
  group?: string | null;
  /** Whose it is, where it is another person's. */
  owner?: string | null;
  form?: "desktop" | "laptop" | "server";
}

export interface FocusPanelOptions {
  words: FocusWords;
  places(): FocusPlace[];
  folders(machine: string, path: string | null): Promise<Folders>;
  /** Each change, as it is made. */
  changed(focus: Focus): void;
  /** Where the panel has a button to close it with. */
  done?: () => void;
  /** Places already named on a machine, offered where its folders are
   *  chosen: a project's. */
  marks?(machine: string): { name: string; path: string }[];
  /** Makes a change that alters the panel's height, where its holder
   *  carries the height over (`resize-upward.ts`). */
  resize?(change: () => void): void;
}

export class FocusPanel {
  readonly element: HTMLElement;
  private readonly body: HTMLElement;
  private readonly field: HTMLInputElement;
  private focus: Focus;
  private query = "";
  private readonly folds = keptFolds();
  private readonly picker: FolderPicker;
  /** What the list was last drawn as. */
  private drawn = "";

  constructor(
    private readonly options: FocusPanelOptions,
    focus: Focus,
  ) {
    const { words } = options;
    this.focus = structuredClone(focus);
    this.picker = new FolderPicker(words.picker);
    this.element = document.createElement("div");
    this.element.className = "focus-panel";
    this.element.innerHTML = `<div class="focus-body"></div><div class="focus-foot"><label class="focus-search">${glyph("search", 16)}<input class="focus-filter" type="search" placeholder="${escape(words.search)}" aria-label="${escape(words.search)}" autocomplete="off" spellcheck="false"></label>${options.done ? `<button type="button" class="sheet-go focus-done" data-act="done">${escape(words.done)}</button>` : ""}</div>`;
    this.body = this.element.querySelector(".focus-body")!;
    this.field = this.element.querySelector(".focus-filter")!;
    this.field.addEventListener("input", () => {
      this.query = this.field.value.trim().toLowerCase();
      const draw = () => this.draw();
      if (this.options.resize) this.options.resize(draw);
      else draw();
    });
    // Inside the input's own form, Enter in the field submits nothing.
    this.field.addEventListener("keydown", (event) => {
      if (event.key === "Enter") event.preventDefault();
    });
    this.element.addEventListener("click", (event) => this.click(event));
    this.draw();
  }

  get value(): Focus {
    return structuredClone(this.focus);
  }

  /** Starts again from `focus`, nothing typed. */
  set(focus: Focus): void {
    this.focus = structuredClone(focus);
    this.query = "";
    this.field.value = "";
    this.draw();
  }

  /** Drawn again for the machines as they stand now. */
  refresh(): void {
    this.draw();
  }

  /** As when it was opened: nothing typed. */
  reset(): void {
    if (!this.query) return;
    this.query = "";
    this.field.value = "";
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
        const change = () => this.change();
        // Choosing one changes what stands first, and so the panel's height.
        if (this.options.resize) this.options.resize(change);
        else change();
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
      case "fold":
        // Opened or closed where it is, what is under it sliding in or out.
        foldChoice(this.body, target, this.folds);
        this.drawn = this.html();
        break;
      case "done":
        this.options.done?.();
        break;
    }
  }

  /** Opens `place`'s folders in a window of their own; the one chosen joins
   *  the machine's. */
  private browse(place: FocusPlace): void {
    const chosen = this.focus.machines.find((one) => one.machine === place.written);
    const named = this.options.marks?.(place.written) ?? [];
    const kept = (chosen?.folders ?? []).filter((path) => !named.some((mark) => mark.path === path)).map((path) => ({ name: folderName(path), path }));
    this.picker.open({
      machine: place.written,
      folders: (path) => this.options.folders(place.name, path),
      marks: [...named, ...kept],
      start: chosen?.folders.at(-1) ?? null,
      ...(place.here ? { allow: () => void allowPrivacy() } : {}),
      chosen: (path) => {
        let held = this.focus.machines.find((one) => one.machine === place.written);
        if (!held) {
          held = { machine: place.written, folders: [] };
          this.focus.machines.push(held);
        }
        if (!held.folders.includes(path)) held.folders.push(path);
        const change = () => this.change();
        if (this.options.resize) this.options.resize(change);
        else change();
      },
    });
  }

  /** The machines it may use, and any chosen before that it no longer may. */
  private placesShown(): FocusPlace[] {
    const places = this.options.places();
    const gone = this.focus.machines
      .filter((chosen) => !places.some((place) => place.written === chosen.machine))
      .map((chosen): FocusPlace => ({ written: chosen.machine, name: chosen.machine.split(".")[0]!, here: false, online: false }));
    return [...places, ...gone];
  }

  private draw(): void {
    // Drawn again only when it changed, the control pressed keeping focus.
    const html = this.html();
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  private html(): string {
    const { words } = this.options;
    const places = this.placesShown();
    if (!places.length) return `<p class="focus-none">${escape(words.noMachine)}</p>`;
    return this.chosenPart(places) + (this.machinesPart(places) || `<p class="focus-none">${escape(words.noMatch)}</p>`);
  }

  /** The machines chosen, each with its folders and a way to add one. */
  private chosenPart(places: FocusPlace[]): string {
    const { words } = this.options;
    if (!this.focus.machines.length) return "";
    const rows = this.focus.machines
      .map((chosen) => {
        const place = places.find((one) => one.written === chosen.machine);
        const away = place?.online === false;
        const note = place?.here ? words.here : "";
        const chips = chosen.folders
          .map(
            (folder) =>
              `<span class="focus-folder" title="${escape(folder)}">${glyph("folder", 14)}<span class="focus-folder-name">${escape(folderName(folder))}</span><button type="button" class="focus-remove" data-act="remove" data-machine="${escape(chosen.machine)}" data-path="${escape(folder)}" aria-label="${escape(`${words.remove} ${folder}`)}" title="${escape(words.remove)}">${glyph("close", 12)}</button></span>`,
          )
          .join("");
        return `<div class="focus-held${away ? " offline" : ""}"${away ? ` title="${escape(words.offline)}"` : ""}>
          <div class="focus-held-head"><span class="focus-mark">${glyph(place?.form ?? "machine", 16)}</span><span class="focus-name">${escape(chosen.machine)}</span>${note ? `<span class="focus-note">${escape(note)}</span>` : ""}<button type="button" class="focus-unpick" data-act="toggle" data-machine="${escape(chosen.machine)}" aria-label="${escape(`${words.remove} ${chosen.machine}`)}" title="${escape(words.remove)}">${glyph("close", 14)}</button></div>
          <div class="focus-folders">${chips}<button type="button" class="focus-add" data-act="browse" data-machine="${escape(chosen.machine)}"${away ? " disabled" : ""}>${glyph("plus", 14)}<span>${escape(words.addFolder)}</span></button></div>
        </div>`;
      })
      .join("");
    return `<section class="focus-part"><h3 class="picker-title">${escape(words.chosen)}</h3>${rows}</section>`;
  }

  /** Every machine it may use, narrowed to what is typed. */
  private machinesPart(places: FocusPlace[]): string {
    const chosen = new Set(this.focus.machines.map((one) => one.machine));
    return machineChoice(places, chosen, this.query, this.folds, this.options.words);
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
  const said = items.map((item) => (item.folder ? `${item.machine}: ${item.folder}` : item.machine)).join(", ");
  const more = items.length > 1 ? `<span class="focus-more">${escape(words.more(items.length - 1))}</span>` : "";
  const html = `<span class="talk-chip-name">${escape(first.folder ?? first.machine)}</span>${first.folder ? `<span class="talk-chip-sub">${escape(first.machine)}</span>` : ""}${more}`;
  return { html, said };
}
