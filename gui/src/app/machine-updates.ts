// The machines page's update view (`world-gui.md`, 「更新」): each login of
// this device asks its hub which machines it may update, every few seconds
// while the view is shown; the machines in the answers are one list, each
// from the view that lets the most be done to it. Over them the ruler of
// versions and the network's policy; what is done to the machines ticked,
// scheduling a release, a repair or how each updates, goes to the hub whose
// view allowed it, and what did not go is said on the machine's row for a
// while.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { compareVersions, type MachineUpdate, type UpdateCall, type UpdatesView, type UpdatesViewCall, type UpdatesViewReply, type UpdateState } from "../world/updates";
import type { AccessWords } from "./account-words";
import type { Confirm } from "./confirm";
import type { UpdatesNow } from "./machine-list";
import { detailOf, known } from "./problem";
import { machinesOf } from "./reach";
import type { Signal } from "./signals";
import { RolloutWindow, type RolloutHost, type RolloutNow } from "./rollout-window";
import { rulerOf } from "./update-ruler";
import { SwitchChanges } from "./switch-change";
import { lineOf } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import type { RolloutPurpose, VersionWindow } from "./version-window";

/** The network's update policy, as this device last heard it, and the words
 *  of setting it; what this device knows of releases and of the network's
 *  machines, and asking other machines about their updates. */
export interface UpdatesAccess {
  words: UpdateWords;
  /** The update view's words. */
  view: UpdateViewWords;
  /** Asks `account`'s hub for its update view, or to schedule or set. */
  ask(hub: string, account: string, call: UpdatesViewCall): Promise<UpdatesViewReply>;
  /** The window of versions, picking the network's lowest one, or none. */
  minimum(current: string | null, pick: (version: string | null) => void): void;
  /** The window of versions, picking the release to roll out a batch at a
   *  time, where releases are withdrawn and taken back. */
  rollout(purpose: RolloutPurpose): void;
  state: Signal<UpdateState | null>;
  remote(machine: string, call: UpdateCall): Promise<UpdateState>;
  /** Has a machine of an earlier release update to the newest on its line. */
  older(machine: string): Promise<unknown>;
  versions: VersionWindow;
}

/** What the update view asks of its page. */
export interface UpdatesHost {
  updates: UpdatesAccess;
  access: AccessWords;
  world(): WorldNow | null;
  /** Whether the view is shown on a page still on the screen. */
  shown(): boolean;
  /** Draws the page again as the World now stands; only what changed
   *  changes. */
  draw(): void;
  /** What the page itself could not do, said over the view; and saying it,
   *  or letting it go. */
  failure(): string | null;
  failed(text: string | null): void;
  /** How wide the list the view is drawn in is. */
  width(): number;
  /** Asks before what cannot be taken lightly: withdrawing a release. */
  confirm: Confirm;
}

/** The first release that sets a machine's choices one by one
 *  (`UpdateCall::Preferences`); earlier ones take the whole of them. */
const PREFERENCES_SINCE = "1.0.1-beta.8";

/** The first release whose node takes an update kept for it past a change
 *  of World program of its own stuck on the way; an earlier one online is
 *  also asked at once, which it takes regardless. */
const ARRANGED_PAST_STUCK_SINCE = "1.0.1-beta.10";

/** How often the view asks while it is shown, and while the page is out of
 *  sight. */
const EVERY_MS = 2500;
const HIDDEN_MS = 15000;

/** How long what scheduling a machine said stays on its row. */
const SAID_MS = 15000;

/** How many machines a change of their ways is sent to at once. */
const WAYS_AT_ONCE = 6;

export class MachineUpdates {
  /** The update view as each login of this device was last told it, the
   *  machines in it as one, and which login's view each came from. */
  private views = new Map<string, UpdatesView>();
  private byName = new Map<string, MachineUpdate>();
  private loginOf = new Map<string, Login>();
  /** Each machine's name on its own hub, by how it is written here. */
  private bare = new Map<string, string>();
  /** The hub answers no update view: it runs an older release. */
  private old = false;
  /** Why the view could not be read, while none was; and which ask was the
   *  latest, so an earlier answer coming later is dropped. */
  private failure: string | null = null;
  private asked = 0;
  private timer = 0;
  private readonly switches = new SwitchChanges();
  private readonly writingWays = new Set<string>();
  private readonly writingPolicy = new Set<string>();
  private readonly acknowledgedWays = new Map<string, Pick<MachineUpdate, "reported" | "auto" | "auto_chosen" | "relays">>();
  /** What scheduling or setting a machine just said, by its name. */
  private readonly said = new Map<string, string>();
  /** Whether the network's policy is unfolded on a phone. */
  private policyOpen = false;
  /** The sheet of a release going out, and the login whose hub it is. */
  private readonly rollouts: RolloutWindow;
  private rolloutKey: string | null = null;

