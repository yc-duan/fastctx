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
// Between the two, where the account keeps MCP servers, a switch for each
// (`world-user-config.md`): all on as a conversation begins, each turned
// off for this one alone, taking effect at its next start.
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
import { whenNear } from "./read-on";

/** How many of the machines' rows are drawn at first, and how many more
 *  each time the end of those comes near. */
const CHOICE_ROWS = 120;

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

/** One of the account's MCP servers, as its switch shows it. */
export interface FocusServer {
  name: string;
  kind: "command" | "remote" | "unknown";
  /** The machine it runs on, for one a command starts. */
  machine: string | null;
  /** Its machine is away. */
  away: boolean;
}

/** The account's MCP servers and those the conversation goes without. */
export interface FocusServers {
  title: string;
  /** When a switch takes effect, said on hover. */
  when: string;
  /** The servers; none while they are read, or where the account keeps
   *  none. */
  list(): FocusServer[] | null;
  off(): string[];
  /** A switch was turned: every server it goes without now. */
  changed(off: string[]): void;
  /** Draws the panel again once the list has come. */
  watch(redraw: () => void): void;
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
  /** The account's MCP servers, where it keeps any. */
  servers?: FocusServers | undefined;
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
  /** How many of the machines' rows are drawn, how many each person's
   *  opens onto as last drawn, and what stops watching the end of those
   *  coming near. */
  private reach = CHOICE_ROWS;
  private inside = new Map<string, number>();
  private unwatch = () => {};

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
      this.reach = CHOICE_ROWS;
      const draw = () => this.draw();
      if (this.options.resize) this.options.resize(draw);
      else draw();
    });
    // Inside the input's own form, Enter in the field submits nothing.
    this.field.addEventListener("keydown", (event) => {
      if (event.key === "Enter") event.preventDefault();
    });
    this.element.addEventListener("click", (event) => this.click(event));
    options.servers?.watch(() => this.draw());
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
    this.reach = CHOICE_ROWS;
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
    this.reach = CHOICE_ROWS;
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
        // Opened or closed where it is, what is under it sliding in or out;
        // a person opened is drawn whole, the rows after it as they were.
        if (target.getAttribute("aria-expanded") !== "true") this.reach += this.inside.get(target.dataset.key ?? "") ?? 0;
        foldChoice(this.body, target, this.folds, () => this.html());
        this.drawn = this.html();
        break;
      case "done":
        this.options.done?.();
        break;
      case "mcp": {
        const servers = this.options.servers;
        const name = target.dataset.name;
        if (!servers || !name) break;
        const off = new Set(servers.off());
        if (off.has(name)) off.delete(name);
        else off.add(name);
        servers.changed([...off].sort());
        this.draw();
        break;
      }
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
      folders: (path) => this.options.folders(place.written, path),
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
    this.unwatch();
    // The panel stands in the panel that scrolls it, risen over the input
    // or opened down from the head.
    this.unwatch = whenNear(this.body, this.element.parentElement, () => {
      this.reach += CHOICE_ROWS;
      this.draw();
    });
  }

  private html(): string {
    const { words } = this.options;
    const places = this.placesShown();
    if (!places.length) return `<p class="focus-none">${escape(words.noMachine)}</p>`;
    return this.chosenPart(places) + this.serversPart() + (this.machinesPart(places) || `<p class="focus-none">${escape(words.noMatch)}</p>`);
  }

  /** A switch for each of the account's MCP servers; out of the way while
   *  machines are being looked for. */
  private serversPart(): string {
    const servers = this.options.servers;
    const list = servers?.list();
    if (!servers || !list?.length || this.query) return "";
    const off = new Set(servers.off());
    const rows = list
      .map((server) => {
        const on = !off.has(server.name);
        const mark = glyph(server.kind === "remote" ? "link" : "terminal", 16);
        return `<button type="button" class="focus-mcp${server.away ? " offline" : ""}" role="switch" aria-checked="${on}" data-act="mcp" data-name="${escape(server.name)}">
          <span class="focus-mark">${mark}</span><span class="focus-name">${escape(server.name)}</span>${server.machine ? `<span class="focus-note">${escape(server.machine)}</span>` : ""}<span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`;
      })
      .join("");
    return `<section class="focus-part"><h3 class="picker-title" title="${escape(servers.when)}">${escape(servers.title)}</h3><div class="focus-mcps">${rows}</div></section>`;
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
    const drawn = machineChoice(places, chosen, this.query, this.folds, this.options.words, this.reach);
    this.inside = drawn.inside;
    return drawn.html;
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
