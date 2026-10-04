// Beginning a conversation: which machine, which agent on it, which
// project, and the first words (`world-gui.md`, 界面的结构). A sheet over the
// page, opened by the button in the sidebar or Ctrl+N; on a phone it rises
// from the foot of the screen. Only machines online now are offered, and
// only projects that span the machine chosen. Once the agent is asked in,
// its conversation opens and its first turn is watched from the start.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/events";
import type { Store } from "./store";
import type { NewTalkWords } from "./words";

export interface Begun {
  agent: string;
  machine: string;
  town: string | null;
  text: string;
}

export interface NewTalkOptions {
  store: Store;
  words: NewTalkWords;
  begin(begun: Begun): Promise<Address>;
  opened(address: Address): void;
}

interface Choice {
  value: string;
  label: string;
  note?: string | undefined;
  disabled?: boolean;
}

const agents = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
];

function remembered(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not kept: the next sheet starts from the usual choice.
  }
}

export class NewTalk {
  private readonly dialog: HTMLDialogElement;
  private readonly form: HTMLFormElement;
  private readonly input: HTMLTextAreaElement;
  private readonly note: HTMLElement;
  private readonly start: HTMLButtonElement;
  private machine: string | null = null;
  private agent: string = remembered("agent") ?? "claude";
  private town: string | null = null;
  private busy = false;

  constructor(private readonly options: NewTalkOptions) {
    const { words } = options;
    this.dialog = document.createElement("dialog");
    this.dialog.className = "sheet new-talk-sheet";
    this.dialog.setAttribute("aria-label", words.title);
    this.dialog.innerHTML = `
      <form class="sheet-body" method="dialog">
        <header class="sheet-head">
          <h2 class="sheet-title">${escape(words.title)}</h2>
          <button class="icon-button sheet-close" type="button" aria-label="${escape(words.cancel)}">${glyph("close", 18)}</button>
        </header>
        <div class="choices"></div>
        <textarea class="first-words" rows="3" placeholder="${escape(words.firstWords)}"></textarea>
        <p class="sheet-note" hidden></p>
        <footer class="sheet-foot">
          <span class="sheet-hint">${escape(words.hint)}</span>
          <button class="sheet-go" type="submit">${escape(words.start)}</button>
        </footer>
      </form>`;
    document.body.append(this.dialog);
    this.form = this.dialog.querySelector("form")!;
    this.input = this.dialog.querySelector("textarea")!;
    this.note = this.dialog.querySelector<HTMLElement>(".sheet-note")!;
    this.start = this.dialog.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.dialog.querySelector(".sheet-close")!.addEventListener("click", () => this.close());
    // A click on the backdrop, outside the sheet, closes it.
    this.dialog.addEventListener("click", (event) => {
      if (event.target === this.dialog) this.close();
    });
    this.dialog.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-choice]");
      if (!button || button.disabled) return;
      const value = button.dataset.value!;
      if (button.dataset.choice === "machine") this.machine = value;
      else if (button.dataset.choice === "agent") this.agent = value;
      else this.town = value || null;
      this.draw();
    });
    this.form.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.input.addEventListener("input", () => this.drawStart());
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        void this.submit();
      }
    });
  }

  /** Opens the sheet; `town` is the project to begin in, when opened from
   *  one. */
  open(town: string | null = null): void {
    const world = this.options.store.world.peek();
    const online = this.machines(world).filter((choice) => !choice.disabled);
    if (!this.machine || !online.some((choice) => choice.value === this.machine)) {
      const here = world?.here ?? null;
      this.machine = online.find((choice) => choice.value === here)?.value ?? online[0]?.value ?? null;
    }
    this.town = town;
    this.note.hidden = true;
    this.draw();
    if (!this.dialog.open) this.dialog.showModal();
    this.input.focus();
  }

  close(): void {
    if (this.busy) return;
    this.dialog.close();
  }

  private machines(world: WorldNow | null): Choice[] {
    const { words } = this.options;
    return (world?.nodes.machines ?? [])
      .filter((node) => !node.revoked)
      .map((node) => ({
        value: node.name,
        label: node.name,
        note: node.name === world?.here ? words.here : node.online === false ? words.offline : undefined,
        disabled: node.online !== true,
      }));
  }

  private draw(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const machines = this.machines(world);
    const towns: Choice[] = [
      { value: "", label: words.noProject },
      ...(world?.towns ?? [])
        .filter((town) => !this.machine || town.record.machines.includes(this.machine))
        .map((town) => ({ value: town.town, label: town.record.name })),
    ];
    if (this.town && !towns.some((choice) => choice.value === this.town)) this.town = null;
    const row = (kind: string, label: string, choices: Choice[], chosen: string | null) =>
      `<div class="choice-row" role="radiogroup" aria-label="${escape(label)}">
        <span class="choice-label">${escape(label)}</span>
        <div class="choice-options">${
          choices.length
            ? choices
                .map(
                  (choice) =>
                    `<button type="button" class="choice${choice.value === (chosen ?? "") ? " chosen" : ""}" role="radio" aria-checked="${choice.value === (chosen ?? "")}" data-choice="${kind}" data-value="${escape(choice.value)}"${choice.disabled ? " disabled" : ""}>${escape(choice.label)}${choice.note ? `<span class="choice-note">${escape(choice.note)}</span>` : ""}</button>`,
                )
                .join("")
            : `<span class="choice-none">${escape(words.noMachine)}</span>`
        }</div>
      </div>`;
    this.dialog.querySelector(".choices")!.innerHTML = [
      row("machine", words.machine, machines, this.machine),
      row("agent", words.agent, agents, this.agent),
      row("town", words.project, towns, this.town),
    ].join("");
    this.drawStart();
  }

  private drawStart(): void {
    this.start.disabled = this.busy || !this.machine || this.input.value.trim() === "";
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    if (this.busy || !this.machine || !text) return;
    this.busy = true;
    this.drawStart();
    this.start.textContent = this.options.words.starting;
    this.note.hidden = true;
    remember("agent", this.agent);
    try {
      const address = await this.options.begin({ agent: this.agent, machine: this.machine, town: this.town, text });
      this.input.value = "";
      this.busy = false;
      this.dialog.close();
      this.options.opened(address);
    } catch (error) {
      this.busy = false;
      this.note.textContent = this.options.words.notOpened(String(error));
      this.note.hidden = false;
    } finally {
      this.start.textContent = this.options.words.start;
      this.drawStart();
    }
  }
}