  constructor(private readonly host: UpdatesHost) {
    this.rollouts = new RolloutWindow(host.updates.view, host.updates.words.versions.close);
  }

  /** Keeps a pressed knob in the DOM until its slide finishes. */
  deferDraw(redraw: () => void): boolean {
    return this.switches.deferDraw(redraw);
  }

  /** What the hub says of a machine, as it is written here. */
  of(written: string): MachineUpdate | undefined {
    return this.byName.get(written);
  }

  /** How many machines may take a newer release. */
  able(): number {
    return [...this.byName.values()].filter((machine) => machine.offer && machine.actions.schedule.allowed).length;
  }

  /** Asks now and every few seconds while the view is shown; stops asking
   *  once it is not. */
  watch(): void {
    clearTimeout(this.timer);
    if (!this.host.shown()) return;
    void this.ask().finally(() => {
      if (!this.host.shown()) return;
      this.timer = window.setTimeout(() => this.watch(), document.hidden ? HIDDEN_MS : EVERY_MS);
    });
  }

  stop(): void {
    clearTimeout(this.timer);
  }

  /** Each login of this device asks its hub for its update view; the
   *  machines in them are one list, each from the view that lets the most
   *  be done to it. */
  private async ask(): Promise<void> {
    const { updates } = this.host;
    const world = this.host.world();
    if (!world) return;
    const logins = loginsOf(world.accounts);
    const asked = ++this.asked;
    const answers = await Promise.allSettled(logins.map((login) => updates.ask(login.hubName, login.account, { do: "view" })));
    if (asked !== this.asked) return;
    const views = new Map<string, UpdatesView>();
    const byName = new Map<string, MachineUpdate>();
    const from = new Map<string, Login>();
    const bare = new Map<string, string>();
    // A machine is known by how it is written here: two hubs' machines of
    // one name stay two, and each is asked by its own.
    const written = new Map<string, string>();
    const keyOf = (hub: string, name: string) => `${hub}\n${name}`;
    for (const machine of machinesOf(world)) {
      for (const on of machine.on) if (!written.has(keyOf(on.hub, machine.name))) written.set(keyOf(on.hub, machine.name), machine.written);
    }
    const writtenOf = (login: Login, name: string) => written.get(keyOf(login.hub, name)) ?? name;
    let old = false;
    let failed: string | null = null;
    answers.forEach((answer, index) => {
      const login = logins[index]!;
      const key = `${login.hub}\n${login.account}`;
      let view: UpdatesView | undefined;
      if (answer.status === "rejected") {
        const why = detailOf(answer.reason);
        // Only a hub before the view says so; anything else passes, and
        // what was read last stands meanwhile.
        if (/does not support the updates view/.test(why)) old = true;
        else {
          failed = why;
          view = this.views.get(key);
        }
      } else if (answer.value.kind === "view") view = answer.value.view;
      if (!view) return;
      views.set(key, view);
      for (const told of view.machines) {
        const written = writtenOf(login, told.machine);
        let machine = written === told.machine ? told : { ...told, machine: written };
        const saved = this.acknowledgedWays.get(written);
        // The hub may still hold the report from before this device saved
        // its settings. That same report cannot undo its acknowledgement.
        if (saved && told.reported === saved.reported) machine = { ...machine, ...saved };
        else if (saved) this.acknowledgedWays.delete(written);
        const kept = byName.get(written);
        const more = (one: MachineUpdate) => Number(one.actions.schedule.allowed) + Number(one.actions.settings.allowed) + Number(one.actions.update.allowed);
        if (!kept || more(machine) > more(kept)) {
          byName.set(written, machine);
          from.set(written, login);
          bare.set(written, told.machine);
        }
      }
    });
    this.views = views;
    this.byName = byName;
    this.loginOf = from;
    this.bare = bare;
    this.old = !views.size && old;
    this.failure = views.size || old ? null : failed;
    const rolling = this.rolloutKey === null ? null : this.rolloutOf(this.rolloutKey);
    if (rolling) this.rollouts.update(rolling);
    // Drawn again as any change is: what is the same as it was is left as
    // it is, every few seconds while the view is open.
    this.host.draw();
  }

