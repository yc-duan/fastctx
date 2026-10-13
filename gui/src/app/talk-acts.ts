// What the user does to a conversation from its "⋯" (`world-gui.md`, 界面的
// 结构): move it to a project, rename it, archive it or bring it back, delete
// it. Each shows done at once and goes to the machine the conversation lives
// on, one after another for one conversation, in the order they were done
// (`world-network.md`, 隔着网络也要顺手). What is shown stands until the World
// says the same; one refused gives way to what the World has, and why is said
// once. Archiving can be undone for a moment.

import type { Controlled } from "../world/control";
import type { Address } from "../world/events";
import type { Forgetting, Managed, Management } from "../world/management";
import type { ResidentNow } from "../world/world";
import { titleOf } from "../world/names";
import { signal } from "./signals";
import type { TalkMenuWords } from "./talk-menu-words";
import { addressOf, sameReach } from "./store";

export interface TalkActsOptions {
  words: TalkMenuWords;
  manage(address: Address, action: Management): Promise<Managed>;
  rename(address: Address, name: string): Promise<Controlled>;
  forget(address: Address): Promise<Forgetting>;
  /** Says something once, with an undo for a moment where there is one;
   *  `detail` is kept for hovering. */
  say(text: string, options?: { undo?: () => void; detail?: string }): void;
  /** Asks the World again, after an act came back. */
  refresh(): void;
}

/** What the user did to one conversation that the World has not said back
 *  yet: each part on its own. */
interface Shown {
  /** Archived, or brought back. */
  archived?: boolean;
  deleting?: boolean;
  town?: { town: string | null };
  name?: string;
}

const keyOf = (address: Address) => `${address.machine}\n${address.resident}`;

export class TalkActs {
  /** Moves whenever what is shown of a conversation changed here. */
  readonly changed = signal(0);
  private readonly shown = new Map<string, Shown>();
  /** Each conversation's acts, one after another. */
  private readonly chains = new Map<string, Promise<void>>();
  /** How many acts of each conversation have not come back: what it
   *  shows stands meanwhile, whatever the World says. */
  private readonly pending = new Map<string, number>();

  constructor(private readonly options: TalkActsOptions) {}

  /** `r` as it is to be shown: what the user did shown done. */
  view(r: ResidentNow): ResidentNow {
    const shown = this.shown.get(keyOf(addressOf(r)));
    if (!shown) return r;
    const now = r.now && shown.name !== undefined ? { ...r.now, title: shown.name } : r.now;
    const copy: ResidentNow = {
      ...r,
      town: shown.town ? shown.town.town : r.town,
      removed: shown.archived === undefined ? (r.removed ?? null) : shown.archived ? (r.removed ?? new Date().toISOString()) : null,
      now,
    };
    sameReach(copy, r);
    return copy;
  }

  deleting(address: Address): boolean {
    return !!this.shown.get(keyOf(address))?.deleting;
  }

  /** Lets go of what the World now says too: of a conversation gone, all;
   *  the part a refusal undid was let go of when it came back. */
  settle(listed: (address: Address) => ResidentNow | null): void {
    let changed = false;
    for (const [key, shown] of this.shown) {
      if (this.pending.get(key)) continue;
      const [machine, resident] = key.split("\n") as [string, string];
      const r = listed({ machine, resident });
      if (!r) {
        this.shown.delete(key);
        changed = true;
        continue;
      }
      if (shown.archived !== undefined && !!r.removed === shown.archived) delete shown.archived;
      if (shown.town && (r.town ?? null) === shown.town.town) delete shown.town;
      if (shown.name !== undefined && titleOf(r) === shown.name) delete shown.name;
      if (Object.keys(shown).length === 0) {
        this.shown.delete(key);
        changed = true;
      }
    }
    if (changed) this.changed.update((n) => n + 1);
  }

  archive(r: ResidentNow): void {
    const name = this.nameOf(r);
    this.show(r, { archived: true });
    this.options.say(this.options.words.archivedSaid(name), { undo: () => this.restore(r, true) });
    this.manageAs(r, { do: "remove" }, "archived", this.options.words.notArchived);
  }

  /** `quiet` when undoing an archiving just said. */
  restore(r: ResidentNow, quiet = false): void {
    this.show(r, { archived: false });
    if (!quiet) this.options.say(this.options.words.restoredSaid(this.nameOf(r)));
    this.manageAs(r, { do: "restore" }, "archived", this.options.words.notRestored);
  }

  move(r: ResidentNow, town: string | null): void {
    this.show(r, { town: { town } });
    this.manageAs(r, { do: "move", town }, "town", this.options.words.notMoved);
  }

  rename(r: ResidentNow, name: string): void {
    this.show(r, { name });
    const address = addressOf(r);
    this.chain(address, async () => {
      try {
        const done = await this.options.rename(address, name);
        if (done.kind === "refused") this.undo(address, "name", this.options.words.notRenamed, done.said);
      } catch (error) {
        this.undo(address, "name", this.options.words.notRenamed, String(error instanceof Error ? error.message : error));
      }
    });
  }

  delete(r: ResidentNow): void {
    this.show(r, { deleting: true });
    const address = addressOf(r);
    this.chain(address, async () => {
      try {
        await this.options.forget(address);
      } catch (error) {
        this.undo(address, "deleting", this.options.words.notDeleted, String(error instanceof Error ? error.message : error));
      }
    });
  }

  private nameOf(r: ResidentNow): string {
    return titleOf(this.view(r)) ?? r.about.id;
  }

  private show(r: ResidentNow, part: Shown): void {
    const key = keyOf(addressOf(r));
    this.shown.set(key, { ...this.shown.get(key), ...part });
    this.changed.update((n) => n + 1);
  }

  /** A management act, its part let go of with why when any stage of it
   *  was not done. */
  private manageAs(r: ResidentNow, action: Management, part: keyof Shown, refused: string): void {
    const address = addressOf(r);
    this.chain(address, async () => {
      try {
        const managed = await this.options.manage(address, action);
        const undone = managed.steps.filter((step) => step.outcome === "failed");
        if (undone.length) this.undo(address, part, refused, undone.map((step) => step.detail).join("\n"));
      } catch (error) {
        this.undo(address, part, refused, String(error instanceof Error ? error.message : error));
      }
    });
  }

  private undo(address: Address, part: keyof Shown, said: string, detail: string): void {
    const shown = this.shown.get(keyOf(address));
    if (shown) {
      delete shown[part];
      if (Object.keys(shown).length === 0) this.shown.delete(keyOf(address));
      this.changed.update((n) => n + 1);
    }
    this.options.say(said, { detail });
  }

  private chain(address: Address, act: () => Promise<void>): void {
    const key = keyOf(address);
    this.pending.set(key, (this.pending.get(key) ?? 0) + 1);
    const next = (this.chains.get(key) ?? Promise.resolve()).then(act).finally(() => {
      const left = (this.pending.get(key) ?? 1) - 1;
      if (left) this.pending.set(key, left);
      else this.pending.delete(key);
      if (this.chains.get(key) === next) this.chains.delete(key);
      this.options.refresh();
    });
    this.chains.set(key, next);
  }
}
