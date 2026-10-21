// The settings' rows of new versions (`world-upgrade.md`, 界面上的样子): this
// application's version, and 「检查更新」, which opens the window of versions
// and looks on GitHub; what was found or heard from the network, a newer
// version opening that window too; and the device's choices: automatic
// updates, and, on a computer, passing packages to the network's devices.
// Which line it follows is the line of the version it runs: no choice.

import { escape } from "../conversation/prose";
import { detailOf } from "./problem";
import { effect } from "./signals";
import { lineOf, stageMark } from "./update-marks";
import type { Updates } from "./updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import type { UpdateSettings, UpdateState } from "../world/updates";

export class UpdateRows {
  private said = "";
  private readonly stop: () => void;

  constructor(
    private readonly element: HTMLElement,
    private readonly updates: Updates,
    private readonly words: UpdateWords,
    /** How an update under way is said, as the machines' update view says it. */
    private readonly view: UpdateViewWords,
    /** The window of versions, looking on GitHub again when asked. */
    private readonly versions: (look: boolean) => void,
    /** Whether this device passes packages on: a computer does, a phone not. */
    private readonly relays: boolean,
  ) {
    element.hidden = false;
    element.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-update-look]")) this.versions(true);
      else if (target.closest("[data-update-open]")) this.versions(false);
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

  private choose(kind: string, value: string): void {
    const state = this.updates.state.peek();
    if (!state) return;
    const settings: UpdateSettings = { ...state.settings };
    if (kind === "auto") {
      settings.auto = value === "on";
      settings.chosen = true;
    } else if (kind === "relays") settings.relays = value === "on";
    this.updates.set(settings).then(
      () => {
        this.said = "";
        this.draw(this.updates.state.peek());
      },
      (error: unknown) => {
        this.said = detailOf(error);
        this.draw(this.updates.state.peek());
      },
    );
  }

  /** The one line of how this device's updates stand, and what is behind
   *  it a hover away. */
  private line(state: UpdateState | null): string {
    const { words } = this;
    const offered = this.updates.offered.peek();
    if (this.said) return `<span class="settings-wrong" title="${escape(this.said)}">${escape(words.notSet)}</span>`;
    if (!state) return "";
    const looked = state.looked;
    const said = (text: string, title = "", tone = "") => `<span${tone ? ` class="${tone}"` : ""}${title ? ` title="${escape(title)}"` : ""}>${escape(text)}</span>`;
    if (looked.checking) return said(words.looking);
    const stage = state.stage ? stageMark(state.stage, lineOf(this.updates.version), this.view) : null;
    if (stage) return said(stage.said, "", stage.kind === "failed" ? "settings-wrong" : "");
    if (offered) return `<button class="text-button quiet-button" type="button" data-update-open>${escape(words.available(offered.manifest.version))}</button>`;
    const next = looked.next ? words.nextCheck(new Date(looked.next)) : "";
    const failure = looked.failure;
    if (failure) {
      const heard = state.network?.[state.channel === "beta" ? "beta" : "stable"] ? words.heardFromNetwork : "";
      const text =
        failure.kind === "rate_limited"
          ? words.rateLimited(failure.until ? new Date(failure.until) : null)
          : failure.kind === "http"
            ? words.githubError(failure.status)
            : failure.kind === "unreachable"
              ? words.reach[failure.reason]
              : words.checkFailed;
      return said(text, [heard, looked.why ?? "", next].filter(Boolean).join("\n"));
    }
    return said(words.latest, [looked.at ? words.versions.looked(looked.at) : "", next].filter(Boolean).join("\n"));
  }

  private draw(state: UpdateState | null): void {
    const { words } = this;
    const line = this.line(state);
    const choices = (kind: string, options: [string, string][], chosen: string) =>
      `<div class="choice-options">${options
        .map(
          ([value, name]) =>
            `<button type="button" class="choice${value === chosen ? " chosen" : ""}" role="radio" aria-checked="${value === chosen}" data-update-choice="${kind}" data-value="${value}">${escape(name)}</button>`,
        )
        .join("")}</div>`;
    const settings = state?.settings;
    const onOff: [string, string][] = [["on", words.on], ["off", words.off]];
    const auto = state ? (state.auto ?? state.settings.auto) : false;
    // Until the owner chooses, a computer with the application does not
    // update by itself: it says so beside the choice.
    const byDefault = settings && !settings.chosen ? `<span class="choice-note">${escape(words.autoByDefault)}</span>` : "";
    const html = `<dt>${escape(words.updates)}</dt><dd><span>${escape(words.current(this.updates.version))}</span><button class="text-button quiet-button" type="button" data-update-look>${escape(words.look)}</button>${
      line ? `<p class="settings-said" role="status">${line}</p>` : ""
    }</dd>${
      settings
        ? `<dt>${escape(words.auto)}</dt><dd>${choices("auto", onOff, auto ? "on" : "off")}${byDefault}</dd>
           ${this.relays ? `<dt>${escape(words.relay)}</dt><dd>${choices("relays", onOff, settings.relays ? "on" : "off")}</dd>` : ""}`
        : ""
    }`;
    if (this.element.innerHTML !== html) this.element.innerHTML = html;
  }
}