  /** What the list draws in the update view: the machines that may be
   *  updated, the ruler and the network's policy over them. */
  now(narrow: boolean): UpdatesNow {
    const words = this.host.updates.view;
    const { newest, hub, line } = this.versionsNow();
    const policies = [...this.views.entries()].filter(([, view]) => view.policy);
    const policy = policies[0]?.[1].policy ?? null;
    const machines = [...this.byName.values()];
    let head = "";
    if (this.old) head = `<p class="uv-old">${escape(words.hubOld)}</p>`;
    else if (this.failure) head = `<p class="uv-old" title="${escape(this.failure)}">${escape(words.unread)}</p>`;
    // What the page itself was asked and could not do, said over it.
    const failure = this.host.failure();
    if (failure) head = `<p class="uv-old">${escape(failure)}</p>${head}`;
    else if (machines.length) {
      const width = Math.max(0, this.host.width() - (narrow ? 32 : 48));
      const rollout = policy?.rollout ?? null;
      head = rulerOf(
        {
          machines,
          newest,
          hub,
          minimum: policy?.minimum ?? null,
          rollout: rollout ? { version: rollout.version, share: rollout.share, paused: !!rollout.paused, stopped: !!rollout.stopped } : null,
          withdrawn: new Set(policy?.withdrawn ?? []),
          line,
          width: width || 900,
          phone: narrow,
        },
        words,
      );
      if (policies.length) {
        // Each hub's own, its name first where there are several.
        const logins = loginsOf(this.host.world()?.accounts);
        const rows = policies
          .map(([key, view]) => {
            const one = view.policy!;
            const minimum = one.minimum ?? null;
            const setting = this.switches.state(`policy:${key}`, one.managed_auto !== false);
            const managed = setting.on;
            const hubName = policies.length > 1 ? (logins.find((login) => `${login.hub}\n${login.account}` === key)?.hubName ?? "") : "";
            // The release going out a batch at a time, and how it stands.
            const going = one.rollout;
            const goingSaid = !going ? words.noRollout : going.stopped ? `${going.version} ${words.rolloutStopped}` : going.paused ? `${going.version} ${words.rolloutPaused}` : going.version;
            const rolling = view.rollouts
              ? `<span class="uv-policy-item"><span>${escape(words.rollout)}</span><button class="uv-chip${going?.stopped ? " caution" : ""}" type="button" data-uv-rollout="${escape(key)}"><span class="ml-num">${escape(goingSaid)}</span>${glyph("chevron", 12)}</button></span>`
              : "";
            return `<div class="uv-policy">${hubName ? `<span class="uv-policy-hub">${escape(hubName)}</span>` : ""}<span class="uv-policy-item"><span>${escape(words.minimum)}</span><button class="uv-chip" type="button" data-uv-minimum="${escape(key)}"><span class="ml-num">${escape(minimum ?? words.noMinimum)}</span>${glyph("chevron", 12)}</button></span>${rolling}<span class="uv-policy-item"><span>${escape(words.managedAuto)}</span><span class="uv-auto${managed ? " on" : ""}" role="switch" tabindex="0" aria-checked="${managed}" aria-busy="${setting.busy}" aria-disabled="${setting.busy}" aria-label="${escape(words.managedAuto)}" data-uv-managed="${escape(key)}"><span class="switch-track"><span class="switch-knob"></span></span></span></span></div>`;
          })
          .join("");
        // On a phone the policy is folded into one line until it is opened.
        head += narrow
          ? `<details class="uv-policy-fold"${this.policyOpen ? " open" : ""}><summary>${glyph("chevron", 14)}<span>${escape(words.policy)}</span></summary>${rows}</details>`
          : rows;
      }
    }
    return { byName: this.byName, line, words, head, said: this.said, autoState: (machine) => this.switches.state(`auto:${machine.machine}`, !!machine.auto) };
  }

