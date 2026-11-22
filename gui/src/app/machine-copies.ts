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
import { setRow } from "./machine-set";
import { detailOf } from "./problem";
import type { Machine } from "./reach";

export interface CopiesWords {
  title: string;
  /** The setting, of the one account or of each when there are several. */
  takeIn: string;
  takeInOf(account: string): string;
  /** What the copies are, and why a server goes without them. */
  what: string;
  takenIn: string;
  paused: string;
  pause: string;
  resume: string;
  /** Those kept while paused, and how much room they take. */
  held: string;
  heldSize(count: number, size: string): string;
  clear: string;
  none: string;
  confirmTitle: string;
  confirm(account: string, count: number, size: string, machine: string): string;
  clearGo(count: number): string;
  cleared: string;
  clearedSaid(count: number, size: string): string;
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
    const head = `<h2 class="mp-sec-title">${escape(words.title)}</h2>`;
    const alone = (said: string, kind = "") => `${head}${setRow(escape(words.takeIn), `<span${kind ? ` class="${kind}"` : ""}>${escape(said)}</span>`, "")}`;
    if (this.problem) return alone(words.unread(this.problem), "fx-wrong");
    if (!this.accounts) return alone(words.asking, "fx-quiet");
    if (!this.accounts.length) return alone(words.noAccount);
    const several = this.accounts.length > 1;
    const hubs = new Set(this.accounts.map((one) => one.hub)).size > 1;
    const disabled = this.busy ? " disabled" : "";
    const rows = this.accounts.map((one, index) => {
      const data = `data-hub="${escape(one.hub)}" data-account="${escape(one.account)}"`;
      const button = (act: string, label: string) => `<button class="text-button quiet-button" type="button" data-act="${act}" ${data}${disabled}>${escape(label)}</button>`;
      const name = several ? words.takeInOf(hubs ? `${one.account} · ${one.hub}` : one.account) : words.takeIn;
      const state = `<span>${escape(one.on ? words.takenIn : words.paused)}</span>`;
      // What the copies are, said once, under the first.
      const what = index === 0 ? escape(words.what) : "";
      const parts = [setRow(escape(name), what, `${state}${one.on ? button("cp-pause", words.pause) : button("cp-on", words.resume)}`)];
      const outcome = this.outcome.get(CopiesBlock.key(one.hub, one.account));
      if (!one.on && one.paused_scopes && !outcome?.applied) {
        const counted = outcome && !outcome.scopes;
        const end = counted ? `<span class="fx-quiet">${escape(words.none)}</span>` : button("cp-clear", words.clear);
        parts.push(setRow(escape(words.held), escape(words.heldSize(one.paused_scopes, bytesSaid(one.paused_bytes ?? 0))), end));
      }
      if (outcome?.applied) {
        const said = [
          words.clearedSaid(outcome.scopes, bytesSaid(outcome.bytes)),
          outcome.kept.length ? words.kept(outcome.kept.length) : "",
          outcome.failed ? words.stopped(outcome.failed) : "",
          words.compact,
        ].filter(Boolean);
        parts.push(setRow(escape(words.cleared), escape(said.join(" ")), ""));
      }
      return parts.join("");
    });
    return `${head}${rows.join("")}`;
  }

  /** Once counted, what may go is let go as the user confirms it: only
   *  what the count said, on the machine it was said of, so a copy made or
   *  held anew since is not let go unseen. */
  private offerClear(hub: string, account: string, machine: Machine): void {
    const counted = this.outcome.get(CopiesBlock.key(hub, account));
    if (!counted || counted.applied || !counted.scopes || machine.written !== this.shown) return;
    const { words } = this.options;
    this.options.confirm.ask({
      title: words.confirmTitle,
      what: words.confirm(account, counted.scopes, bytesSaid(counted.bytes), machine.name),
      go: words.clearGo(counted.scopes),
      act: () => this.run({ do: "clear", hub, account, apply: true, ...(counted.keys ? { only: counted.keys } : {}) }, machine),
      done: () => this.options.changed(),
    });
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
      case "cp-clear": {
        // Counted first, then confirmed with what the count found.
        const machine = this.machine;
        if (!machine) return true;
        void this.run({ do: "clear", hub, account, apply: false }).then(() => this.offerClear(hub, account, machine));
        return true;
      }
    }
    return false;
  }
}
