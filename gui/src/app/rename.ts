// A machine's name changed where it is written: a field in place of the
// name, checked by the World's rule as it is typed, saved with Enter and
// left with Escape. Under it, quietly, what the old name goes on doing;
// when something is wrong, that line says what instead (`world-network.md`,
// 机器的名字).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, type AccountChange, type Login } from "../world/accounts";
import type { NameProblem, WorldNow } from "../world/bridge";
import type { NameSaid } from "./name-words";
import { detailOf } from "./problem";
import { holds, isSuper, type Machine } from "./reach";
import { composing } from "../typing";

/** Renames a machine `to`; `called` is what it is called now, when that is
 *  no longer the name it was found under (as when a renaming is undone). */
export type Renamer = (to: string, called?: string) => Promise<void>;

/** How `machine` is renamed from this device, where it may be: this
 *  computer by its own node, on no hub or through the one it is registered
 *  on; another by a login that holds it or administers its hub, on a hub
 *  that renames (`world-network.md`, 机器的名字). */
export function renamerOf(
  machine: Machine,
  world: WorldNow,
  change: (login: Login, change: AccountChange) => Promise<void>,
  here: (to: string) => Promise<void>,
): Renamer | null {
  if (machine.here) {
    const hubs = world.accounts.hubs;
    return !hubs.length || hubs.some((hub) => hub.home && hub.renames) ? (to) => here(to) : null;
  }
  const login = loginsOf(world.accounts).find(
    (one) =>
      machine.on.some((on) => on.hub === one.hub) &&
      world.accounts.hubs.find((hub) => hub.hub === one.hub)?.renames &&
      (holds(machine, one, world) || isSuper(world, one)),
  );
  return login ? (to, called = machine.name) => change(login, { change: "rename", machine: called, to }) : null;
}

export interface RenameWords {
  rename: string;
  renameSave: string;
  cancel: string;
  renameKeeps: string;
  renameMore: string;
  renameFailed(why: string): string;
}

export interface RenameOptions {
  words: RenameWords;
  nameSaid: NameSaid;
  /** The name it has now. */
  name: string;
  /** Said under the field instead of what the old name does, as when the
   *  machine is offline and takes the name later. */
  note?: string | undefined;
  check(text: string): Promise<NameProblem | null>;
  rename(to: string): Promise<void>;
  /** Renamed `to`. */
  done(to: string): void;
  /** Left as it was. */
  cancel(): void;
}

/** How long typing rests before the name is checked. */
const CHECK_MS = 160;

export class RenameField {
  readonly element: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly said: HTMLElement;
  private readonly save: HTMLButtonElement;
  private timer = 0;
  private busy = false;
  /** The text last checked, and whether the rule let it through. */
  private checked: { text: string; problem: NameProblem | null } | null = null;

  constructor(private readonly options: RenameOptions) {
    const { words } = options;
    this.element = document.createElement("div");
    this.element.className = "rename-field";
    this.element.innerHTML = `<div class="rename-row"><input class="rename-input" name="machine-name" value="${escape(options.name)}" spellcheck="false" autocomplete="off" autocapitalize="off" aria-label="${escape(words.rename)}"><button class="icon-button rename-save" type="button" aria-label="${escape(words.renameSave)}" title="${escape(words.renameSave)}">${glyph("check", 16)}</button><button class="icon-button rename-cancel" type="button" aria-label="${escape(words.cancel)}" title="${escape(words.cancel)}">${glyph("close", 16)}</button></div><p class="rename-said" role="status"></p>`;
    this.input = this.element.querySelector<HTMLInputElement>(".rename-input")!;
    this.said = this.element.querySelector<HTMLElement>(".rename-said")!;
    this.save = this.element.querySelector<HTMLButtonElement>(".rename-save")!;
    this.quiet();
    this.input.addEventListener("input", () => {
      clearTimeout(this.timer);
      this.timer = window.setTimeout(() => void this.check(), CHECK_MS);
    });
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !composing(event)) {
        event.preventDefault();
        event.stopPropagation();
        void this.submit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        if (!this.busy) options.cancel();
      }
    });
    this.save.addEventListener("click", () => void this.submit());
    this.element.querySelector(".rename-cancel")!.addEventListener("click", () => {
      if (!this.busy) options.cancel();
    });
  }

  /** Puts the caret in the field with the name chosen, to be typed over. */
  focus(): void {
    this.input.focus({ preventScroll: true });
    this.input.select();
  }

  /** What the old name keeps doing, or the note given instead. */
  private quiet(): void {
    const { words, note } = this.options;
    this.said.classList.remove("wrong");
    this.said.textContent = note ?? words.renameKeeps;
    this.said.title = words.renameMore;
  }

  private wrong(text: string, detail = ""): void {
    this.said.classList.add("wrong");
    this.said.textContent = text;
    this.said.title = detail;
  }

  /** Checks the name as typed; says what the rule finds wrong. */
  private async check(): Promise<NameProblem | null> {
    const text = this.input.value.trim();
    if (this.checked?.text === text) return this.checked.problem;
    const problem = text === this.options.name ? null : await this.options.check(text).catch(() => null);
    // Typing went on while it was checked.
    if (this.input.value.trim() !== text) return problem;
    this.checked = { text, problem };
    if (problem) this.wrong(this.options.nameSaid(problem));
    else this.quiet();
    return problem;
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    clearTimeout(this.timer);
    const text = this.input.value.trim();
    if (text === this.options.name) return this.options.cancel();
    if (await this.check()) {
      this.input.focus();
      return;
    }
    this.busy = true;
    this.element.classList.add("busy");
    this.input.readOnly = true;
    this.save.disabled = true;
    try {
      await this.options.rename(text);
      this.options.done(text);
    } catch (error) {
      const detail = detailOf(error);
      this.wrong(this.options.words.renameFailed(detail), detail);
      this.input.readOnly = false;
      this.save.disabled = false;
      this.input.focus();
    } finally {
      this.busy = false;
      this.element.classList.remove("busy");
    }
  }

  dispose(): void {
    clearTimeout(this.timer);
  }
}
