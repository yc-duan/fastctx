// Machines chosen together, on the drawing, the list or the update view
// (`world-gui.md`, 账号、机器、共享与协助): a bar floating at the foot of the
// page, saying how many are chosen and what can be done to them together.
// It takes no room from the list, which keeps its width and its boxes, and
// lists none of them again: the rows chosen are washed where they stand.
// On the drawing and the list the bar shares them, groups them, takes them
// to the update view and takes them off; in the update view it updates them
// to a release, sets how they update and takes back what was scheduled. An
// action that does not reach all of them says how many it reaches, and
// pointed at, the rows it leaves stand back (`reach`). On a phone the bar
// lies along the bottom, a mark over a short word each.

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { compareVersions, type MachineUpdate, type UpdateState } from "../world/updates";
import type { AccessWords, MachinesWords } from "./account-words";
import type { BatchWords } from "./machine-batch-words";
import { groupingOf } from "./machine-groups";
import type { ReadLog } from "./machine-log";
import { holderLoginOf, removerLoginOf, updaterLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import type { RemoveMachineSheet } from "./machine-remove";
import { removeMachines } from "./machine-removing";
import type { WaysSheet } from "./machine-ways";
import { isVisitor, runsHub, sameLogin, type Machine } from "./reach";
import type { Signal } from "./signals";
import { runningOf } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";
import type { VersionWindow } from "./version-window";

/** One thing done to the machines chosen: its label, saying how many it
 *  reaches when not all of them; its word on a phone's bar; the keys of
 *  those it reaches. */
interface Verb {
  act: string;
  label: string;
  word: string;
  icon: GlyphName;
  reach: string[];
  primary?: boolean;
  danger?: boolean;
}

export interface ChosenOptions {
  words: MachinesWords;
  batch: BatchWords;
  manage: ManageWords;
  access: AccessWords;
  change(login: Login, change: AccountChange): Promise<void>;
  refresh(): Promise<void>;
  removing: RemoveMachineSheet;
  /** A hub's log, to follow the machines told to uninstall. */
  log: ReadLog;
  ways: WaysSheet;
  /** Shares `machines`, all `login`'s, with the same accounts. */
  share(machines: Machine[], login: Login): void;
  /** Puts `machines`, all one owner's, in one group. */
  group(machines: Machine[]): void;
  /** Lets the machines chosen go. */
  clear(): void;
  /** Shows the update view, those chosen still chosen. */
  toUpdates(): void;
  /** What the update view knows and does, where this device updates
   *  machines. */
  updates?:
    | {
        words: UpdateViewWords;
        of(machine: Machine): MachineUpdate | undefined;
        schedule(machines: MachineUpdate[], version: string | null): Promise<void>;
        /** Has the hub keep a repair for each, or call one kept off. */
        repair(machines: MachineUpdate[], cancel: boolean): Promise<void>;
        ways(machines: MachineUpdate[], auto: boolean | null, relays: boolean | null): Promise<void>;
        state: Signal<UpdateState | null>;
        versions: VersionWindow;
      }
    | undefined;
}

const keyOf = (machine: Machine) => `m:${machine.written}`;

export class ChosenBar {
  constructor(private readonly options: ChosenOptions) {}

  /** The bar, for `machines` chosen in the update view or elsewhere. */
  html(machines: Machine[], world: WorldNow, inUpdates: boolean): string {
    const { words, batch } = this.options;
    const verbs = inUpdates ? this.updateVerbs(machines) : this.verbs(machines, world);
    const button = (verb: Verb) => {
      const some = verb.reach.length < machines.length;
      const label = some ? batch.some(verb.label, verb.reach.length) : verb.label;
      return `<button class="ms-act${verb.primary ? " primary" : ""}${verb.danger ? " danger" : ""}" type="button" data-act="${verb.act}" data-reach="${escape(verb.reach.join("\n"))}"${some ? ` title="${escape(label)}"` : ""}><span class="ms-mark">${glyph(verb.icon, 18)}${some ? `<span class="ms-reach">${verb.reach.length}</span>` : ""}</span><span class="ms-label">${escape(label)}</span><span class="ms-word">${escape(verb.word)}</span></button>`;
    };
    return `<button class="icon-button ms-close" type="button" data-act="framed-clear" aria-label="${escape(batch.clear)}" title="${escape(batch.clear)}">${glyph("close", 16)}</button>
      <span class="ms-count" aria-live="polite">${escape(words.framedTitle(machines.length))}</span>
      <span class="ms-acts" style="--acts:${verbs.length}">${verbs.map(button).join("")}</span>`;
  }

  /** On the drawing and the list. */
  private verbs(machines: Machine[], world: WorldNow): Verb[] {
    const { batch } = this.options;
    const verbs: Verb[] = [];
    const share = sharable(machines, world);
    const group = groupable(machines, world);
    const update = this.options.updates ? machines.filter((machine) => updaterLoginOf(machine, world)) : [];
    const remove = machines.filter((machine) => removerLoginOf(machine, world));
    if (share) verbs.push({ act: "framed-share", label: batch.share, word: batch.bar.share, icon: "colleague", reach: share.machines.map(keyOf) });
    if (group.length) verbs.push({ act: "framed-group", label: batch.group, word: batch.bar.group, icon: "folder", reach: group.map(keyOf) });
    if (update.length) verbs.push({ act: "framed-update", label: batch.update, word: batch.bar.update, icon: "update", reach: update.map(keyOf) });
    if (remove.length) verbs.push({ act: "framed-remove", label: batch.remove, word: batch.bar.remove, icon: "close", reach: remove.map(keyOf), danger: true });
    return verbs;
  }

  /** In the update view: what each machine allows, as the hub says. */
  private updateVerbs(machines: Machine[]): Verb[] {
    const updates = this.options.updates;
    if (!updates) return [];
    const view = updates.words;
    const allowing = (allows: (update: MachineUpdate) => boolean) => machines.filter((machine) => {
      const update = updates.of(machine);
      return !!update && allows(update);
    });
    const verbs: Verb[] = [];
    const to = allowing((update) => update.actions.schedule.allowed);
    const ways = allowing((update) => update.actions.settings.allowed);
    const cancel = allowing((update) => update.actions.cancel.allowed && !!update.due);
    const repair = allowing((update) => !!update.actions.repair?.allowed);
    const unrepair = allowing((update) => !!update.actions.repair?.allowed && !!update.repair_due);
    if (to.length) verbs.push({ act: "framed-update-to", label: view.updateTo, word: view.bar.updateTo, icon: "update", reach: to.map(keyOf), primary: true });
    if (ways.length) verbs.push({ act: "framed-ways", label: view.ways, word: view.bar.ways, icon: "tune", reach: ways.map(keyOf) });
    if (cancel.length) verbs.push({ act: "framed-cancel", label: view.cancel, word: view.bar.cancel, icon: "cross", reach: cancel.map(keyOf) });
    if (repair.length) verbs.push({ act: "framed-repair", label: view.repair, word: view.bar.repair, icon: "repair", reach: repair.map(keyOf) });
    if (unrepair.length) verbs.push({ act: "framed-unrepair", label: view.cancelRepair, word: view.bar.cancelRepair, icon: "cross", reach: unrepair.map(keyOf) });
    return verbs;
  }

  /** The bar's own buttons; whether the press was one of them. */
  click(act: string, machines: Machine[], world: WorldNow): boolean {
    switch (act) {
      case "framed-share": {
        const share = sharable(machines, world);
        if (share) this.options.share(share.machines, share.login);
        return true;
      }
      case "framed-group":
        this.options.group(groupable(machines, world));
        return true;
      case "framed-update":
        this.options.toUpdates();
        return true;
      case "framed-update-to":
        this.updateTo(machines);
        return true;
      case "framed-ways":
        this.ways(machines);
        return true;
      case "framed-cancel":
        void this.cancel(machines);
        return true;
      case "framed-repair":
        void this.repair(machines, false);
        return true;
      case "framed-unrepair":
        void this.repair(machines, true);
        return true;
      case "framed-remove":
        this.remove(machines.filter((machine) => removerLoginOf(machine, world)), world);
        return true;
    }
    return false;
  }

  /** Those of `machines` the update view knows and that allow `allows`. */
  private known(machines: Machine[], allows: (update: MachineUpdate) => boolean): MachineUpdate[] {
    const updates = this.options.updates;
    return updates ? machines.flatMap((machine) => updates.of(machine) ?? []).filter(allows) : [];
  }

  /** The window of versions for those that can be updated, from the oldest
   *  of what they run; the release picked is scheduled for each. */
  private updateTo(machines: Machine[]): void {
    const updates = this.options.updates;
    const local = updates?.state.peek();
    const targets = this.known(machines, (update) => update.actions.schedule.allowed);
    if (!updates || !local || !targets.length) return;
    const { batch } = this.options;
    const runs = targets.flatMap((update) => runningOf(update) ?? []).sort(compareVersions);
    const oldest = runs[0];
    if (!oldest) return;
    const same = runs.every((version) => version === oldest);
    updates.versions.open(
      { who: null, running: oldest, state: local, said: same ? batch.allRun(targets.length, oldest) : batch.oldestRuns(targets.length, oldest) },
      { kind: "update", pick: (version) => updates.schedule(targets, version) },
    );
  }

  /** How those that may be set update; why each of the rest is left. */
  private ways(machines: Machine[]): void {
    const updates = this.options.updates;
    if (!updates) return;
    const view = updates.words;
    const able: MachineUpdate[] = [];
    const skipped: string[] = [];
    for (const machine of machines) {
      const update = updates.of(machine);
      if (update?.actions.settings.allowed) able.push(update);
      else skipped.push(`${machine.written}: ${view.reasons[update?.actions.settings.reason ?? "other"]}`);
    }
    if (!able.length) return;
    this.options.ways.open({ count: able.length, skipped, apply: (auto, relays) => updates.ways(able, auto, relays) });
  }

  private async cancel(machines: Machine[]): Promise<void> {
    const targets = this.known(machines, (update) => update.actions.cancel.allowed && !!update.due);
    if (targets.length) await this.options.updates?.schedule(targets, null);
  }

  /** Repairs those of `machines` the hub lets this login repair, or calls
   *  off the repairs kept for them. */
  private async repair(machines: Machine[], cancel: boolean): Promise<void> {
    const targets = this.known(machines, (update) => !!update.actions.repair?.allowed && (!cancel || !!update.repair_due));
    if (targets.length) await this.options.updates?.repair(targets, cancel);
  }

  private remove(machines: Machine[], world: WorldNow): void {
    const { manage, access } = this.options;
    if (!machines.length) return;
    const title = machines.length === 1 ? manage.removeTitle(machines[0]!.written) : manage.removeMany(machines.length);
    removeMachines(machines, world, title, {
      manage,
      access,
      sheet: this.options.removing,
      running: (name) => this.options.updates?.state.peek()?.network?.machines?.[name],
      change: this.options.change,
      log: this.options.log,
      done: () => {
        void this.options.refresh();
        this.options.clear();
      },
    });
  }
}

/** The most of `machines` one login of this device holds and may share:
 *  a visitor shares none, and no one the machine the hub runs on. */
function sharable(machines: Machine[], world: WorldNow): { login: Login; machines: Machine[] } | null {
  let best: { login: Login; machines: Machine[] } | null = null;
  for (const machine of machines) {
    const login = holderLoginOf(machine, world);
    if (!login || isVisitor(world, login) || (best && sameLogin(best.login, login))) continue;
    const mine = machines.filter((one) => {
      const holder = holderLoginOf(one, world);
      return !!holder && sameLogin(holder, login) && !runsHub(one, login, world);
    });
    if (!best || mine.length > best.machines.length) best = { login, machines: mine };
  }
  return best;
}

/** Those grouped together with the first that may be grouped: by the same
 *  login, for the same owner. */
function groupable(machines: Machine[], world: WorldNow): Machine[] {
  const first = machines.map((machine) => groupingOf(machine, world)).find(Boolean);
  if (!first) return [];
  return machines.filter((machine) => {
    const grouping = groupingOf(machine, world);
    return !!grouping && sameLogin(grouping.login, first.login) && grouping.owner === first.owner;
  });
}
