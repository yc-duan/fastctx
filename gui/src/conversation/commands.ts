// The composer's two menus. Typing `/` at the start of the words opens the
// conversation's commands: the few the World does to any conversation it
// holds (compact, change the model, rename, fork, stop, set its goal), each offered only
// where the conversation takes it, and its host's own, as the host lists
// them now. A World control stands for its name, so the same command does
// the same thing on every host; the host's command of that name is kept
// behind it for words the control does not take (`/compact` with
// instructions). The model chip opens the models the host lists, each with
// the reasoning efforts it takes.

import { layerOpened } from "../app/back";
import { escape } from "./prose";
import type { CommandArguments, CommandOffer, ControlKind, ModelOffer, Offers } from "../world/control";
import { composing } from "../typing";

export interface CommandWords {
  world: string;
  compact: string;
  model: string;
  rename: string;
  fork: string;
  stop: string;
  goal: string;
  noCommands: string;
  effort: string;
  current: string;
  /** What rename takes, as a usage hint. */
  renameArgs: string;
  /** What a goal takes, as a usage hint. */
  goalArgs: string;
  /** A command that takes words was sent without them. */
  needsArgs(usage: string): string;
  /** Codex's commands the World carries out, said in the interface's
   *  words; the node lists them in the words of a command line. */
  host: Record<string, string>;
}

/** A command as the menu shows it: the host's own, or one of the World's. */
export interface MenuCommand {
  name: string;
  description: string;
  args: string | null;
  arguments: CommandArguments;
  /** The World's control it stands for, when it is one of the World's. */
  control: ControlKind | "stop" | "goal" | null;
  source: string | null;
  /** The host's own identity for it, handed back when it runs. */
  path?: string | undefined;
  /** The host's command of the same name a World control stands for. */
  host?: MenuCommand | undefined;
}

/** The commands a conversation offers now: the World's for the controls
 *  it takes, then its host's under the names left. */
/** What `/` offers. While a turn runs, compaction (and Codex's review, which
 *  starts a turn of its own) waits for it to end, so it is not offered. */
export function commandsOf(offers: Offers | null, words: CommandWords, moving = false, goal = false): MenuCommand[] {
  // A node that keeps them until the turn ends takes them at any time.
  const waits = (name: string) => moving && !offers?.control_queue && (name === "compact" || name === "review");
  const own: MenuCommand[] = (offers?.commands ?? []).filter((command) => command.path || !waits(command.name)).map((command: CommandOffer) => ({
    name: command.name,
    description: (command.source === "codex" && !command.path && words.host[command.name]) || command.description,
    args: command.args,
    arguments: command.arguments ?? "optional",
    control: null,
    source: command.source,
    path: command.path,
  }));
  // Setting a goal is the World's own, where the page carries the goal.
  const controls = new Set<ControlKind | "stop" | "goal">([...(offers?.controls ?? []), "stop", ...(goal ? (["goal"] as const) : [])]);
  // A skill keeps its own name even where it is a control's: it is the
  // host's file, not the command.
  const hostOf = (name: string) => own.find((command) => command.name === name && !command.path);
  const world: MenuCommand[] = (
    [
      { name: "compact", description: words.compact, args: null, arguments: "none", control: "compact" },
      { name: "model", description: words.model, args: null, arguments: "optional", control: "model" },
      { name: "rename", description: words.rename, args: words.renameArgs, arguments: "required", control: "rename" },
      { name: "fork", description: words.fork, args: null, arguments: "none", control: "fork" },
      { name: "stop", description: words.stop, args: null, arguments: "none", control: "stop" },
      { name: "goal", description: words.goal, args: words.goalArgs, arguments: "required", control: "goal" },
    ] as const
  )
    .filter((command) => controls.has(command.control) && !waits(command.name))
    .map((command) => ({ ...command, source: words.world, host: hostOf(command.name) }));
  const standing = new Set(world.map((command) => command.name));
  return [...world, ...own.filter((command) => command.path || !standing.has(command.name))];
}

/** What runs for `/name args`: the World's control, unless words follow that
 *  it takes none of and the host's command of that name takes them. */
export function commandFor(commands: MenuCommand[], name: string, args: string): MenuCommand | null {
  const command = commands.find((one) => one.name === name && !one.path) ?? commands.find((one) => one.name === name) ?? null;
  if (command?.control && args && command.arguments === "none" && command.host && command.host.arguments !== "none") return command.host;
  return command;
}

/** A small menu floating above the composer, chosen from with the arrow
 *  keys, Tab or Enter, or the pointer. */
export class Menu<T> {
  readonly element: HTMLElement;
  private items: T[] = [];
  private chosen = 0;
  private readonly id = `menu-${++menus}`;

  constructor(
    className: string,
    private readonly row: (item: T, chosen: boolean) => string,
    private readonly pick: (item: T) => void,
    /** The field it is chosen from, told which row is chosen. */
    private readonly field: HTMLElement | null = null,
  ) {
    this.element = document.createElement("div");
    this.element.className = `composer-menu ${className}`;
    this.element.hidden = true;
    this.element.id = this.id;
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
    if (!this.open) layerOpened({ isOpen: () => this.open && this.element.isConnected, close: () => this.hide() });
    this.element.hidden = false;
    this.field?.setAttribute("aria-controls", this.id);
    this.draw(empty);
  }

  hide(): void {
    this.element.hidden = true;
    this.chosen = 0;
    if (this.field?.getAttribute("aria-controls") === this.id) {
      this.field.removeAttribute("aria-controls");
      this.field.removeAttribute("aria-activedescendant");
    }
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
    if ((event.key === "Enter" || event.key === "Tab") && !event.shiftKey && !composing(event)) {
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
      ? this.items
          .map(
            (item, index) =>
              `<div class="menu-row${index === this.chosen ? " chosen" : ""}" role="option" id="${this.id}-${index}" aria-selected="${index === this.chosen}" data-index="${index}">${this.row(item, index === this.chosen)}</div>`,
          )
          .join("")
      : `<div class="menu-empty">${escape(empty)}</div>`;
    this.element.querySelector(".chosen")?.scrollIntoView({ block: "nearest" });
    if (this.items.length) this.field?.setAttribute("aria-activedescendant", `${this.id}-${this.chosen}`);
    else this.field?.removeAttribute("aria-activedescendant");
  }
}

let menus = 0;

export function commandRow(command: MenuCommand): string {
  return `<span class="menu-name">/${escape(command.name)}${command.args ? `<span class="menu-args"> ${escape(command.args)}</span>` : ""}</span><span class="menu-what">${escape(command.description)}</span>${command.source ? `<span class="menu-source">${escape(command.source)}</span>` : ""}`;
}

/** How a command is written with its words, for the hint while they are
 *  typed. */
export function usageOf(command: MenuCommand): string {
  return `/${command.name}${command.args ? ` ${command.args}` : ""}`;
}

export function modelRow(model: ModelOffer, current: string | null, words: CommandWords): string {
  return `<span class="menu-name">${escape(model.name ?? model.id)}${model.id === current ? `<span class="menu-current"> ${escape(words.current)}</span>` : ""}</span><span class="menu-what">${escape(model.description ?? "")}</span>${
    model.efforts.length ? `<span class="menu-source">${escape(words.effort)}：${model.efforts.map(escape).join(" / ")}</span>` : ""
  }`;
}
