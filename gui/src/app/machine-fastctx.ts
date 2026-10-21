// The Fastctx on a machine, at the top of its page's 「管理」 (`world-gui.md`,
// 机器的详情; `world-upgrade.md`, 别的机器的更新): what it runs, and the
// application's version where one carries it; whether a boot service keeps
// it; what it can take, an update picked in the window of versions, and how
// that update goes, as the machine itself says, asked every few seconds
// while it is shown; and its two choices. A machine away takes an update
// the hub keeps for it until it is back. A machine whose Fastctx is older
// updates to the newest on its line alone. This device's own opens the
// window its settings open.

import { escape } from "../conversation/prose";
import { copyButton, copyFrom } from "./copy-button";
import type { Login } from "../world/accounts";
import { compareVersions, newestKnown, type UpdateCall, type UpdateSettings, type UpdateState } from "../world/updates";
import { lineOf, stageMark } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";
import type { ManageWords } from "./machine-manage-words";
import type { Machine } from "./reach";
import type { Signal } from "./signals";
import type { VersionWindow } from "./version-window";

/** How often a machine is asked how its updates stand, and while a package
 *  comes or it changes its World program. */
const EVERY_MS = 5_000;
const MOVING_MS = 1_000;

export interface FastctxOptions {
  words: ManageWords;
  /** How an update under way or stopped is said, as the machines' update
   *  view says it. */
  view: UpdateViewWords;
  versions: VersionWindow;
  /** How updates stand on this device: what it knows of releases, and of
   *  the network's machines. */
  local: Signal<UpdateState | null>;
  /** Asks another machine about its updates. */
  remote(machine: string, call: UpdateCall): Promise<UpdateState>;
  /** Has a machine of an earlier release update to the newest on its line. */
  older(machine: string): Promise<UpdateState>;
  /** This device's own window of versions. */
  versionsHere(look: boolean): void;
  /** Keeps an update for a machine away, as `login`, or takes it back. */
  due(login: Login, machine: string, version: string | null): Promise<void>;
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
  /** Its hub keeps an update for it while it is away: one that reads such
   *  records, which a machine takes from 1.0.1 beta 3 on. */
  keeps: boolean;
}

/** The first release that answers about its updates (`RemoteCall::Updates`). */
const ASKED_SINCE = "1.0.1-beta.3";
/** What a machine's own terminal runs to hand its boot service the new
 *  program, asking for an administrator once. */
const ADMIN_LINE = "fastctx renew guardian";

