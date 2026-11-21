// The copies a machine holds of the conversations living on other machines,
// in its page's 「管理」, for its owner (storage relief; the user, 2026-11-19:
// servers keep copies nobody reads there): each account's, taken in or
// paused, how many paused and about how much room they take; pausing,
// taking in again, counting what may be let go and letting it go. What is
// let go lives on where its conversation lives, and is taken whole again if
// it is taken in again; its room comes back once the machine's library is
// compacted there.

import { bytesSaid } from "../bytes";
import { escape } from "../conversation/prose";
import type { Cleared, CopiesAnswer, CopiesAsk, CopiesOf } from "../world/data";
import type { Confirm } from "./confirm";
import { detailOf } from "./problem";
import type { Machine } from "./reach";

export interface CopiesWords {
  title: string;
  /** What the copies are, and why a server lets them go. */
  what: string;
  takenIn: string;
  paused: string;
  pausedHeld(count: number, size: string): string;
  pause: string;
  takeIn: string;
  count: string;
  clearable(count: number, size: string): string;
  none: string;
  clear(count: number): string;
  confirmTitle: string;
  confirm(account: string, count: number, size: string, machine: string): string;
  cleared(count: number, size: string): string;
  kept(count: number): string;
  /** Letting go stopped short after some went. */
  stopped(why: string): string;
  /** Where its room comes back. */
  compact: string;
  asking: string;
  noAccount: string;
  unread(why: string): string;
}

export interface CopiesOptions {
  words: CopiesWords;
  ask(machine: string, ask: CopiesAsk): Promise<CopiesAnswer>;
  confirm: Confirm;
  changed(): void;
}

export class CopiesBlock {
  /** The machine it is of, as the World writes it: one of each network's
   *  of a name is another machine. */
  private shown: string | null = null;
  private machine: Machine | null = null;
  private accounts: CopiesOf[] | null = null;
  private problem: string | null = null;
  /** What was counted or let go, by account. */
  private outcome = new Map<string, Cleared>();
  private busy = false;

  constructor(private readonly options: CopiesOptions) {}

  /** The machine it is of, and whether it is in view: asked once shown. */
  show(machine: Machine | null, shown: boolean): void {
    if ((machine?.written ?? null) !== this.shown) {
      this.shown = machine?.written ?? null;
      this.accounts = null;
      this.problem = null;
      this.outcome.clear();
    }
    this.machine = machine;
    if (shown && machine && machine.online !== false && this.accounts === null && this.problem === null && !this.busy) {
      void this.run({ do: "report" });
    }
  }

  private static key(hub: string, account: string): string {
    return `${hub}\n${account}`;
  }

  /** Asks `machine` — the one shown as it was asked, by default — and keeps
   *  what it answers while that one is shown still. */
  private async run(ask: CopiesAsk, machine: Machine | null = this.machine): Promise<void> {
    if (!machine || this.busy) return;
    this.busy = true;
    this.options.changed();
    try {
      const answer = await this.options.ask(machine.written, ask);
      if (machine.written !== this.shown) return;
      if (answer.copies === "report") {
        this.accounts = answer.accounts;
        this.problem = null;
      } else if (ask.do === "clear") {
        const { copies: _, ...cleared } = answer;
        this.outcome.set(CopiesBlock.key(ask.hub, ask.account), cleared);
        if (cleared.applied) {
          // What it came to stays said, the counts after it asked again where
          // they can be: not asked now, they are as the page is shown again.
          try {
            const report = await this.options.ask(machine.written, { do: "report" });
            if (machine.written === this.shown && report.copies === "report") this.accounts = report.accounts;
          } catch {
            // Kept as it was.
          }
        }
      }
    } catch (error) {
      if (machine.written === this.shown) this.problem = detailOf(error);
    } finally {
      this.busy = false;
      this.options.changed();
    }
  }

