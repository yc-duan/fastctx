// The Fastctx on a machine, at the top of its page's 「管理」 (`world-gui.md`,
// 机器的详情; `world-upgrade.md`, 别的机器的更新): what it runs, and the
// application's version where one carries it; whether a boot service keeps
// it; what it can take and how an update of it goes, as the machine itself
// says, asked every few seconds while it is shown. Updating it and its
// choices are the machines' update view's, which 「更新…」 opens with it
// ticked; this device's own opens its update card. A machine whose Fastctx
// is older updates to the newest on its line alone, from here.

import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import { compareVersions, newestKnown, type UpdateCall, type UpdateState } from "../world/updates";
import { link } from "./places";
import { detailOf } from "./problem";
import { lineOf, stageMark } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";
import type { ManageWords } from "./machine-manage-words";
import type { Machine } from "./reach";
import type { Signal } from "./signals";

/** How often a machine is asked how its updates stand, and while a package
 *  comes or it changes its World program. */
const EVERY_MS = 5_000;
const MOVING_MS = 1_000;

export interface FastctxOptions {
  words: ManageWords;
  /** How an update under way or stopped is said, as the machines' update
   *  view says it. */
  view: UpdateViewWords;
  /** How updates stand on this device: what it knows of releases, and of
   *  the network's machines. */
  local: Signal<UpdateState | null>;
  /** Asks another machine about its updates. */
  remote(machine: string, call: UpdateCall): Promise<UpdateState>;
  /** Has a machine of an earlier release update to the newest on its line. */
  older(machine: string): Promise<UpdateState>;
  /** This device's own update card. */
  here(): void;
  /** How it is kept running: in a boot service, or with its user's login. */
  kept(service: boolean): string;
  /** Something shown changed: the page draws it again. */
  changed(): void;
}

/** The machine the block is of, as the page knows it. */
export interface FastctxSubject {
  machine: Machine;
  /** The World program it runs, as it last said. */
  version: string | null;
  /** Whether a boot service keeps it; unknown before it said. */
  service: boolean | null;
  /** The login that may update it and set its choices; none, to look only. */
  manages: Login | null;
}

/** The first release that answers about its updates (`RemoteCall::Updates`). */
const ASKED_SINCE = "1.0.1-beta.3";
/** What a machine's own terminal runs to hand its boot service the new
 *  program, asking for an administrator once. */

export class FastctxBlock {
  private name: string | null = null;
  private state: UpdateState | null = null;
  private problem: string | null = null;
  private said: string | null = null;
  /** What the kernel said of the last thing done, for the pointer. */
  private saidWhy = "";
  private timer = 0;
  private asking = false;
  /** The machine was away when last asked: it is asked as soon as it is back. */
  private away = false;
  private shown = false;
  private subject: FastctxSubject | null = null;

  constructor(private readonly options: FastctxOptions) {}

  /** Whether the block is in view: it asks only then. */
  show(subject: FastctxSubject | null, shown: boolean): void {
    const name = subject?.machine.name ?? null;
    if (name !== this.name) {
      this.name = name;
      this.state = null;
      this.problem = null;
      this.said = null;
    this.saidWhy = "";
      this.saidWhy = "";
    }
    const was = this.shown;
    this.shown = shown;
    // Shown again, or back online after it was not asked while away.
    if (shown && (!was || (this.away && subject?.machine.online !== false))) void this.ask(subject);
    if (!shown) clearTimeout(this.timer);
  }

  leave(): void {
    this.shown = false;
    clearTimeout(this.timer);
  }

  private async ask(subject: FastctxSubject | null = this.subject): Promise<void> {
    clearTimeout(this.timer);
    this.subject = subject;
    if (!subject || !this.shown || this.asking) return;
    const { machine, manages } = subject;
    const older = this.older(subject);
    this.away = machine.online === false;
    if (machine.here || !manages || this.away || older) return;
    this.asking = true;
    try {
      this.state = await this.options.remote(machine.written, { do: "state" });
      this.problem = null;
    } catch (error) {
      this.problem = detailOf(error);
    } finally {
      this.asking = false;
    }
    if (subject.machine.name !== this.name) return;
    this.options.changed();
    const moving = !!this.state?.fetching || (!!this.state?.renewing && this.state.renewing.phase !== "failed");
    if (this.shown) this.timer = window.setTimeout(() => void this.ask(), moving ? MOVING_MS : EVERY_MS);
  }

  /** Whether the machine's Fastctx is of a release before it answers about
   *  its updates, as the network last heard. */
  private older(subject: FastctxSubject): boolean {
    const running = this.options.local.peek()?.network?.machines?.[subject.machine.name]?.kernel ?? subject.version;
    return !!running && compareVersions(running, ASKED_SINCE) < 0;
  }

  /** The block's markup. */
  html(subject: FastctxSubject): string {
    this.subject = subject;
    const { words } = this.options;
    const { machine } = subject;
    const local = this.options.local.peek();
    const state = machine.here ? local : this.state;
    const heard = local?.network?.machines?.[machine.name];
    const running = state?.running ?? subject.version ?? heard?.kernel ?? null;
    const app = machine.here ? local?.app?.version : (state?.app?.version ?? heard?.app);
    const facts = [
      running ? escape(running) : "",
      app ? escape(app === running ? words.withApp : words.app(app)) : "",
      subject.service === null ? "" : escape(this.options.kept(subject.service)),
    ].filter(Boolean);
    const rows = [`<dt>${escape(words.fastctx)}</dt><dd class="fx-facts"><span>${facts.join(`<span class="sep"></span>`)}</span></dd>`];
    const status = this.status(subject, state, running);
    if (status) rows.push(`<dt></dt><dd class="fx-status">${status}</dd>`);
    return `<dl class="mp-rows">${rows.join("")}</dl>`;
  }