  /** The update view's own presses in the list: a machine's switch, and the
   *  network's policy. Whether the press was one of them. */
  press(target: HTMLElement): boolean {
    const { updates } = this.host;
    if (!this.host.shown()) return false;
    const fold = target.closest<HTMLElement>(".uv-policy-fold > summary");
    if (fold) {
      this.policyOpen = !this.policyOpen;
      this.host.draw();
      return true;
    }
    const autoButton = target.closest<HTMLElement>("[data-uv-auto]");
    const auto = autoButton?.dataset.uvAuto;
    if (auto && autoButton) {
      const machine = this.byName.get(auto);
      if (machine && !this.switches.state(`auto:${auto}`, !!machine.auto).busy) {
        void this.switches.run(`auto:${auto}`, autoButton, (on) => this.setWay(machine, on, null)).then(() => {
          this.host.draw();
          return this.ask();
        });
      }
      return true;
    }
    const rolloutOf = target.closest<HTMLElement>("[data-uv-rollout]")?.dataset.uvRollout;
    if (rolloutOf !== undefined) {
      this.openRollout(rolloutOf);
      return true;
    }
    const minimumOf = target.closest<HTMLElement>("[data-uv-minimum]")?.dataset.uvMinimum;
    const managedOf = target.closest<HTMLElement>("[data-uv-managed]")?.dataset.uvManaged;
    const key = minimumOf ?? managedOf;
    const policy = key === undefined ? undefined : this.views.get(key)?.policy;
    if (minimumOf !== undefined && policy) {
      updates.minimum(policy.minimum ?? null, (version) => void this.setPolicy(minimumOf, version, policy.managed_auto !== false).then(() => this.ask()));
      return true;
    }
    if (managedOf !== undefined && policy) {
      const button = target.closest<HTMLElement>("[data-uv-managed]")!;
      if (!this.switches.state(`policy:${managedOf}`, policy.managed_auto !== false).busy) {
        void this.switches.run(`policy:${managedOf}`, button, (on) => this.setPolicy(managedOf, policy.minimum ?? null, on)).then(() => {
          this.host.draw();
          return this.ask();
        });
      }
      return true;
    }
    return false;
  }

  /** Schedules `version` for each of `machines` with the hub whose view
   *  let it, or takes back what was scheduled; what did not go is said on
   *  its row for a while. */
  async schedule(machines: MachineUpdate[], version: string | null): Promise<void> {
    const { updates } = this.host;
    const words = updates.view;
    await this.byHub(machines, (login, names) => updates.ask(login.hubName, login.account, { do: "schedule", machines: names, version }));
    if (version !== null) {
      const stuck = machines.filter((machine) => machine.online && machine.kernel && compareVersions(machine.kernel, ARRANGED_PAST_STUCK_SINCE) < 0);
      await inTurns(stuck, WAYS_AT_ONCE, async (machine) => {
        try {
          await updates.remote(machine.machine, { do: "kernel", version });
        } catch (error) {
          this.sayOnRow(machine.machine, plainly(error, words, words.notScheduled));
        }
      });
    }
    await this.ask();
  }

  /** Has each machine's hub keep a repair for it, made at once where it is
   *  online and once it is back where not; or calls off the repairs kept
   *  (`dev-notes/2026-10-22-更新的修复-设计.md`, 别的设备替它). */
  async repair(machines: MachineUpdate[], cancel: boolean): Promise<void> {
    const { updates } = this.host;
    await this.byHub(machines, (login, names) => updates.ask(login.hubName, login.account, cancel ? { do: "cancel_repair", machines: names } : { do: "repair", machines: names, version: null }));
    await this.ask();
  }