export class FastctxBlock {
  private name: string | null = null;
  private state: UpdateState | null = null;
  private problem: string | null = null;
  private said: string | null = null;
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
      this.problem = error instanceof Error ? error.message : String(error);
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
    if (!machine.here && subject.manages && state && !this.older(subject) && machine.online !== false) {
      rows.push(...this.choices(state));
    }
    return `<dl class="mp-rows">${rows.join("")}</dl>`;
  }

  /** What it can take now and how an update of it goes. */
  private status(subject: FastctxSubject, state: UpdateState | null, running: string | null): string {
    const { words } = this.options;
    const { machine, manages } = subject;
    const button = (act: string, label: string, title = "") =>
      `<button class="text-button quiet-button" type="button" data-act="${act}"${title ? ` title="${escape(title)}"` : ""}>${escape(label)}</button>`;
    // A node that says how its update stands is said in the update view's words.
    const staged = state?.stage ? stageMark(state.stage, lineOf(running), this.options.view) : null;
    if (machine.here) {
      if (staged) return `<span${staged.kind === "failed" ? ` class="fx-wrong"` : ""}>${escape(staged.said)}</span>${button("fx-versions-here", words.update)}`;
      const offer = state?.offer && running && compareVersions(state.offer.version, running) > 0 ? state.offer.version : null;
      return `<span>${escape(offer ? words.canTake(offer) : words.upToDate)}</span>${button("fx-versions-here", words.update)}`;
    }
    const local = this.options.local.peek();
    const due = local?.network?.due?.[machine.name];
    const newest = this.newest(local, running);
    if (this.said) return `<span class="fx-said">${escape(this.said)}</span>`;
    if (machine.online === false) {
      if (due) return `<span>${escape(words.due(due.version, due.by))}</span>${manages ? button("fx-undue", words.undue) : ""}`;
      if (!manages) return "";
      return subject.keeps && !this.older(subject)
        ? `<span class="fx-quiet">${escape(words.offline)}</span>${button("fx-versions", words.update)}`
        : `<span class="fx-quiet">${escape(words.away)}</span>`;
    }
    if (this.older(subject)) {
      if (!newest) return `<span>${escape(words.upToDate)}</span>`;
      // A release before 1.0.1 beta 3 changes only its World program from
      // elsewhere; an application's computer updates at it.
      if (local?.network?.machines?.[machine.name]?.app) return `<span>${escape(words.olderApp(newest))}</span>`;
      return `<span>${escape(words.canTake(newest))}</span>${manages ? button("fx-older", words.olderGo, words.older) : ""}`;
    }
    if (!manages) return newest ? `<span>${escape(words.canTake(newest))}</span>` : `<span>${escape(words.upToDate)}</span>`;
    if (!state) return `<span class="fx-quiet">${escape(this.problem ?? words.asking)}</span>`;
    if (staged && state.stage) {
      const stage = state.stage;
      const said = `<span${staged.kind === "failed" ? ` class="fx-wrong"` : ""}>${escape(staged.said)}</span>`;
      if (stage.kind === "fetching") {
        const percent = stage.total ? Math.min(100, Math.floor((stage.received / stage.total) * 100)) : null;
        return `${said}<span class="fx-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100"${percent === null ? "" : ` aria-valuenow="${percent}"`}><span style="width:${percent ?? 6}%"></span></span>`;
      }
      if (stage.kind === "administrator") return `${said}<span class="fx-admin"><span>${escape(words.adminThere)}</span><code class="mono">${escape(ADMIN_LINE)}</code>${copyButton(ADMIN_LINE, words.copy, `data-act="fx-copy"`)}</span>`;
      return staged.kind === "failed" ? `${said}${button("fx-versions", words.update)}` : said;
    }
    const megabytes = (bytes: number) => `${(bytes / 1_000_000).toFixed(1)} MB`;
    if (state.fetching) {
      const { received, total, via, version } = state.fetching;
      const percent = total ? Math.min(100, Math.floor((received / total) * 100)) : null;
      return `<span>${escape(version)} ${escape(words.fetching(megabytes(received), total ? megabytes(total) : null, via === "network"))}</span><span class="fx-bar" role="progressbar" aria-valuemin="0" aria-valuemax="100"${percent === null ? "" : ` aria-valuenow="${percent}"`}><span style="width:${percent ?? 6}%"></span></span>`;
    }
    if (state.renewing) {
      const { version, phase, error } = state.renewing;
      const said = phase === "failed" ? words.failed(error ?? words.phase.failed) : words.phase[phase];
      const step = `<span${phase === "failed" ? ` class="fx-wrong"` : ""}>${escape(`${version} ${said}`)}</span>`;
      if (phase !== "awaiting_administrator") return step;
      return `${step}<span class="fx-admin"><span>${escape(words.adminThere)}</span><code class="mono">${escape(ADMIN_LINE)}</code>${copyButton(ADMIN_LINE, words.copy, `data-act="fx-copy"`)}</span>`;
    }
    if (state.failed) return `<span class="fx-wrong" title="${escape(state.failed.why)}">${escape(words.failed(state.failed.why))}</span>${button("fx-versions", words.update)}`;
    if (state.take && state.app) {
      const said = state.open ? words.appTakes(state.take) : state.app.target.startsWith("windows") ? words.appClosed(state.take) : words.appOpenThere(state.take);
      return `<span>${escape(said)}</span>`;
    }
    const offer = state.offer && running && compareVersions(state.offer.version, running) > 0 ? state.offer.version : null;
    return offer ? `<span>${escape(words.canTake(offer))}</span>${button("fx-versions", words.update)}` : `<span>${escape(words.upToDate)}</span>`;
  }

  /** The newest release this device knows on the line of `running`. */
  private newest(local: UpdateState | null, running: string | null): string | null {
    return running ? newestKnown(local, running) : null;
  }

  private choices(state: UpdateState): string[] {
    const { words } = this.options;
    const choice = (kind: string, on: boolean) =>
      `<div class="choice-options">${([true, false] as const)
        .map(
          (value) =>
            `<button type="button" class="choice${value === on ? " chosen" : ""}" role="radio" aria-checked="${value === on}" data-act="fx-choice" data-kind="${kind}" data-value="${value ? "on" : "off"}">${escape(value ? words.on : words.off)}</button>`,
        )
        .join("")}</div>`;
    const auto = state.auto ?? state.settings.auto;
    const byDefault = state.settings.chosen ? "" : `<span class="choice-note">${escape(words.autoByDefault)}</span>`;
    return [
      `<dt>${escape(words.auto)}</dt><dd>${choice("auto", auto)}${byDefault}</dd>`,
      `<dt>${escape(words.relays)}</dt><dd>${choice("relays", state.settings.relays)}</dd>`,
    ];
  }

  /** The block's own buttons; whether the press was one of them. */
  click(target: HTMLElement): boolean {
    const subject = this.subject;
    const act = target.dataset.act;
    if (!subject || !act?.startsWith("fx-")) return false;
    const { machine, manages } = subject;
    switch (act) {
      case "fx-versions-here":
        this.options.versionsHere(true);
        return true;
      case "fx-copy":
        void copyFrom(target, this.options.words.copied);
        return true;
      case "fx-versions":
        this.versions(subject);
        return true;
      case "fx-undue":
        if (manages) this.act(this.options.due(manages, machine.name, null));
        return true;
      case "fx-older":
        this.act(this.options.older(machine.name));
        return true;
      case "fx-choice": {
        const state = this.state;
        if (!state) return true;
        const settings: UpdateSettings = { ...state.settings };
        if (target.dataset.kind === "auto") {
          settings.auto = target.dataset.value === "on";
          settings.chosen = true;
        } else settings.relays = target.dataset.value === "on";
        this.act(this.options.remote(machine.written, { do: "set", settings }));
        return true;
      }
    }
    return false;
  }

  /** The window of versions for this machine: an update picked goes to it
   *  now, or, while it is away, is kept by the hub until it is back. */
  private versions(subject: FastctxSubject): void {
    const { machine, manages } = subject;
    const local = this.options.local.peek();
    const state = this.state ?? local;
    if (!state || !manages) return;
    const running = this.state?.running ?? subject.version ?? "";
    const away = machine.online === false;
    this.options.versions.open(
      {
        who: machine.written,
        running,
        state,
        ...(away ? {} : { look: () => this.options.remote(machine.written, { do: "look" }) }),
      },
      {
        kind: "update",
        pick: async (version) => {
          if (away) await this.options.due(manages, machine.name, version);
          else this.state = await this.options.remote(machine.written, { do: "kernel", version });
          this.options.changed();
          void this.ask();
        },
      },
    );
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
        this.said = this.options.words.failed(error instanceof Error ? error.message : String(error));
        this.options.changed();
      },
    );
  }
}
