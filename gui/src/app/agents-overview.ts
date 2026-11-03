// The agents of one machine at a glance (`world-gui.md`, 接入与工作台): a
// paper card for each agent found there, a line for each of its logins.
// A connected login stands in ink; one found and not yet connected offers
// to connect; a connection changed since, or asking for more, offers to be
// renewed, in clay. Connecting or renewing opens the window that tells
// what it changes (`agent-change.ts`). Claude Code's and Codex's connected
// logins offer the workbench. Each agent's own page, with its folders, its
// scans and each login's menu, is a step further in, from the card.

import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import { agentGlyph, glyph } from "../conversation/glyphs";
import type { WorldNow } from "../world/bridge";
import type { AgentLogins, ChangePreview, Login, LoginChange, MachineLogins, WorldLogins } from "../world/logins";
import { AgentChangeSheet, type AgentChangeTarget, type AgentChangeWords } from "./agent-change";
import { CLAUDE_OLDEST, claudeTooOld, dirName, planName, programLine } from "./agent-login-row";
import type { AgentLoginsWords } from "./agent-logins-words";
import { chosen, keepMachine, keptMachine, pickButton, pickMenu, pickables, type Pickable } from "./agent-machines";
import type { AgentsOverviewWords } from "./agents-overview-words";
import "./agents-overview.css";
import { loginLine, type LineTag, type Standing } from "./login-line";
import { reachDrawing, type Reach } from "./agents-reach";
import { redrawKeepingFocus } from "./focus";
import { link } from "./places";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect, type Signal } from "./signals";
import type { Store } from "./store";
import { keepRecord, keptRecord, readerNow } from "./usage/kept";

/** An agent, by the word the node names it by. */
type Agent = string;

/** The two agents with a workbench and a page of their own, first; the
 *  others the node found follow (`MachineLogins.others`). */
const HOSTS = ["claude", "codex"] as const;
type Host = (typeof HOSTS)[number];

const isHost = (agent: Agent): agent is Host => (HOSTS as readonly string[]).includes(agent);

/** Every agent a machine's record names, hosts first. */
function agentsOf(record: MachineLogins): Agent[] {
  return [...HOSTS, ...Object.keys(record.others ?? {}).sort()];
}

/** An agent's logins in a machine's record. */
function sideOf(record: MachineLogins, agent: Agent): AgentLogins | undefined {
  return isHost(agent) ? record[agent] : record.others?.[agent];
}

/** While a scan or a change runs there, the machine's record is read again
 *  this often. */
const FOLLOW_MS = 1200;

export interface AgentsOverviewOptions {
  store: Store;
  words: AgentsOverviewWords;
  logins: AgentLoginsWords;
  changeWords: AgentChangeWords;
  head(title: string, actions?: string): string;
  /** The machine to show, when one was named. */
  machine: string | null;
  workbench: Signal<boolean>;
  read(machine: string | null): Promise<WorldLogins>;
  scan(machine: string, stop: boolean): Promise<MachineLogins>;
  preview(machine: string, agent: Agent, dir: string, into: boolean, takeBack: boolean, workbench?: boolean): Promise<ChangePreview>;
  change(machine: string, agent: Agent, dir: string, into: boolean, raise: boolean, takeBack: boolean, expected?: string, workbench?: boolean): Promise<LoginChange>;
  /** The machine shown changed: the address follows. */
  showing(machine: string): void;
  /** Opens the workbench on a machine whose node keeps it by account. */
  workbenchOn(machine: string, agent: Agent, dir: string): void;
  /** The workbench was opened or closed in a login. */
  benched(open: boolean): void;
  /** Starts a conversation with `agent`'s login `home` on `machine`, its
   *  default one where none, the workbench open in it. */
  talkOn(machine: string, agent: Agent, home: string | null): void;
}

export class AgentsOverview implements Page {
  readonly element = document.createElement("section");
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly pick: HTMLElement;
  private readonly sheet: AgentChangeSheet;
  private machine: string | null;
  private record: MachineLogins | null = null;
  private problem: { machine: string; detail: string; offline: boolean } | null = null;
  private reading: string | null = null;
  private fetched: string | null = null;
  private following = 0;
  private drawn = "";
  private pickDrawn = "";
  private machines: { element: HTMLElement; popover: Popover } | null = null;
  private readonly stop: () => void;

