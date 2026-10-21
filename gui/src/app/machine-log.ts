// The hub's record of changes to machines, opened from the drawing's corner
// (`world-gui.md`, 账号、机器、共享与协助): who asked, from which device, what
// changed, and whether it was done. On a phone it rises from below.

import { glyph } from "../conversation/glyphs";
import { layerOpened } from "./back";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { MachineLogPage, MachineLogRecord } from "../world/machine-log";
import type { MachinesWords } from "./account-words";

export type ReadLog = (hub: string, account: string, before: number | null) => Promise<MachineLogPage>;

export class MachineLog {
  readonly button: HTMLButtonElement;
  readonly panel: HTMLElement;
  private chosen = 0;
  private records: MachineLogRecord[] = [];
  private next: number | null = null;
  private state: "idle" | "loading" | "failed" = "idle";
  private why = "";

  constructor(
    private readonly words: MachinesWords,
    private readonly read: ReadLog,
    private readonly logins: () => Login[],
    private readonly when: (at: Date) => string,
  ) {
    this.button = document.createElement("button");
    this.button.type = "button";
    this.button.className = "map-log-open";
    this.button.setAttribute("aria-label", words.log);
    this.button.title = words.log;
    this.button.setAttribute("aria-expanded", "false");
    this.button.innerHTML = glyph("history", 16);
    this.panel = document.createElement("div");
    this.panel.className = "map-log";
    this.panel.setAttribute("role", "dialog");
    this.panel.setAttribute("aria-label", words.log);
    this.panel.hidden = true;
    this.button.addEventListener("click", () => (this.panel.hidden ? this.open() : this.close()));
    this.panel.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const login = target.closest<HTMLElement>("[data-log-login]");
      if (login) {
        this.chosen = Number(login.dataset.logLogin);
        this.records = [];
        this.next = null;
        void this.load();
      } else if (target.closest("[data-log-earlier]")) void this.load();
      else if (target.closest("[data-log-close]")) this.close();
    });
    this.panel.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        this.close();
      }
    });
    document.addEventListener("pointerdown", this.outside, true);
  }

  dispose(): void {
    document.removeEventListener("pointerdown", this.outside, true);
  }

  private readonly outside = (event: Event): void => {
    const target = event.target as Node;
    if (!this.panel.hidden && !this.panel.contains(target) && !this.button.contains(target)) this.close();
  };

  private open(): void {
    this.panel.classList.toggle("as-sheet", matchMedia("(max-width: 760px)").matches);
    layerOpened({ isOpen: () => !this.panel.hidden && this.panel.isConnected, close: () => this.close(), holds: (target) => this.panel.contains(target) });
    this.panel.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    this.records = [];
    this.next = null;
    void this.load();
  }

  private close(): void {
    this.panel.hidden = true;
    this.button.setAttribute("aria-expanded", "false");
  }

  /** Reads the next page, older than what is shown, for the login chosen. */
  private async load(): Promise<void> {
    const login = this.logins()[this.chosen] ?? this.logins()[0];
    if (!login) return;
    this.state = "loading";
    this.draw();
    try {
      const page = await this.read(login.hubName, login.account, this.records.length ? this.next : null);
      this.records.push(...page.records);
      this.next = page.next_before;
      this.state = "idle";
    } catch (error) {
      this.state = "failed";
      this.why = error instanceof Error ? error.message : String(error);
    }
    this.draw();
  }

  private draw(): void {
    const { words } = this;
    const logins = this.logins();
    const choice =
      logins.length > 1
        ? `<div class="map-log-logins">${logins
            .map((login, index) => `<button class="chip${index === this.chosen ? " current" : ""}" type="button" data-log-login="${index}">${escape(`${login.hubName} ${login.account}`)}</button>`)
            .join("")}</div>`
        : "";
    const rows = this.records
      .map((record) => this.row(record))
      .filter(Boolean)
      .join("");
    const tail =
      this.state === "loading"
        ? `<p class="map-log-note">…</p>`
        : this.state === "failed"
          ? `<p class="map-log-note wrong" title="${escape(this.why)}">${escape(words.logFailed)}</p>`
          : this.next !== null
            ? `<button class="text-button map-log-earlier" type="button" data-log-earlier>${escape(words.logEarlier)}</button>`
            : rows
              ? ""
              : `<p class="map-log-note">${escape(words.logEmpty)}</p>`;
    this.panel.innerHTML = `<header class="map-log-head"><span>${escape(words.log)}</span><button class="icon-button" type="button" data-log-close aria-label="${escape(words.closeDetails)}">${glyph("close", 14)}</button></header>${choice}<ol class="map-log-list">${rows}</ol>${tail}`;
  }

  /** One change in a line: a tick or a cross, what changed, who asked and
   *  when; a refusal keeps the hub's reason at hand. */
  private row(record: MachineLogRecord): string {
    const { words } = this;
    const said = words.logSaid(record.action);
    if (!said) return "";
    const done = record.outcome.kind === "done";
    const by = [words.logBy(record.actor.account, record.actor.member), record.actor.authenticated ? "" : words.logUnverified, this.when(new Date(record.at))]
      .filter(Boolean)
      .join(" ");
    const why = record.outcome.kind === "refused" ? record.outcome.reason : "";
    return `<li class="map-log-row${done ? "" : " refused"}"${why ? ` title="${escape(why)}"` : ""}><span class="map-log-mark">${glyph(done ? "check" : "cross", 14)}</span><span class="map-log-said">${escape(said)}</span><span class="map-log-by">${escape(by)}</span></li>`;
  }
}
