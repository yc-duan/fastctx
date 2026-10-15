// The settings' rows of new versions (`world-upgrade.md`, 界面上的样子): this
// application's version with a look on GitHub and what it found or heard
// from the network; and the device's three choices: automatic updates, test
// releases, and, on a computer, passing packages to the network's devices.

import { escape } from "../conversation/prose";
import { effect } from "./signals";
import type { Updates } from "./updates";
import type { UpdateWords } from "./update-words";
import type { UpdateSettings, UpdateState } from "../world/updates";

export class UpdateRows {
  private looking = false;
  private said = "";
  private readonly stop: () => void;

  constructor(
    private readonly element: HTMLElement,
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    /** The new version's card. */
    private readonly open: () => void,
    /** Whether this device passes packages on: a computer does, a phone not. */
    private readonly relays: boolean,
  ) {
    element.hidden = false;
    element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-update-look]")) this.look();
      else if (target.closest("[data-update-open]")) this.open();
      const choice = target.closest<HTMLButtonElement>("[data-update-choice]");
      if (choice) this.choose(choice.dataset.updateChoice!, choice.dataset.value!);
    });
    this.stop = effect(() => {
      updates.offered.get();
      this.draw(updates.state.get());
    });
  }

  leave(): void {
    this.stop();
  }

  private look(): void {
    if (this.looking) return;
    this.looking = true;
    this.said = "";
    this.draw(this.updates.state.peek());
    this.updates
      .look()
      .catch((error: unknown) => {
        this.said = String(error);
      })
      .finally(() => {
        this.looking = false;
        this.draw(this.updates.state.peek());
      });
  }

  private choose(kind: string, value: string): void {
    const state = this.updates.state.peek();
    if (!state) return;
    const settings: UpdateSettings = { ...state.settings };
    if (kind === "auto") settings.auto = value === "on";
    else if (kind === "relays") settings.relays = value === "on";
    else if (kind === "beta") {
      if (value === "network") delete settings.beta;
      else settings.beta = value === "on";
    }
    this.updates.set(settings).catch((error: unknown) => {
      this.said = String(error);
      this.draw(this.updates.state.peek());
    });
  }

  private draw(state: UpdateState | null): void {
    const { words } = this;
    const offered = this.updates.offered.peek();
    let line: string;
    if (this.looking) line = escape(words.looking);
    else if (this.said) line = escape(this.said);
    else if (offered)
      line = `<button class="text-button quiet-button" type="button" data-update-open>${escape(words.available(offered.manifest.version))}</button>`;
    else if (!state) line = "";
    else {
      const parts = [words.latest];
      if (state.looked.github === false) parts.push(words.unreachable);
      if (state.looked.github === false && state.network?.[state.channel === "beta" ? "beta" : "stable"]) parts.push(words.heardFromNetwork);
      line = escape(parts.join(" · "));
    }
    const choices = (kind: string, options: [string, string][], chosen: string) =>
      `<div class="choice-options">${options
        .map(
          ([value, name]) =>
            `<button type="button" class="choice${value === chosen ? " chosen" : ""}" role="radio" aria-checked="${value === chosen}" data-update-choice="${kind}" data-value="${value}">${escape(name)}</button>`,
        )
        .join("")}</div>`;
    const settings = state?.settings;
    const onOff: [string, string][] = [["on", words.on], ["off", words.off]];
    const html = `<dt>${escape(words.updates)}</dt><dd><span>${escape(words.current(this.updates.version))}</span><button class="text-button quiet-button" type="button" data-update-look${this.looking ? " disabled" : ""}>${escape(words.look)}</button>${
      line ? `<p class="settings-said" role="status">${line}</p>` : ""
    }</dd>${
      settings
        ? `<dt>${escape(words.auto)}</dt><dd>${choices("auto", onOff, settings.auto ? "on" : "off")}</dd>
           <dt>${escape(words.betaChoice)}</dt><dd>${choices("beta", [["network", words.followNetwork], ...onOff], settings.beta === undefined || settings.beta === null ? "network" : settings.beta ? "on" : "off")}</dd>
           ${this.relays ? `<dt>${escape(words.relay)}</dt><dd>${choices("relays", onOff, settings.relays ? "on" : "off")}</dd>` : ""}`
        : ""
    }`;
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