  /** What it can take now and how an update of it goes, then the way to
   *  update it: this device's card, or the update view with it ticked. */
  private status(subject: FastctxSubject, state: UpdateState | null, running: string | null): string {
    const { words } = this.options;
    const { machine, manages } = subject;
    const bar = (percent: number | null) =>
      `<span class="fx-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100"${percent === null ? "" : ` aria-valuenow="${percent}"`}><span style="width:${percent ?? 6}%"></span></span>`;
    const wrong = (said: string, why = "") => `<span class="fx-wrong"${why ? ` title="${escape(why)}"` : ""}>${escape(said)}</span>`;
    // A node that says how its update stands is said in the update view's words.
    const staged = state?.stage ? stageMark(state.stage, lineOf(running), this.options.view) : null;
    if (machine.here) {
      const go = `<button class="text-button quiet-button" type="button" data-act="fx-here">${escape(words.update)}</button>`;
      if (staged) return `${staged.kind === "failed" ? wrong(staged.said, staged.why) : `<span>${escape(staged.said)}</span>`}${go}`;
      const offer = state?.offer && running && compareVersions(state.offer.version, running) > 0 ? state.offer.version : null;
      return `<span>${escape(offer ? words.canTake(offer) : words.upToDate)}</span>${go}`;
    }
    const local = this.options.local.peek();
    const newest = this.newest(local, running);
    if (this.said) return `<span class="fx-said"${this.saidWhy ? ` title="${escape(this.saidWhy)}"` : ""}>${escape(this.said)}</span>`;
    if (this.older(subject)) {
      if (!newest) return `<span>${escape(words.upToDate)}</span>`;
      // A release before 1.0.1 beta 3 changes only its World program from
      // elsewhere; an application's computer updates at it.
      if (local?.network?.machines?.[machine.name]?.app) return `<span>${escape(words.olderApp(newest))}</span>`;
      return `<span>${escape(words.canTake(newest))}</span>${manages ? `<button class="text-button quiet-button" type="button" data-act="fx-older" title="${escape(words.older)}">${escape(words.olderGo)}</button>` : ""}`;
    }
    const go = manages ? `<a class="text-button quiet-button" href="${link({ kind: "machines", focus: machine.written, updates: true })}">${escape(words.update)}</a>` : "";
    if (machine.online === false) {
      const due = local?.network?.due?.[machine.name];
      return `<span class="fx-quiet">${escape(due ? words.due(due.version, due.by) : words.away)}</span>${go}`;
    }
    if (!manages) return newest ? `<span>${escape(words.canTake(newest))}</span>` : `<span>${escape(words.upToDate)}</span>`;
    if (!state) return `${this.problem ? wrong(words.unread(this.problem), this.problem) : `<span class="fx-quiet">${escape(words.asking)}</span>`}${go}`;
    if (staged && state.stage) {
      const stage = state.stage;
      const said = staged.kind === "failed" ? wrong(staged.said, staged.why) : `<span>${escape(staged.said)}</span>`;
      if (stage.kind === "fetching") return `${said}${bar(stage.total ? Math.min(100, Math.floor((stage.received / stage.total) * 100)) : null)}`;
      if (stage.kind === "administrator") return said;
      return `${said}${go}`;
    }
    const megabytes = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;
    if (state.fetching) {
      const { received, total, via, version } = state.fetching;
      return `<span>${escape(version)} ${escape(words.fetching(megabytes(received), total ? megabytes(total) : null, via === "network"))}</span>${bar(total ? Math.min(100, Math.floor((received / total) * 100)) : null)}`;
    }
    if (state.renewing) {
      const { version, phase, error } = state.renewing;
      const said = `${version} ${phase === "failed" ? (error ? words.failed(error) : words.phase.failed) : words.phase[phase]}`;
      return phase === "failed" ? `${wrong(said, error ?? "")}${go}` : `<span>${escape(said)}</span>`;
    }
    if (state.failed) return `${wrong(words.failed(state.failed.why), state.failed.why)}${go}`;
    if (state.take && state.app) {
      const said = state.open ? words.appTakes(state.take) : state.app.target.startsWith("windows") ? words.appClosed(state.take) : words.appOpenThere(state.take);
      return `<span>${escape(said)}</span>`;
    }
    const offer = state.offer && running && compareVersions(state.offer.version, running) > 0 ? state.offer.version : null;
    return `<span>${escape(offer ? words.canTake(offer) : words.upToDate)}</span>${go}`;
  }

  /** The newest release this device knows on the line of `running`. */
  private newest(local: UpdateState | null, running: string | null): string | null {
    return running ? newestKnown(local, running) : null;
  }

  /** The block's own buttons; whether the press was one of them. */
  click(target: HTMLElement): boolean {
    const subject = this.subject;
    const act = target.dataset.act;
    if (!subject || !act?.startsWith("fx-")) return false;
    switch (act) {
      case "fx-here":
        this.options.here();
        return true;
      case "fx-older":
        this.act(this.options.older(subject.machine.written));
        return true;
    }
    return false;
  }

  /** Does `doing`, then shows how it stands. */
  private act(doing: Promise<unknown>): void {
    this.said = null;
    doing.then(
      (result) => {
        if (result && typeof result === "object" && "settings" in result) this.state = result as UpdateState;
        this.options.changed();
        void this.ask();
      },
      (error: unknown) => {
        this.saidWhy = detailOf(error);
        this.said = this.options.words.failed(this.saidWhy);
        this.options.changed();
      },
    );
  }
}
