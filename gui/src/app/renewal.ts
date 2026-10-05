// Which World this computer runs, beside the interface's: the same, or an
// older one the interface cannot speak with. Putting the new one in place
// is the user's to start; the program the interface carries does it and
// keeps its own record, and this row says where it stands, step by step,
// asked again every two seconds while a step is under way, as the World
// is. The one step that needs an administrator is asked for only when it
// is reached.

import { escape } from "../conversation/prose";
import type { RenewalPhase, RenewalStatus } from "../world/renewal";
import type { SettingsWords } from "./words";

export interface Renewing {
  state(): Promise<RenewalStatus>;
  /** Puts the carried program in place and asks the node to hand over. */
  renew(): Promise<void>;
  /** The administrator's step: the part that keeps this computer in the
   *  World after it starts. */
  guardian(): Promise<void>;
}

/** Steps the World is taking by itself, after which it is asked again. */
const UNDER_WAY: RenewalPhase[] = ["preparing", "waiting_for_idle", "updating_guardian", "returning_to_service"];

export class RenewalRow {
  private status: RenewalStatus | null = null;
  private said = "";
  private busy = false;
  private timer = 0;
  private left = false;

  constructor(
    private readonly element: HTMLElement,
    private readonly words: SettingsWords,
    private readonly renewing: Renewing,
  ) {
    element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-renew]");
      if (button && !button.disabled) this.act(button.dataset.renew as "renew" | "guardian");
    });
    this.ask();
  }

  leave(): void {
    this.left = true;
    clearTimeout(this.timer);
  }

  private ask(): void {
    clearTimeout(this.timer);
    this.renewing.state().then(
      (status) => {
        if (this.left) return;
        this.status = status;
        this.draw();
        if (status.phase && UNDER_WAY.includes(status.phase)) this.timer = window.setTimeout(() => this.ask(), 2000);
      },
      (error: unknown) => {
        if (this.left) return;
        this.said = String(error);
        this.draw();
      },
    );
  }

  private act(what: "renew" | "guardian"): void {
    if (this.busy) return;
    this.busy = true;
    this.said = "";
    this.draw();
    (what === "renew" ? this.renewing.renew() : this.renewing.guardian())
      .catch((error: unknown) => {
        this.said = this.words.renewFailed(String(error));
      })
      .finally(() => {
        this.busy = false;
        this.ask();
      });
  }

  private draw(): void {
    const { words } = this;
    const status = this.status;
    const node = status?.node?.build ?? null;
    const mismatch = status?.mismatch ?? null;
    let state = "";
    if (mismatch) state = words.versionOld(mismatch.node_version, mismatch.node_protocol, mismatch.client_version, mismatch.client_protocol);
    else if (node) state = words.versionHere(node.version);
    else if (status) state = words.versionNone;
    const phase = status?.phase ?? null;
    const step = phase ? words.renewPhase[phase] : "";
    const failed = phase === "failed" && status?.error ? words.renewFailed(status.error) : "";
    // Offered where there is a newer one to put in place and nothing is
    // already doing it.
    const offer = (mismatch || status?.pending) && (!phase || phase === "staged" || phase === "failed");
    const buttons = [
      offer ? `<button class="text-button quiet-button" type="button" data-renew="renew"${this.busy ? " disabled" : ""}>${escape(words.renew)}</button>` : "",
      phase === "awaiting_administrator"
        ? `<button class="text-button quiet-button" type="button" data-renew="guardian" title="${escape(words.guardianWhy)}"${this.busy ? " disabled" : ""}>${escape(words.guardian)}</button>`
        : "",
    ].join("");
    const notes = [step, failed, this.said].filter(Boolean);
    this.element.hidden = !state && !notes.length;
    this.element.innerHTML = `<dt>${escape(words.worldVersion)}</dt><dd><span>${escape(state)}</span>${buttons}${notes
      .map((note) => `<p class="settings-said" role="status">${escape(note)}</p>`)
      .join("")}</dd>`;
  }
}