  html(): string {
    const { words } = this.options;
    const machine = this.machine;
    if (!machine) return "";
    const head = `<h3 class="mp-sec-title">${escape(words.title)}</h3><p class="mp-sec-note">${escape(words.what)}</p>`;
    if (this.problem) return `${head}<p class="mp-sec-note fx-wrong">${escape(words.unread(this.problem))}</p>`;
    if (!this.accounts) return `${head}<p class="mp-sec-note fx-quiet">${escape(words.asking)}</p>`;
    if (!this.accounts.length) return `${head}<p class="mp-sec-note">${escape(words.noAccount)}</p>`;
    const several = new Set(this.accounts.map((one) => one.hub)).size > 1;
    const disabled = this.busy ? " disabled" : "";
    const rows = this.accounts.map((one) => {
      const data = `data-hub="${escape(one.hub)}" data-account="${escape(one.account)}"`;
      const name = several ? `${one.account} · ${one.hub}` : one.account;
      const state = one.on ? words.takenIn : words.paused;
      const held = one.paused_scopes ? words.pausedHeld(one.paused_scopes, bytesSaid(one.paused_bytes ?? 0)) : "";
      const turn = one.on
        ? `<button class="text-button quiet-button" type="button" data-act="cp-pause" ${data}${disabled}>${escape(words.pause)}</button>`
        : `<button class="text-button quiet-button" type="button" data-act="cp-on" ${data}${disabled}>${escape(words.takeIn)}</button>`;
      const outcome = this.outcome.get(CopiesBlock.key(one.hub, one.account));
      let clear = "";
      if (!one.on && one.paused_scopes) {
        if (!outcome || outcome.applied) {
          clear = `<button class="text-button quiet-button" type="button" data-act="cp-count" ${data}${disabled}>${escape(words.count)}</button>`;
        } else if (outcome.scopes) {
          clear = `<span>${escape(words.clearable(outcome.scopes, bytesSaid(outcome.bytes)))}</span><button class="text-button quiet-button danger" type="button" data-act="cp-clear" ${data}${disabled}>${escape(words.clear(outcome.scopes))}</button>`;
        } else {
          clear = `<span class="fx-quiet">${escape(words.none)}</span>`;
        }
      }
      // Each line past the first under no name of its own, as the page's
      // other rows go.
      const done = outcome?.applied
        ? `<dt></dt><dd class="fx-status"><span>${escape(words.cleared(outcome.scopes, bytesSaid(outcome.bytes)))}${outcome.kept.length ? ` ${escape(words.kept(outcome.kept.length))}` : ""}${outcome.failed ? ` ${escape(words.stopped(outcome.failed))}` : ""}</span></dd><dt></dt><dd class="fx-status"><span class="fx-quiet">${escape(words.compact)}</span></dd>`
        : "";
      return `<dt>${escape(name)}</dt><dd class="fx-facts"><span>${escape(state)}${held ? `<span class="sep"></span>${escape(held)}` : ""}</span></dd><dt></dt><dd class="fx-status">${turn}${clear}</dd>${done}`;
    });
    return `${head}<dl class="mp-rows">${rows.join("")}</dl>`;
  }

  /** The block's own buttons; whether the press was one of them. */
  click(target: HTMLElement): boolean {
    const act = target.dataset.act;
    if (!act?.startsWith("cp-")) return false;
    const hub = target.dataset.hub ?? "";
    const account = target.dataset.account ?? "";
    switch (act) {
      case "cp-pause":
        void this.run({ do: "set", hub, account, on: false });
        return true;
      case "cp-on":
        this.outcome.delete(CopiesBlock.key(hub, account));
        void this.run({ do: "set", hub, account, on: true });
        return true;
      case "cp-count":
        void this.run({ do: "clear", hub, account, apply: false });
        return true;
      case "cp-clear": {
        const counted = this.outcome.get(CopiesBlock.key(hub, account));
        const machine = this.machine;
        if (!counted || !machine) return true;
        const { words } = this.options;
        this.options.confirm.ask({
          title: words.confirmTitle,
          what: words.confirm(account, counted.scopes, bytesSaid(counted.bytes), machine.name),
          go: words.clear(counted.scopes),
          // Only what the count said, on the machine it was said of: a copy
          // made or held anew since is not let go unseen.
          act: () => this.run({ do: "clear", hub, account, apply: true, ...(counted.keys ? { only: counted.keys } : {}) }, machine),
          done: () => this.options.changed(),
        });
        return true;
      }
    }
    return false;
  }
}