  /** Sends `call` for `machines` to each hub whose view let it, by their
   *  names there; what each machine's hub did not do is said on its row. */
  private async byHub(machines: MachineUpdate[], call: (login: Login, names: string[]) => Promise<UpdatesViewReply>): Promise<void> {
    const words = this.host.updates.view;
    const byLogin = new Map<string, { login: Login; names: string[] }>();
    for (const machine of machines) {
      const login = this.loginOf.get(machine.machine);
      if (!login) continue;
      const key = `${login.hub}\n${login.account}`;
      const entry = byLogin.get(key) ?? { login, names: [] };
      // The hub knows it by its name there.
      entry.names.push(this.bare.get(machine.machine) ?? machine.machine);
      byLogin.set(key, entry);
    }
    for (const { login, names } of byLogin.values()) {
      // Back from the hub's name to how it is written here.
      const written = (name: string) => [...this.bare.entries()].find(([as, one]) => one === name && this.loginOf.get(as) === login)?.[0] ?? name;
      try {
        const reply = await call(login, names);
        if (reply.kind !== "scheduled") continue;
        for (const result of reply.results) {
          if (result.done) this.said.delete(written(result.machine));
          else this.sayOnRow(written(result.machine), words.notScheduled(result.reason ? words.reasons[result.reason] : (result.why ?? words.reasons.other)));
        }
      } catch (error) {
        for (const name of names) this.sayOnRow(written(name), plainly(error, words, words.notScheduled));
      }
    }
  }

  /** Sets how each of `machines` updates, as far as it says: a release
   *  that sets them one by one is told only what changes, an earlier one
   *  the whole of them. */
  async setWays(machines: MachineUpdate[], auto: boolean | null, relays: boolean | null): Promise<void> {
    // A room of machines is asked a few at a time, not all at once.
    await inTurns(machines, WAYS_AT_ONCE, async (machine) => { await this.setWay(machine, auto, relays); });
    await this.ask();
  }

  private async setWay(machine: MachineUpdate, auto: boolean | null, relays: boolean | null): Promise<boolean> {
    if (this.writingWays.has(machine.machine)) return false;
    this.writingWays.add(machine.machine);
    const { updates } = this.host;
    const call: UpdateCall =
      machine.kernel && compareVersions(machine.kernel, PREFERENCES_SINCE) >= 0
        ? { do: "preferences", auto, relays }
        : { do: "set", settings: { auto: auto ?? !!machine.auto, chosen: auto !== null || machine.auto_chosen, relays: relays ?? machine.relays ?? true } };
    try {
      const saved = await updates.remote(machine.machine, call);
      this.asked += 1;
      const settings = { reported: machine.reported, auto: saved.auto_enabled ?? saved.auto ?? saved.settings.auto, auto_chosen: saved.settings.chosen ?? machine.auto_chosen, relays: saved.settings.relays };
      this.acknowledgedWays.set(machine.machine, settings);
      this.byName.set(machine.machine, { ...machine, ...settings });
      this.said.delete(machine.machine);
      return true;
    } catch (error) {
      this.sayOnRow(machine.machine, plainly(error, updates.view, (why) => why));
      return false;
    } finally {
      this.writingWays.delete(machine.machine);
    }
  }

  /** A network's update policy, set by the login `key` of this device that
   *  is its super administrator. */
  private async setPolicy(key: string, minimum: string | null, managedAuto: boolean): Promise<boolean> {
    const { updates } = this.host;
    const login = loginsOf(this.host.world()?.accounts).find((one) => `${one.hub}\n${one.account}` === key);
    if (!login || this.writingPolicy.has(key)) return false;
    this.writingPolicy.add(key);
    try {
      await updates.ask(login.hubName, login.account, { do: "policy", minimum, managed_auto: managedAuto });
      this.asked += 1;
      const view = this.views.get(key);
      if (view?.policy) {
        view.policy = { ...view.policy, managed_auto: managedAuto };
        if (minimum === null) delete view.policy.minimum;
        else view.policy.minimum = minimum;
      }
      this.host.failed(null);
      return true;
    } catch (error) {
      this.host.failed(this.host.access.failed(detailOf(error)));
      return false;
    } finally {
      this.writingPolicy.delete(key);
    }
  }

  /** The newest releases the views tell of, the hub's own, and the line
   *  they share. */
  private versionsNow(): { newest: string[]; hub: string | null; line: string | null } {
    const views = [...this.views.values()];
    const newest = views.flatMap((view) => [view.stable?.version, view.beta?.version].filter((version): version is string => !!version));
    const hub = views[0]?.hub ?? null;
    const top = [...newest].sort(compareVersions).pop() ?? hub;
    return { newest, hub, line: lineOf(top) };
  }

