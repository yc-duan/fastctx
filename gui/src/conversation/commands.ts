// The composer's two menus. Typing `/` at the start of the words opens the
// conversation's commands: its host's own, as the host lists them now, and
// the few the World does to any conversation it holds (compact, change the
// model, rename, fork, stop), each offered only where the conversation
// takes it. The model chip opens the models the host lists, each with the
// reasoning efforts it takes.

import { escape } from "./prose";
import type { CommandOffer, ControlKind, ModelOffer, Offers } from "../world/control";

export interface CommandWords {
  world: string;
  compact: string;
  model: string;
  rename: string;
  fork: string;
  stop: string;
  noCommands: string;
  effort: string;
  current: string;
}

/** A command as the menu shows it: the host's own, or one of the World's. */
export interface MenuCommand {
  name: string;
  description: string;
  args: string | null;
  /** The World's control it stands for, when it is one of the World's. */
  control: ControlKind | "stop" | null;
  source: string | null;
}

/** The commands a conversation offers now: its host's, then the World's
 *  for controls it takes whose names its host does not use. */
export function commandsOf(offers: Offers | null, words: CommandWords): MenuCommand[] {
  const own: MenuCommand[] = (offers?.commands ?? []).map((command: CommandOffer) => ({
    name: command.name,
    description: command.description,
    args: command.args,
    control: null,
    source: command.source,
  }));
  const taken = new Set(own.map((command) => command.name));
  const controls = new Set(offers?.controls ?? []);
  const world: MenuCommand[] = [
    { name: "compact", description: words.compact, args: null, control: "compact" as const },
    { name: "model", description: words.model, args: null, control: "model" as const },
    { name: "rename", description: words.rename, args: "…", control: "rename" as const },
    { name: "fork", description: words.fork, args: null, control: "fork" as const },
  ]
    .filter((command) => controls.has(command.control) && !taken.has(command.name))
    .map((command) => ({ ...command, source: words.world }));
  const stop: MenuCommand = { name: "stop", description: words.stop, args: null, control: "stop", source: words.world };
  return [...own, ...world, ...(taken.has("stop") ? [] : [stop])];
}

/** A small menu floating above the composer, chosen from with the arrow
 *  keys, Tab or Enter, or the pointer. */
export class Menu<T> {
  readonly element: HTMLElement;
  private items: T[] = [];
  private chosen = 0;

  constructor(
    className: string,
    private readonly row: (item: T, chosen: boolean) => string,
    private readonly pick: (item: T) => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = `composer-menu ${className}`;
    this.element.hidden = true;
    this.element.setAttribute("role", "listbox");
    this.element.addEventListener("mousedown", (event) => {
      // Keeps the words' focus while choosing with the pointer.
      event.preventDefault();
      const item = (event.target as HTMLElement).closest<HTMLElement>("[data-index]");
      if (item) this.pick(this.items[Number(item.dataset.index)]!);
    });
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  show(items: T[], empty: string): void {
    this.items = items;
    this.chosen = Math.min(this.chosen, Math.max(0, items.length - 1));
    this.element.hidden = false;
    this.draw(empty);
  }

  hide(): void {
    this.element.hidden = true;
    this.chosen = 0;
  }

  /** Handles a key while open; says whether it took it. */
  key(event: KeyboardEvent): boolean {
    if (!this.open) return false;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      if (!this.items.length) return true;
      this.chosen = (this.chosen + (event.key === "ArrowDown" ? 1 : -1) + this.items.length) % this.items.length;
      this.draw("");
      return true;
    }
    if ((event.key === "Enter" || event.key === "Tab") && !event.shiftKey && !event.isComposing) {
      const item = this.items[this.chosen];
      if (!item) return false;
      this.pick(item);
      return true;
    }
    if (event.key === "Escape") {
      this.hide();
      return true;
    }
    return false;
  }

  private draw(empty: string): void {
    this.element.innerHTML = this.items.length
      ? this.items.map((item, index) => `<div class="menu-row${index === this.chosen ? " chosen" : ""}" role="option" data-index="${index}">${this.row(item, index === this.chosen)}</div>`).join("")
      : `<div class="menu-empty">${escape(empty)}</div>`;
    this.element.querySelector(".chosen")?.scrollIntoView({ block: "nearest" });
  }
}

export function commandRow(command: MenuCommand): string {
  return `<span class="menu-name">/${escape(command.name)}${command.args ? `<span class="menu-args"> ${escape(command.args)}</span>` : ""}</span><span class="menu-what">${escape(command.description)}</span>${command.source ? `<span class="menu-source">${escape(command.source)}</span>` : ""}`;
}

export function modelRow(model: ModelOffer, current: string | null, words: CommandWords): string {
  return `<span class="menu-name">${escape(model.name ?? model.id)}${model.id === current ? `<span class="menu-current"> · ${escape(words.current)}</span>` : ""}</span><span class="menu-what">${escape(model.description ?? "")}</span>${
    model.efforts.length ? `<span class="menu-source">${escape(words.effort)}：${model.efforts.map(escape).join(" / ")}</span>` : ""
  }`;
}