  constructor(private readonly options: AgentsOverviewOptions) {
    const { words } = options;
    this.title = words.title;
    this.machine = options.machine ?? keptMachine();
    if (options.machine) keepMachine(options.machine);
    this.element.className = "page ao-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1><span class="agents-pick-place"></span>`, `<div class="ao-state"></div>`)}
      <div class="ao-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".ao-body")!;
    this.pick = this.element.querySelector<HTMLElement>(".agents-pick-place")!;
    this.sheet = new AgentChangeSheet(options.changeWords, {
      preview: (target) => options.preview(this.written(target.machine), target.agent, target.dir, target.into, !!target.takeBack, target.workbench),
      change: (target, raise, expected) => options.change(this.written(target.machine), target.agent, target.dir, target.into, raise, !!target.takeBack, expected, target.workbench),
      progress: async (target) => {
        const record = await options.read(this.written(target.machine));
        const machine = record.machines.find((one) => one.kind === "answered" && one.logins.machine === target.machine);
        if (machine?.kind !== "answered") return null;
        const agent = sideOf(machine.logins, target.agent);
        return agent ? ([...agent.listed, ...agent.candidates].find((login) => login.dir === target.dir)?.change ?? null) : null;
      },
      ended: () => void this.read(),
      benched: (open) => options.benched(open),
    });
    this.pick.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const item = target.closest<HTMLElement>("[data-pick]");
      if (item) {
        this.machines?.popover.hide();
        this.show(item.dataset.pick!);
      } else if (target.closest("[data-pick-open]")) this.openMachines();
    });
    this.element.addEventListener("click", (event) => void this.click(event));
    this.stop = effect(() => {
      options.store.world.get();
      options.store.clock.get();
      options.workbench.get();
      this.draw();
      this.ensure();
    });
  }

  leave(): void {
    this.stop();
    clearTimeout(this.following);
    this.machines?.element.remove();
  }

  /** The machine named `name` as every call names it: as this computer
   *  writes it, its hub with it where another hub has one of that name. */
  private written(name: string): string {
    const shown = this.current(this.options.store.world.peek()).machine;
    return shown?.name === name ? shown.written : name;
  }

  /** The machines of the menu, and the one shown among them. */
  private current(world: WorldNow | null): { machines: Pickable[]; machine: Pickable | null } {
    const machines = pickables(world);
    return { machines, machine: chosen(machines, this.machine, null) };
  }

  private show(machine: string): void {
    if (machine === this.machine) return;
    this.machine = machine;
    keepMachine(machine);
    const shown = this.current(this.options.store.world.peek()).machine;
    if (shown) this.options.showing(shown.written);
    this.record = shown ? keptRecord(shown.name) : null;
    this.problem = null;
    this.fetched = null;
    this.draw();
    this.ensure();
  }

  /** Reads the machine shown once it is shown; what was last read of it
   *  shows at once, while it is read anew. */
  private ensure(): void {
    const machine = this.current(this.options.store.world.peek()).machine;
    if (!machine || this.reading === machine.name || this.fetched === machine.name) return;
    this.fetched = machine.name;
    if (this.record?.machine !== machine.name) {
      this.record = keptRecord(machine.name);
      if (this.record) this.draw();
    }
    void this.read();
  }

  /** Reads what the machine shown recorded; while a scan or a change runs
   *  there, reads again until it ends. */
  private async read(): Promise<void> {
    const machine = this.current(this.options.store.world.peek()).machine;
    if (!machine || this.reading === machine.name) return;
    const name = machine.name;
    this.reading = name;
    const asked = readerNow();
    try {
      const world = await this.options.read(machine.written);
      const heard = world.machines.find((one) => (one.kind === "answered" ? one.logins.machine : one.machine) === name) ?? world.machines[0];
      if (this.reading !== name) return;
      if (heard?.kind === "answered") {
        this.record = heard.logins;
        keepRecord(heard.logins, asked);
        this.problem = null;
      } else if (heard) {
        this.record = null;
        this.problem = { machine: name, detail: heard.kind === "no_answer" ? heard.reason : "", offline: heard.kind === "offline" };
      }
    } catch (error) {
      if (this.reading === name) this.problem = { machine: name, detail: detailOf(error), offline: false };
    } finally {
      if (this.reading === name) this.reading = null;
    }
    if (this.current(this.options.store.world.peek()).machine?.name !== name) return;
    this.draw();
    clearTimeout(this.following);
    if (this.running()) this.following = window.setTimeout(() => void this.read(), FOLLOW_MS);
  }

  private running(): boolean {
    const record = this.record;
    if (!record) return false;
    return (
      record.scan?.usual.state === "running" ||
      agentsOf(record).some((agent) => {
        const side = sideOf(record, agent);
        return !!side && [...side.listed, ...side.candidates].some((login) => login.change && !login.change.end);
      })
    );
  }

  private draw(): void {
    const world = this.options.store.world.peek();
    const machine = this.current(world).machine;
    this.drawPick(world);
    const state = this.element.querySelector<HTMLElement>(".ao-state")!;
    const stateHtml = machine ? this.state(machine) : "";
    if (state.innerHTML !== stateHtml) state.innerHTML = stateHtml;
    const html = machine ? this.machineBody(machine) : "";
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  /** Whether the user may connect and scan there: its owner, and those it
   *  is shared with whole. */
  private mine(machine: Pickable): boolean {
    return machine.hold === "mine" || machine.hold === "shared";
  }

  /** The head's end: scanning, or the way to scan again. */
  private state(machine: Pickable): string {
    const { logins } = this.options;
    const record = this.record?.machine === machine.name ? this.record : null;
    if (!record?.scan || !this.mine(machine)) return "";
    if (record.scan.usual.state === "running") return `<span class="ao-scanning" role="status">${escape(logins.scanning)}</span>`;
    return `<button class="text-button" type="button" data-ao="scan" data-machine="${escape(machine.name)}">${escape(logins.rescan)}</button>`;
  }

  private machineBody(machine: Pickable): string {
    const { logins, words } = this.options;
    if (this.problem?.machine === machine.name) {
      const said = this.problem.offline || machine.online === false ? logins.offline : logins.noAnswer;
      return `<p class="ao-quiet" title="${escape(this.problem.detail)}">${escape(said)}</p>`;
    }
    const record = this.record?.machine === machine.name ? this.record : null;
    if (!record) return `<p class="ao-quiet">${escape(logins.reading)}</p>`;
    // Never scanned: the agents' marks and the one thing to do. A machine
    // whose agents assist the user tells no scan, only the logins it may.
    if (!record.scan && machine.hold !== "assist") {
      return `<div class="ao-first">
        <span class="ao-first-marks" aria-hidden="true">${glyph("claude", 34)}${glyph("codex", 34)}</span>
        ${this.mine(machine) ? `<button class="sheet-go" type="button" data-ao="scan" data-machine="${escape(machine.name)}">${escape(machine.here ? logins.scanHere : logins.scanThere)}</button>` : ""}
      </div>`;
    }
    const found = agentsOf(record).filter((agent) => {
      const side = sideOf(record, agent);
      return !!side && (side.programs.length || side.listed.length || side.candidates.length);
    });
    const missing = HOSTS.filter((agent) => !found.includes(agent)).map((agent) => logins[agent]);
    return `${this.reach(machine, record, found)}<div class="ao-cards">${found.map((agent) => this.card(machine, record, agent)).join("")}</div>
      ${missing.length ? `<p class="ao-missing">${escape(words.missing(missing, machine.here))}</p>` : ""}`;
  }

  /** Where the agents found reach, drawn wide and narrow, the page's own
   *  width choosing: only where there is another machine of the user's. */
  private reach(machine: Pickable, record: MachineLogins, found: Agent[]): string {
    const others = pickables(this.options.store.world.peek())
      .filter((one) => one.name !== machine.name && (one.hold === "mine" || one.hold === "shared"))
      .sort((a, b) => Number(b.online !== false) - Number(a.online !== false));
    if (!others.length || !found.length) return "";
    const reach: Reach = { machine, agents: found.map((agent) => ({ agent, connected: (sideOf(record, agent)?.listed.length ?? 0) > 0 })), others };
    const more = this.options.words.more;
    return `<div class="ao-reach">${reachDrawing(reach, 680, more)}${reachDrawing(reach, 340, more)}</div>`;
  }

  /** An agent's card: its mark and name, the way to its own page, and a
   *  line for each login, connected ones first. */
  private card(machine: Pickable, record: MachineLogins, agent: Agent): string {
    const { logins, words } = this.options;
    const side = sideOf(record, agent) ?? { programs: [], listed: [], candidates: [], elsewhere: [] };
    const programs = side.programs.map((program) => programLine(program, logins)).join("\n\n");
    const lines = [...side.listed, ...(this.mine(machine) ? side.candidates : [])].map((login) => this.line(machine, side, agent, login, !!record.by_login)).join("");
    // Only Codex and Claude Code have a page of their own, for their
    // folders, scans and each login's menu.
    const manage = isHost(agent)
      ? `<a class="text-button ao-manage" href="${link({ kind: "agents", machine: machine.written, section: agent })}" title="${escape(words.manageSaid)}">${escape(words.manage)}</a>`
      : "";
    return `<section class="ao-card" data-agent="${escape(agent)}">
      <header class="ao-head">
        <span class="ao-mark" aria-hidden="true">${agentGlyph(agent, 22)}</span>
        <h2 class="ao-name"${programs ? ` title="${escape(programs)}"` : ""}>${escape(this.nameOf(agent, side))}</h2>
        ${manage}
      </header>
      ${lines ? `<ul class="ao-logins">${lines}</ul>` : `<p class="ao-none">${escape(words.noLogin)}</p>`}
    </section>`;
  }

  /** A login's line: whether it is connected, drawn; who it is; its plan
   *  and what to know of it; and what can be done to it. */
  private line(machine: Pickable, side: AgentLogins, agent: Agent, login: Login, byLogin: boolean): string {
    const { logins, words } = this.options;
    const listed = side.listed.includes(login);
    const busy = !!login.change && !login.change.end;
    const standing: Standing = busy ? "busy" : !listed ? "found" : login.drift || login.integration === "old" ? "renew" : "in";
    const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
    const old = agent === "claude" ? claudeTooOld(agent, side.programs, login) : null;
    const tags: LineTag[] = [
      login.default ? { text: logins.defaultLogin } : null,
      identity?.plan ? { text: planName(identity.plan) } : null,
      identity?.method === "oauth_token" ? { text: logins.token, said: logins.tokenSaid } : null,
      identity && !identity.logged_in ? { text: logins.loggedOut } : null,
      old ? { text: logins.tooOld, said: logins.tooOldSaid(old, CLAUDE_OLDEST), warn: true } : null,
      byLogin && login.workbench ? { text: words.workbench } : null,
    ].filter((tag): tag is LineTag => !!tag);
    const at = `data-machine="${escape(machine.name)}" data-agent="${escape(agent)}" data-dir="${escape(login.dir)}"`;
    const act =
      standing === "busy"
        ? `<span class="ll-busy">${escape(login.change?.into === false ? logins.removing : logins.applying)}</span>`
        : !this.mine(machine)
          ? ""
          : standing === "found"
            ? `<button class="text-button ll-go" type="button" data-ao="connect" ${at}>${escape(logins.apply)}</button>`
            : standing === "renew"
              ? `<button class="text-button ll-go" type="button" data-ao="renew" ${at} title="${escape(login.drift?.kind === "changed" ? login.drift.reason : logins.driftAsks)}">${escape(words.renew)}</button>`
              : !isHost(agent)
                ? // Another agent has no page of its own: it is taken out here.
                  `<button class="text-button ll-quiet" type="button" data-ao="remove" ${at}>${escape(logins.remove)}</button>`
                : byLogin && login.workbench
                  ? `<button class="text-button ll-quiet" type="button" data-ao="talk" ${at}>${escape(words.talk)}</button>`
                  : `<button class="text-button ll-quiet" type="button" data-ao="workbench" ${at} title="${escape(words.workbenchSaid)}">${escape(words.workbench)}</button>`;
    return loginLine({ standing, who: identity?.email ?? dirName(login.dir), whoSaid: login.dir, tags, act });
  }

  /** The head's machine and, while it is open, its menu. */
  private drawPick(world: WorldNow | null): void {
    const { machines, machine } = this.current(world);
    const button = pickButton(machine, this.options.logins);
    if (button !== this.pickDrawn) {
      const open = this.machines?.popover.open ?? false;
      this.machines?.element.remove();
      this.machines = null;
      this.pick.innerHTML = button;
      this.pickDrawn = button;
      if (open) this.openMachines();
    }
    if (this.machines) {
      const menu = pickMenu(machines, machine, this.options.logins);
      if (this.machines.element.dataset.drawn !== menu) {
        this.machines.element.innerHTML = menu;
        this.machines.element.dataset.drawn = menu;
      }
    }
  }

  private openMachines(): void {
    const anchor = this.pick.querySelector<HTMLElement>(".agents-pick");
    if (!anchor) return;
    if (this.machines) {
      this.machines.popover.toggle();
      return;
    }
    const { machines, machine } = this.current(this.options.store.world.peek());
    const element = document.createElement("div");
    element.className = "map-menu agents-pick-menu";
    element.setAttribute("role", "menu");
    const menu = pickMenu(machines, machine, this.options.logins);
    element.innerHTML = menu;
    element.dataset.drawn = menu;
    this.pick.append(element);
    this.machines = { element, popover: new Popover(element, anchor, () => {}) };
    this.machines.popover.show();
    element.querySelector<HTMLElement>(".agents-pick-item.current")?.focus({ preventScroll: true });
  }

  /** How the page names an agent: Codex and Claude Code by their own
   *  names, the others as their descriptions name them. */
  private nameOf(agent: Agent, side: AgentLogins | undefined): string {
    return isHost(agent) ? this.options.logins[agent] : (side?.title ?? agent);
  }

  /** What connecting or renewing a login changes is told by the window
   *  that makes the change. */
  private target(machine: Pickable, agent: Agent, dir: string, into: boolean): AgentChangeTarget | null {
    const record = this.record?.machine === machine.name ? this.record : null;
    const side = record ? sideOf(record, agent) : undefined;
    const login = side ? ([...side.listed, ...side.candidates].find((one) => one.dir === dir) ?? null) : null;
    if (!side || !login) return null;
    const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
    return {
      agent,
      title: this.nameOf(agent, side),
      machine: machine.name,
      dir,
      who: identity?.email ?? dirName(dir),
      programs: side.programs
        .filter((_, index) => login.programs.includes(index))
        .map((program) => ({ name: program.within?.name ?? this.options.logins.commandLine(program.version ?? null), detail: programLine(program, this.options.logins), within: !!program.within })),
      here: machine.here,
      into,
      takeBack: login.drift?.kind === "changed",
    };
  }

  private async click(event: Event): Promise<void> {
    const button = (event.target as HTMLElement).closest<HTMLElement>("[data-ao]");
    if (!button || (button as HTMLButtonElement).disabled) return;
    const machine = this.current(this.options.store.world.peek()).machine;
    if (!machine) return;
    const agent = button.dataset.agent as Agent | undefined;
    const dir = button.dataset.dir ?? "";
    switch (button.dataset.ao) {
      case "scan": {
        const record = await this.options.scan(machine.written, false).catch(() => null);
        if (record) {
          this.record = record;
          keepRecord(record, readerNow());
        }
        this.fetched = null;
        this.draw();
        void this.read();
        return;
      }
      case "connect":
      case "renew": {
        const target = agent ? this.target(machine, agent, dir, true) : null;
        if (target) this.sheet.open(target);
        return;
      }
      case "remove": {
        const target = agent ? this.target(machine, agent, dir, false) : null;
        if (target) this.sheet.open(target);
        return;
      }
      case "workbench": {
        if (!agent) return;
        // A node keeping the workbench by login opens it there, after its
        // window; an earlier one keeps it by account.
        const record = this.record?.machine === machine.name ? this.record : null;
        const target = record?.by_login ? this.target(machine, agent, dir, true) : null;
        if (target) this.sheet.open({ ...target, workbench: true });
        else this.options.workbenchOn(machine.written, agent, dir);
        return;
      }
      case "talk": {
        if (!agent) return;
        const record = this.record?.machine === machine.name ? this.record : null;
        const login = record ? sideOf(record, agent)?.listed.find((one) => one.dir === dir) : undefined;
        // The default login is named by none, as a conversation records it.
        this.options.talkOn(machine.written, agent, login?.default ? null : dir);
        return;
      }
    }
  }
}
