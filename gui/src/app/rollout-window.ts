// The sheet of a release going out a batch at a time (`specs/world-engine.md`,
// 发布与推广): which release, how many of the network's other machines take
// it, and the machines named to take it first, found by name among the
// computers the network updates; pausing it, going on with it and ending it.
// The sheet keeps its size; only the list of machines scrolls.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { MachineUpdate, Policy } from "../world/updates";
import { Sheet } from "./confirm";
import { runningOf, shortVersion } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";

/** The shares of the other machines to pick from, in hundredths. */
const SHARES = [0, 10, 25, 50, 100];
/** The most machines listed at once; finding narrows them. */
const LISTED = 400;

export interface RolloutNow {
  policy: Policy;
  /** The machines it names first, by their names now. */
  first: string[];
  /** The network's computers it may go to. */
  machines: MachineUpdate[];
  /** The kind of computer each is, by its name, for its mark. */
  form(machine: string): "desktop" | "laptop" | "server";
  /** The release line the versions shown share, left unsaid. */
  line: string | null;
}

export interface RolloutHost {
  /** Sets the rollout of `version`, or ends it with none. */
  set(version: string | null, first: string[], share: number): Promise<void>;
  pause(paused: boolean): Promise<void>;
  /** Opens the window of versions on another to roll out. */
  another(current: string, pick: (version: string) => void): void;
  /** A call that failed, in plain words, the kernel's own behind them. */
  failed(error: unknown): { said: string; why: string };
}

export class RolloutWindow {
  private sheet: Sheet | null = null;
  private now: RolloutNow | null = null;
  private host: RolloutHost | null = null;
  // What is being set: the release, the share of the others, the first.
  private version = "";
  private share = 0;
  private first = new Set<string>();
  private find = "";
  private busy = false;
  private said = "";
  private why = "";
  private drawn = "";

  constructor(
    private readonly words: UpdateViewWords,
    /** What the sheet's close button is called. */
    private readonly close: string,
  ) {}

  /** Opens on the rollout now, or on `version` to roll out. */
  open(now: RolloutNow, host: RolloutHost, version?: string): void {
    this.now = now;
    this.host = host;
    const rollout = now.policy.rollout;
    this.version = version ?? rollout?.version ?? "";
    // The same release keeps how it goes; another starts with none of the
    // others, the machines named before named again.
    this.share = rollout && rollout.version === this.version ? rollout.share : 0;
    this.first = new Set(now.first);
    this.find = "";
    this.busy = false;
    this.said = "";
    this.drawn = "";
    const sheet = this.sheet ?? this.make();
    this.draw();
    sheet.show();
  }

  /** The network changed while the sheet is open. */
  update(now: RolloutNow): void {
    if (!this.sheet?.dialog.open) return;
    this.now = now;
    this.draw();
  }