  /** What the rollout's sheet shows of the hub of the login `key`: its
   *  policy, the machines it names first, and the computers it updates; a
   *  phone takes its application's releases by itself. */
  private rolloutOf(key: string): RolloutNow | null {
    const view = this.views.get(key);
    if (!view?.policy || !view.rollouts) return null;
    const world = this.host.world();
    const forms = new Map(world ? machinesOf(world).map((machine) => [machine.name, "Ok" in machine.facts ? machine.facts.Ok?.form : undefined]) : []);
    return {
      policy: view.policy,
      first: view.rollout_first ?? [],
      machines: view.machines.filter((machine) => machine.kind !== "phone"),
      form: (name) => {
        const form = forms.get(name);
        return form === "laptop" ? "laptop" : form === "server" ? "server" : "desktop";
      },
      line: this.versionsNow().line,
    };
  }

  /** Opens the sheet of the release going out, or first the window of
   *  versions to pick one to roll out. */
  private openRollout(key: string): void {
    const now = this.rolloutOf(key);
    if (!now) return;
    this.rolloutKey = key;
    if (now.policy.rollout) {
      this.rollouts.open(now, this.rolloutHost(key));
      return;
    }
    this.pickRollout(key, (version) => {
      const fresh = this.rolloutOf(key);
      if (fresh) this.rollouts.open(fresh, this.rolloutHost(key), version);
    });
  }

  /** The window of versions on the release to roll out, where releases are
   *  withdrawn and taken back too. */
  private pickRollout(key: string, pick: (version: string) => void): void {
    this.host.updates.rollout({
      kind: "rollout",
      rolling: this.views.get(key)?.policy?.rollout?.version ?? null,
      withdrawn: () => new Set(this.views.get(key)?.policy?.withdrawn ?? []),
      pick: async (version) => pick(version),
      withdraw: (version, withdrawn, done) => this.withdraw(key, version, withdrawn, done),
    });
  }

  private rolloutHost(key: string): RolloutHost {
    return {
      set: (version, first, share) => this.call(key, { do: "rollout", version, first, share }),
      pause: (paused) => this.call(key, { do: "pause", paused }),
      another: (_current, pick) => this.pickRollout(key, pick),
      failed: (error) => {
        const detail = detailOf(error);
        return { said: this.host.access.failed(detail), why: detail };
      },
    };
  }

  /** Asks the hub of the login `key` to change how releases go out, then
   *  reads the view again. */
  private async call(key: string, call: UpdatesViewCall): Promise<void> {
    const login = loginsOf(this.host.world()?.accounts).find((one) => `${one.hub}\n${one.account}` === key);
    if (!login) throw new Error("the login is no longer on this device");
    await this.host.updates.ask(login.hubName, login.account, call);
    await this.ask();
  }

  /** Withdraws `version`, asked first, or takes it back at once. */
  private withdraw(key: string, version: string, withdrawn: boolean, done: () => void): void {
    const words = this.host.updates.words.versions;
    if (!withdrawn) {
      this.call(key, { do: "withdraw", version, withdrawn: false }).then(done, (error: unknown) => {
        this.host.failed(this.host.access.failed(detailOf(error)));
        this.host.draw();
      });
      return;
    }
    this.host.confirm.ask({
      title: words.withdrawAsk(version),
      what: words.withdrawWhat,
      go: words.withdraw,
      act: () => this.call(key, { do: "withdraw", version, withdrawn: true }),
      done,
    });
  }

  /** Says `text` on a machine's row in the update view for a while. */
  private sayOnRow(name: string, text: string): void {
    this.said.set(name, text);
    window.setTimeout(() => {
      if (this.said.get(name) !== text) return;
      this.said.delete(name);
      this.host.draw();
    }, SAID_MS);
  }
}

/** `error` as a machine's row says it: in plain words, the kernel's own on
 *  a line of their own, for the pointer to rest on. */
function plainly(error: unknown, words: UpdateViewWords, say: (why: string) => string): string {
  const detail = detailOf(error);
  return `${say(known(detail, words.problems) ?? words.reasons.other)}\n${detail}`;
}

/** Runs `each` over `items`, at most `width` at a time. */
async function inTurns<T>(items: T[], width: number, each: (item: T) => Promise<void>): Promise<void> {
  let next = 0;
  const lane = async (): Promise<void> => {
    while (next < items.length) await each(items[next++]!);
  };
  await Promise.all(Array.from({ length: Math.min(width, items.length) }, lane));
}
