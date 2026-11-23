// How long the conversations a machine runs are kept idle before they are
// deleted with all they keep, in its page's 「管理」, for its owner: one
// setting for their own conversations and one, a ceiling, for those of the
// accounts its agents assist. Off until set; turned on or shortened, nothing
// goes for seven days, and the owner sees first how many conversations it
// would take.

import { escape } from "../conversation/prose";
import { RETENTION_DAYS, type RetentionAnswer, type RetentionAsk } from "../world/retention";
import type { Confirm } from "./confirm";
import { setRow } from "./machine-set";
import { detailOf } from "./problem";
import type { Machine } from "./reach";

export interface RetentionWords {
  title: string;
  own: string;
  ownWhat: string;
  assisted: string;
  assistedWhat: string;
  off: string;
  days(n: number): string;
  /** Nothing goes before this, the rule having been turned on or shortened. */
  graceUntil(when: string): string;
  /** How many go first once a setting is kept. */
  dueSoon(count: number): string;
  confirmTitle: string;
  confirm(days: number, count: number, machine: string): string;
  confirmGo: string;
  asking: string;
  unread(why: string): string;
}

export interface RetentionOptions {
  words: RetentionWords;
  ask(machine: string, ask: RetentionAsk): Promise<RetentionAnswer>;
  confirm: Confirm;
  changed(): void;
}

type Setting = "own" | "assisted";

export class RetentionBlock {
  private shown: string | null = null;
  private machine: Machine | null = null;
  private answer: RetentionAnswer | null = null;
  private problem: string | null = null;
  private busy = false;

  constructor(private readonly options: RetentionOptions) {}

  show(machine: Machine | null, shown: boolean): void {
    if ((machine?.written ?? null) !== this.shown) {
      this.shown = machine?.written ?? null;
      this.answer = null;
      this.problem = null;
    }
    this.machine = machine;
    if (shown && machine && machine.online !== false && this.answer === null && this.problem === null && !this.busy) {
      void this.run({ do: "read" });
    }
  }

  private async run(ask: RetentionAsk, machine: Machine | null = this.machine): Promise<void> {
    if (!machine || this.busy) return;
    this.busy = true;
    this.options.changed();
    try {
      const answer = await this.options.ask(machine.written, ask);
      if (machine.written !== this.shown) return;
      this.answer = answer;
      this.problem = null;
    } catch (error) {
      if (machine.written === this.shown) this.problem = detailOf(error);
    } finally {
      this.busy = false;
      this.options.changed();
    }
  }

  html(): string {
    const { words } = this.options;
    if (!this.machine) return "";
    const head = `<h2 class="mp-sec-title">${escape(words.title)}</h2>`;
    if (this.problem) return `${head}${setRow(escape(words.own), `<span class="fx-wrong">${escape(words.unread(this.problem))}</span>`, "")}`;
    const answer = this.answer;
    if (!answer) return `${head}${setRow(escape(words.own), `<span class="fx-quiet">${escape(words.asking)}</span>`, "")}`;
    const rule = answer.rule;
    const choices = (setting: Setting, chosen: number | undefined) => {
      const disabled = this.busy ? " disabled" : "";
      const option = (n: number | null) => {
        const on = (n ?? undefined) === chosen;
        const label = n === null ? words.off : words.days(n);
        return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-act="rt-set" data-setting="${setting}" data-days="${n ?? ""}"${disabled}>${escape(label)}</button>`;
      };
      return `<div class="choice-options" role="radiogroup">${[null, ...RETENTION_DAYS].map(option).join("")}</div>`;
    };
    const rows = [
      setRow(escape(words.own), escape(words.ownWhat), choices("own", rule.own_days)),
      setRow(escape(words.assisted), escape(words.assistedWhat), choices("assisted", rule.assisted_days)),
    ];
    const notes: string[] = [];
    if (rule.grace_until && Date.parse(rule.grace_until) > Date.now()) {
      notes.push(words.graceUntil(new Date(rule.grace_until).toLocaleString()));
    }
    const soon = answer.conversations.filter((one) => one.at && Date.parse(one.at) <= Date.now() + 7 * 86_400_000).length;
    if (soon) notes.push(words.dueSoon(soon));
    if (notes.length) rows.push(setRow("", `<span class="fx-quiet">${escape(notes.join(" "))}</span>`, ""));
    return `${head}${rows.join("")}`;
  }

  /** How many of the conversations shown would have been idle `days` days
   *  by the end of the grace. */
  private wouldGo(days: number, setting: Setting): number {
    const answer = this.answer;
    if (!answer) return 0;
    const graceEnd = Date.now() + 7 * 86_400_000;
    // The owner's own and the assisted ones cannot be told apart here; the
    // count is of all those the setting could reach, at most.
    void setting;
    return answer.conversations.filter((one) => Date.parse(one.idle_since) + days * 86_400_000 <= graceEnd).length;
  }

  click(target: HTMLElement): boolean {
    if (target.dataset.act !== "rt-set") return false;
    const machine = this.machine;
    const rule = this.answer?.rule;
    if (!machine || !rule) return true;
    const setting = target.dataset.setting as Setting;
    const days = target.dataset.days ? Number(target.dataset.days) : null;
    const before = setting === "own" ? rule.own_days : rule.assisted_days;
    const ask: RetentionAsk = {
      do: "owner",
      own_days: setting === "own" ? days : (rule.own_days ?? null),
      assisted_days: setting === "assisted" ? days : (rule.assisted_days ?? null),
    };
    // Turned on or shortened, it is confirmed with what it would take.
    if (days !== null && (before === undefined || days < before)) {
      const { words } = this.options;
      this.options.confirm.ask({
        title: words.confirmTitle,
        what: words.confirm(days, this.wouldGo(days, setting), machine.name),
        go: words.confirmGo,
        act: () => this.run(ask, machine),
        done: () => this.options.changed(),
      });
      return true;
    }
    void this.run(ask, machine);
    return true;
  }
}