  private make(): Sheet {
    const sheet = new Sheet("ro-sheet");
    this.sheet = sheet;
    sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const share = target.closest<HTMLElement>("[data-ro-share]")?.dataset.roShare;
      if (share !== undefined) {
        this.share = Number(share);
        this.draw();
        return;
      }
      const machine = target.closest<HTMLElement>("[data-ro-machine]")?.dataset.roMachine;
      if (machine !== undefined) {
        if (!this.first.delete(machine)) this.first.add(machine);
        this.draw();
        return;
      }
      if (target.closest("[data-ro-another]")) {
        this.host?.another(this.version, (version) => {
          if (version === this.version) return;
          this.version = version;
          const rollout = this.now?.policy.rollout;
          this.share = rollout && rollout.version === version ? rollout.share : 0;
          this.draw();
        });
        return;
      }
      const act = target.closest<HTMLElement>("[data-ro-act]")?.dataset.roAct;
      if (act) this.act(act as "go" | "pause" | "resume" | "end");
    });
    sheet.body.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (!input.matches(".ro-find")) return;
      this.find = input.value;
      this.draw();
    });
    return sheet;
  }

  private act(act: "go" | "pause" | "resume" | "end"): void {
    const { host, sheet } = this;
    if (!host || !sheet || this.busy) return;
    this.busy = true;
    sheet.busy = true;
    this.said = "";
    this.draw();
    const asked =
      act === "go"
        ? host.set(this.version, [...this.first], this.share)
        : act === "end"
          ? host.set(null, [], 0)
          : host.pause(act === "pause");
    asked.then(
      () => {
        this.busy = false;
        sheet.busy = false;
        // Pausing and going on stay to be seen; the rest is done with.
        if (act === "go" || act === "end") sheet.close();
        else this.draw();
      },
      (error: unknown) => {
        this.busy = false;
        sheet.busy = false;
        ({ said: this.said, why: this.why } = host.failed(error));
        this.draw();
      },
    );
  }

  private draw(): void {
    const { sheet, now, words } = this;
    if (!sheet || !now) return;
    const rollout = now.policy.rollout ?? null;
    const same = !!rollout && rollout.version === this.version;
    const named = new Set(now.first);
    const changed = !same || rollout.share !== this.share || named.size !== this.first.size || [...this.first].some((name) => !named.has(name));
    const disabled = this.busy ? " disabled" : "";
    const shares = SHARES.map(
      (share) =>
        `<button type="button" class="choice${share === this.share ? " chosen" : ""}" role="radio" aria-checked="${share === this.share}" data-ro-share="${share}"${disabled}>${share}%</button>`,
    ).join("");
    // How it stands: stopped by the hub, with why behind the pointer, or
    // paused.
    const stopped = same ? rollout.stopped : undefined;
    const state = stopped
      ? `<p class="ro-state caution" title="${escape(words.rolloutStoppedWhy)}">${escape(words.rolloutStoppedBy(stopped.machine, words.troubles.what({ id: 0, at: stopped.at, kind: stopped.kind })))}</p>`
      : same && rollout.paused
        ? `<p class="ro-state">${escape(words.rolloutPaused)}</p>`
        : "";
    // The first named first, then the rest by name, as far as finding lets.
    const looked = this.find.trim().toLowerCase();
    const machines = now.machines
      .filter((machine) => !looked || machine.machine.toLowerCase().includes(looked))
      .sort((a, b) => Number(this.first.has(b.machine)) - Number(this.first.has(a.machine)) || a.machine.localeCompare(b.machine));
    const rows = machines
      .slice(0, LISTED)
      .map((machine) => {
        const chosen = this.first.has(machine.machine);
        const icon = now.form(machine.machine);
        const ran = runningOf(machine);
        const running = ran ? shortVersion(ran, now.line) : "";
        return `<li><button type="button" class="ro-machine${machine.online ? "" : " away"}" role="checkbox" aria-checked="${chosen}" data-ro-machine="${escape(machine.machine)}"${disabled}><span class="ro-check" aria-hidden="true"></span>${glyph(icon, 15)}<span class="ro-name">${escape(machine.machine)}</span><span class="ro-runs">${escape(running)}</span></button></li>`;
      })
      .join("");
    const list = rows || `<li class="ro-none">${escape(words.rolloutFound)}</li>`;
    const ends = rollout
      ? `<button class="text-button quiet-button" type="button" data-ro-act="end"${disabled}>${escape(words.rolloutEnd)}</button>`
      : "";
    const pausing = same
      ? `<button class="text-button quiet-button" type="button" data-ro-act="${rollout.paused ? "resume" : "pause"}"${disabled}>${escape(rollout.paused ? words.rolloutResume : words.rolloutPause)}</button>`
      : "";
    const go = `<button class="sheet-go" type="button" data-ro-act="go"${this.busy || !changed || !this.version ? " disabled" : ""}>${escape(same ? words.rolloutSave : words.rolloutStart)}</button>`;
    const html = `${sheet.head(words.rollout, this.close)}
      <div class="choice-row"><span class="choice-label">${escape(words.rolloutVersion)}</span><span class="ro-version"><span class="ml-num">${escape(this.version)}</span><button class="text-button quiet-button" type="button" data-ro-another${disabled}>${escape(words.rolloutAnother)}</button></span></div>
      <div class="choice-row"><span class="choice-label">${escape(words.rolloutShare)}</span><div class="choice-options" role="radiogroup" aria-label="${escape(words.rolloutShare)}">${shares}</div></div>
      ${state}
      <div class="ro-first-head"><span class="choice-label">${escape(words.rolloutFirst)}</span><label class="focus-search">${glyph("search", 16)}<input class="ro-find focus-filter" type="search" placeholder="${escape(words.rolloutFind)}" value="${escape(this.find)}" aria-label="${escape(words.rolloutFind)}" autocomplete="off" spellcheck="false"></label></div>
      <ol class="ro-machines">${list}</ol>
      <p class="sheet-note" role="alert"${this.said ? "" : " hidden"}${this.said && this.why ? ` title="${escape(this.why)}"` : ""}>${escape(this.said)}</p>
      <footer class="sheet-foot"><span class="ro-acts">${ends}${pausing}</span>${go}</footer>`;
    if (html === this.drawn) return;
    this.drawn = html;
    // Finding keeps its place and what is typed as the list under it
    // changes.
    const find = sheet.body.querySelector<HTMLInputElement>(".ro-find");
    const typing = !!find && document.activeElement === find;
    const caret = find?.selectionStart ?? null;
    const scrolled = sheet.body.querySelector<HTMLElement>(".ro-machines")?.scrollTop ?? 0;
    sheet.body.innerHTML = html;
    const listed = sheet.body.querySelector<HTMLElement>(".ro-machines");
    if (listed) listed.scrollTop = scrolled;
    if (typing) {
      const again = sheet.body.querySelector<HTMLInputElement>(".ro-find");
      again?.focus();
      if (again && caret !== null) again.setSelectionRange(caret, caret);
    }
  }
}
