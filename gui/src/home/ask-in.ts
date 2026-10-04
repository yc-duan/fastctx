// Asking an agent into the World from the front page: the same composer as
// a conversation's, with who, where and in which project set in the
// sentence beside it, "Ask [Codex] on [laptop] · [mark50]". Machines are
// those online now; projects, those that span the chosen machine. Whether
// that machine can run that agent is the World's to say when asked.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { WorldState } from "./model";
import type { HomeWords } from "./words";

export interface Asked {
  agent: "codex" | "claude";
  machine: string;
  town: string | null;
  text: string;
}

export class AskIn {
  readonly element: HTMLElement;
  private agent: HTMLSelectElement;
  private machine: HTMLSelectElement;
  private town: HTMLSelectElement;
  private input: HTMLTextAreaElement;
  private send: HTMLButtonElement;
  private note: HTMLElement;
  private world: WorldState | null = null;
  private busy = false;

  constructor(
    private readonly words: HomeWords,
    private readonly open: (asked: Asked) => Promise<void>,
  ) {
    this.element = document.createElement("footer");
    this.element.className = "composer-dock ask-in";
    this.element.innerHTML = `
      <p class="ask-in-note" hidden></p>
      <form class="composer">
        <textarea rows="1" placeholder="${escape(words.firstWords)}"></textarea>
        <div class="composer-row">
          <span class="ask-in-who">
            <span>${escape(words.askIn)}</span>
            <select class="pick" name="agent"><option value="codex">Codex</option><option value="claude">Claude</option></select>
            <span>${escape(words.onMachine)}</span>
            <select class="pick" name="machine"></select>
            <span class="sep">·</span>
            <select class="pick" name="town"></select>
          </span>
          <button class="composer-action" type="submit" aria-label="${escape(words.askInSend)}" disabled>${glyph("send", 18)}</button>
        </div>
      </form>`;
    this.agent = this.element.querySelector<HTMLSelectElement>('[name="agent"]')!;
    this.machine = this.element.querySelector<HTMLSelectElement>('[name="machine"]')!;
    this.town = this.element.querySelector<HTMLSelectElement>('[name="town"]')!;
    this.input = this.element.querySelector("textarea")!;
    this.send = this.element.querySelector<HTMLButtonElement>(".composer-action")!;
    this.note = this.element.querySelector<HTMLElement>(".ask-in-note")!;
    this.machine.addEventListener("change", () => this.drawTowns());
    this.input.addEventListener("input", () => {
      this.input.style.height = "auto";
      this.input.style.height = `${this.input.scrollHeight}px`;
      this.drawSend();
    });
    this.input.addEventListener("keydown", (event) => {
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !matchMedia("(pointer: coarse)").matches) {
        event.preventDefault();
        void this.submit();
      }
    });
    this.element.querySelector("form")!.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
  }

  /** Keeps the choices to what the World now has, keeping what is chosen
   *  where it still can be. */
  update(world: WorldState): void {
    this.world = world;
    const machines = world.nodes.filter((node) => node.online).map((node) => node.name);
    const chosen = this.machine.value || world.here;
    const options = machines.map((name) => `<option value="${escape(name)}">${escape(name)}</option>`).join("");
    if (this.machine.innerHTML !== options) {
      this.machine.innerHTML = options;
      this.machine.value = machines.includes(chosen) ? chosen : (machines[0] ?? "");
    }
    this.drawTowns();
    this.drawSend();
  }

  private drawTowns(): void {
    const machine = this.machine.value;
    const towns = (this.world?.towns ?? []).filter((town) => town.record.machines.includes(machine));
    const chosen = this.town.value;
    const options = [
      `<option value="">${escape(this.words.noProject)}</option>`,
      ...towns.map((town) => `<option value="${escape(town.town)}">${escape(town.town)}</option>`),
    ].join("");
    if (this.town.innerHTML !== options) {
      this.town.innerHTML = options;
      this.town.value = towns.some((town) => town.town === chosen) ? chosen : "";
    }
  }

  private drawSend(): void {
    this.send.disabled = this.busy || !this.input.value.trim() || !this.machine.value;
  }

  private async submit(): Promise<void> {
    const text = this.input.value.trim();
    if (!text || !this.machine.value || this.busy) return;
    this.busy = true;
    this.note.hidden = true;
    this.drawSend();
    try {
      await this.open({
        agent: this.agent.value === "claude" ? "claude" : "codex",
        machine: this.machine.value,
        town: this.town.value || null,
        text,
      });
      this.input.value = "";
    } catch (error) {
      this.note.textContent = this.words.notOpened(String(error));
      this.note.hidden = false;
    } finally {
      this.busy = false;
      this.drawSend();
    }
  }
}
