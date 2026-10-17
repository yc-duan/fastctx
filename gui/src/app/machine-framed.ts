// Machines chosen together, on the drawing or the list (`world-gui.md`,
// 账号、机器、共享与协助): a row each, with the version it runs and whether a
// newer one is out, and what can be done to them together: sharing them
// with the same person, grouping them, updating them, how they update, and
// taking them off. An action that does not reach all of them says how many
// it reaches; a row nothing can be done to stands back and says why when
// pointed at. An update of several shows its plan in the rows, then how
// each goes (`machine-batch.ts`).

import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { AccountChange, Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { osName } from "../world/names";
import { compareVersions, newestKnown, type UpdateCall, type UpdateState } from "../world/updates";
import type { AccessWords, MachinesWords } from "./account-words";
import { planOf, UpdateBatch, type BatchRow } from "./machine-batch";
import type { BatchWords } from "./machine-batch-words";
import { groupingOf } from "./machine-groups";
import type { ReadLog } from "./machine-log";
import { ownerLoginOf, removerLoginOf, updaterLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import type { RemoveMachineSheet } from "./machine-remove";
import { removeMachines } from "./machine-removing";
import type { WaysSheet } from "./machine-ways";
import { isVisitor, sameLogin, type Machine } from "./reach";
import type { Signal } from "./signals";
import type { VersionWindow } from "./version-window";

/** The first release that answers about its updates (`RemoteCall::Updates`). */
const ASKED_SINCE = "1.0.1-beta.3";

/** One thing done to the machines chosen: its label, saying how many it
 *  reaches when not all of them; its word on a phone's bar, where how many
 *  is a small figure on its mark. */
interface Verb {
  act: string;
  label: string;
  word: string;
  reach: number | null;
  icon: GlyphName;
  danger?: boolean;
}

export interface FramedOptions {
  words: MachinesWords;
  batch: BatchWords;
  manage: ManageWords;
  access: AccessWords;
  /** What this device knows of releases and of the network's machines, and
   *  asking other machines about their updates. */
  updates?:
    | {
        state: Signal<UpdateState | null>;
        remote(machine: string, call: UpdateCall): Promise<UpdateState>;
        older(machine: string): Promise<unknown>;
        versions: VersionWindow;
      }
    | undefined;
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
  /** Lets the machines chosen go, once they were taken off. */
  clear(): void;
  changed(): void;
}

export class FramedPanel {
  /** An update of several, planned or under way, until it is closed. */
  private batch: UpdateBatch | null = null;
  /** What each machine said to setting how it updates, by name. */
  private readonly said = new Map<string, { text: string; wrong: boolean }>();

  constructor(private readonly options: FramedOptions) {}

  dispose(): void {
    this.batch?.dispose();
  }

  /** The machines chosen changed: a plan not started and what setting how
   *  they update said are let go; an update under way goes on. */
  forget(): void {
    this.said.clear();
    if (this.batch && !this.batch.started) {
      this.batch.dispose();
      this.batch = null;
    }
  }

  /** Whether an update of several is planned or under way: its rows and
   *  its start are the panel itself. */
  get planning(): boolean {
    return this.batch !== null;
  }

  html(machines: Machine[], world: WorldNow): string {
    const { words, batch } = this.options;
    const local = this.options.updates?.state.peek() ?? null;
    const plan = this.batch;
    const title = plan ? `${words.framedTitle(machines.length)} ${batch.planTitle(plan.version)}` : words.framedTitle(machines.length);
    const acts = this.acts(machines, world, local);
    return `<header class="panel-head framed-head"><h2 class="panel-title">${escape(title)}</h2></header><button class="icon-button panel-close" type="button" data-act="framed-clear" aria-label="${escape(batch.clear)}" title="${escape(batch.clear)}">${glyph("close", 16)}</button>
      <ul class="framed-list">${machines.map((machine) => this.row(machine, world, local)).join("")}</ul>
      ${acts ? `<div class="framed-acts">${acts}</div>` : ""}`;
  }

  /** What is done to them together as a bar of equal columns, a mark over
   *  each word, for the bottom of a phone while several are chosen there. */
  bar(machines: Machine[], world: WorldNow): string {
    const local = this.options.updates?.state.peek() ?? null;
    const verbs = this.verbs(machines, world, local);
    return verbs.length
      ? `<div class="sheet-acts framed-bar" style="--acts:${verbs.length}">${verbs
          .map(
            (verb) =>
              `<button class="sheet-act${verb.danger ? " danger" : ""}" type="button" data-act="${verb.act}"${verb.reach === null ? "" : ` title="${escape(verb.label)}"`}><span class="framed-mark">${glyph(verb.icon, 18)}${verb.reach === null ? "" : `<span class="framed-reach">${verb.reach}</span>`}</span><span>${escape(verb.word)}</span></button>`,
          )
          .join("")}</div>`
      : "";
  }

  /** A machine's row: its name and system, then what it runs and whether
   *  a newer one is out, or what an update of several makes of it. */
  private row(machine: Machine, world: WorldNow, local: UpdateState | null): string {
    const { batch: words } = this.options;
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const running = runningOf(machine, local);
    const planned = this.batch?.rows.find((row) => row.machine.name === machine.name);
    const set = this.said.get(machine.name);
    let tail = "";
    let kind = "";
    if (planned) ({ text: tail, kind } = this.going(planned));
    else if (set) ({ text: tail, kind } = { text: set.text, kind: set.wrong ? " wrong" : "" });
    else if (running) {
      const newer = newestKnown(local, running);
      tail = `${running} ${machine.online === false ? words.away : newer ? words.newer : words.latest}`;
      if (newer && machine.online !== false) kind = " newer";
    } else if (machine.online === false) tail = words.away;
    const manageable = machine.here || !!(updaterLoginOf(machine, world) || removerLoginOf(machine, world) || ownerLoginOf(machine, world) || groupingOf(machine, world));
    const state = `${machine.online === false ? " offline" : ""}${manageable ? "" : " faint"}${kind}`;
    return `<li><button class="framed-row${state}" type="button" data-act="framed-pick" data-key="m:${escape(machine.written)}"${manageable ? "" : ` title="${escape(words.notYours)}"`}><span class="framed-icon" aria-hidden="true">${glyph(facts?.form === "laptop" ? "laptop" : facts?.form === "server" ? "server" : "desktop", 15)}</span><span class="framed-name">${escape(machine.written)}</span>${planned ? "" : `<span class="framed-sub">${escape(facts ? osName(facts.os) : "")}</span>`}<span class="framed-tail"${tail ? ` title="${escape(tail)}"` : ""}>${escape(tail)}</span></button></li>`;
  }

  /** What an update of several makes of a machine, planned or as it goes. */
  private going(row: BatchRow): { text: string; kind: string } {
    const { batch: words, manage } = this.options;
    const version = this.batch!.version;
    const going = row.going;
    if (!going) {
      if (row.fate.kind === "skip") return { text: row.fate.why, kind: " skip" };
      return { text: row.fate.kind === "due" ? words.willDue : words.willUpdate, kind: "" };
    }
    switch (going.kind) {
      case "queued":
        return { text: words.queued, kind: "" };
      case "asking":
      case "older":
        return { text: words.asking, kind: "" };
      case "state": {
        const { fetching, renewing } = going.state;
        if (fetching) {
          const percent = fetching.total ? Math.min(100, Math.floor((fetching.received / fetching.total) * 100)) : null;
          return { text: words.fetching(percent, `${(fetching.received / 1_000_000).toFixed(1)} MB`, fetching.via === "network"), kind: "" };
        }
        if (renewing) return { text: manage.phase[renewing.phase], kind: "" };
        return { text: words.asking, kind: "" };
      }
      case "done":
        return { text: words.updated(version), kind: " done" };
      case "kept":
        return { text: words.dueKept, kind: " done" };
      case "failed":
        return { text: manage.failed(going.why), kind: " wrong" };
    }
  }

  /** What is done to them together; an update of several, its start, or
   *  once through, how it went. Letting them go is the head's close. */
  private acts(machines: Machine[], world: WorldNow, local: UpdateState | null): string {
    const { batch: words } = this.options;
    const button = (act: string, label: string, extra = "") => `<button class="text-button quiet-button" type="button" data-act="${act}"${extra}>${escape(label)}</button>`;
    const plan = this.batch;
    if (plan && !plan.started) {
      return `<button class="sheet-go" type="button" data-act="framed-start"${plan.reaching ? "" : " disabled"}>${escape(words.start(plan.reaching))}</button>${button("framed-cancel", words.cancel)}`;
    }
    if (plan?.finished) return `<span class="framed-said">${escape(words.finished(plan.done, plan.failed))}</span>${button("framed-close", words.close)}`;
    if (plan) return "";
    return this.verbs(machines, world, local)
      .map((verb) => button(verb.act, verb.label, verb.danger ? " data-danger" : ""))
      .join("");
  }

  /** What may be done to them together, each saying how many it reaches
   *  when not all of them. */
  private verbs(machines: Machine[], world: WorldNow, local: UpdateState | null): Verb[] {
    const { batch: words } = this.options;
    const verb = (act: string, label: string, word: string, reach: number, icon: GlyphName, danger = false): Verb => {
      const some = reach < machines.length;
      return { act, label: some ? words.some(label, reach) : label, word, reach: some ? reach : null, icon, danger };
    };
    const share = sharable(machines, world);
    const group = groupable(machines, world);
    const update = this.options.updates ? updatable(machines, world) : [];
    const ways = update.filter((machine) => machine.online !== false && !olderOf(machine, local));
    const remove = machines.filter((machine) => removerLoginOf(machine, world));
    const verbs: Verb[] = [];
    if (share) verbs.push(verb("framed-share", words.share, words.bar.share, share.machines.length, "colleague"));
    if (group.length) verbs.push(verb("framed-group", words.group, words.bar.group, group.length, "folder"));
    if (update.length) verbs.push(verb("framed-update", words.update, words.bar.update, update.length, "update"));
    if (ways.length) verbs.push(verb("framed-ways", words.ways, words.bar.ways, ways.length, "settings"));
    if (remove.length) verbs.push(verb("framed-remove", words.remove, words.bar.remove, remove.length, "close", true));
    return verbs;
  }

  /** The panel's own buttons; whether the press was one of them. */
  click(act: string, machines: Machine[], world: WorldNow): boolean {
    const local = this.options.updates?.state.peek() ?? null;
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
        this.update(updatable(machines, world), world);
        return true;
      case "framed-ways":
        this.ways(machines, world, local);
        return true;
      case "framed-remove":
        this.remove(machines.filter((machine) => removerLoginOf(machine, world)), world);
        return true;
      case "framed-start":
        this.batch?.start();
        return true;
      case "framed-cancel":
      case "framed-close":
        this.batch?.dispose();
        this.batch = null;
        this.options.changed();
        return true;
    }
    return false;
  }

  /** The window of versions for `machines`, from the oldest of what they
   *  run; the version picked makes the plan. */
  private update(machines: Machine[], world: WorldNow): void {
    const updates = this.options.updates;
    const local = updates?.state.peek();
    if (!updates || !local || !machines.length) return;
    const { batch: words } = this.options;
    const runs = machines.flatMap((machine) => runningOf(machine, local) ?? []).sort(compareVersions);
    const oldest = runs[0];
    if (!oldest) return;
    const same = runs.every((version) => version === oldest);
    updates.versions.open(
      { who: null, running: oldest, state: local, said: same ? words.allRun(machines.length, oldest) : words.oldestRuns(machines.length, oldest) },
      {
        kind: "update",
        pick: async (version) => {
          this.batch?.dispose();
          this.batch = new UpdateBatch(version, planOf(machines, world, version, updates.state.peek(), words), {
            remote: updates.remote,
            older: updates.older,
            due: async (login, machine, to, batch) => {
              await this.options.change(login, { change: "update_due", machine, ...(to ? { version: to } : {}), ...(batch ? { batch } : {}) });
              await this.options.refresh();
            },
            heard: (machine) => updates.state.peek()?.network?.machines?.[machine]?.kernel ?? null,
            changed: () => this.options.changed(),
          });
          this.options.changed();
        },
      },
    );
  }

  /** How those that can be set now update; why each of the rest is left. */
  private ways(machines: Machine[], world: WorldNow, local: UpdateState | null): void {
    const updates = this.options.updates;
    if (!updates) return;
    const { batch: words, manage } = this.options;
    const able: Machine[] = [];
    const skipped: string[] = [];
    for (const machine of machines) {
      const why = machine.here
        ? words.skipHere
        : !updaterLoginOf(machine, world)
          ? words.skipNotYours
          : machine.online === false
            ? words.skipAway
            : olderOf(machine, local)
              ? words.skipOlderWays
              : null;
      if (why) skipped.push(`${machine.written}: ${why}`);
      else able.push(machine);
    }
    if (!able.length) return;
    this.options.ways.open({
      count: able.length,
      skipped,
      apply: async (auto, relays) => {
        await Promise.all(
          able.map(async (machine) => {
            try {
              const state = await updates.remote(machine.name, { do: "state" });
              const settings = { ...state.settings, ...(auto === null ? {} : { auto, chosen: true }), ...(relays === null ? {} : { relays }) };
              await updates.remote(machine.name, { do: "set", settings });
              this.said.set(machine.name, { text: words.set, wrong: false });
            } catch (error) {
              this.said.set(machine.name, { text: manage.failed(error instanceof Error ? error.message : String(error)), wrong: true });
            }
            this.options.changed();
          }),
        );
      },
    });
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

/** What a machine runs, as the network last heard or it said of itself. */
function runningOf(machine: Machine, local: UpdateState | null): string | null {
  const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
  return local?.network?.machines?.[machine.name]?.kernel ?? facts?.version ?? null;
}

/** Whether a machine runs a release before it answers about its updates. */
function olderOf(machine: Machine, local: UpdateState | null): boolean {
  const running = runningOf(machine, local);
  return !!running && compareVersions(running, ASKED_SINCE) < 0;
}

/** Those that may be updated from here. */
function updatable(machines: Machine[], world: WorldNow): Machine[] {
  return machines.filter((machine) => updaterLoginOf(machine, world));
}

/** The most of `machines` one login of this device owns and may share:
 *  a visitor shares none of its machines. */
function sharable(machines: Machine[], world: WorldNow): { login: Login; machines: Machine[] } | null {
  let best: { login: Login; machines: Machine[] } | null = null;
  for (const machine of machines) {
    const login = ownerLoginOf(machine, world);
    if (!login || isVisitor(world, login) || (best && sameLogin(best.login, login))) continue;
    const mine = machines.filter((one) => {
      const owner = ownerLoginOf(one, world);
      return !!owner && sameLogin(owner, login);
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
