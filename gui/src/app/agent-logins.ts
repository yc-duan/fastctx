// The agents (`world-gui.md`, 账号、机器、共享与借出; `world-agent-logins.md`):
// Codex and Claude Code each in their own tab. Opening the page starts no
// program: it reads what each machine's node recorded of its agents, and
// a machine never scanned shows only the agent's mark and 「扫描这台电脑」.
// A scan finds candidates; hooking one up is a real change, made on a card
// of its own (`./agent-change`), and only then does the login join the
// list the World uses. A listed login is a slip: one line of who, a small
// tag for its plan (and for Claude's long-lived token), a bar for each
// usage window, the stamp, and a 「⋯」 with removing, lending, reading the
// limits again and its directory. Every machine of the network the user
// may look at comes under the same tab, each in its own section; the
// user's own other machines are scanned and hooked up from here too, by
// their own nodes.

import { glyph } from "../conversation/glyphs";
import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import type { AccountChange, HubAccounts, Login as AccountLogin, PlanLabel } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { AgentLimits, Fact, LimitWindow } from "../world/agents";
import type { WorldNow } from "../world/bridge";
import type { AgentLogins, ChangePreview, FoundProgram, Login, LoginChange, MachineLogins, WorldLogins } from "../world/logins";
import type { AccessWords } from "./account-words";
import { AgentChangeSheet, type AgentChangeTarget, type AgentChangeWords } from "./agent-change";
import type { AgentLoginsWords } from "./agent-logins-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet } from "./machine-sheets";
import { detailOf } from "./problem";
import { machinesOf, owns, ownerOf } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

type Agent = "codex" | "claude";

export interface AgentLoginsOptions {
  store: Store;
  words: AgentLoginsWords;
  changeWords: AgentChangeWords;
  access: AccessWords;
  head(title: string, actions?: string): string;
  /** The machine to show first, when one was named. */
  machine: string | null;
  when(at: Date, now: Date): string;
  read(machine: string | null): Promise<WorldLogins>;
  scan(machine: string, stop: boolean): Promise<MachineLogins>;
  add(machine: string, agent: Agent, dir: string): Promise<MachineLogins>;
  preview(machine: string, agent: Agent, dir: string, into: boolean): Promise<ChangePreview>;
  change(machine: string, agent: Agent, dir: string, into: boolean, raise: boolean): Promise<LoginChange>;
  limits(machine: string, agent: Agent, dir: string): Promise<MachineLogins>;
  change_(login: AccountLogin, change: AccountChange): Promise<void>;
  confirm: Confirm;
}

/** A machine as the page shows it. */
interface Shown {
  name: string;
  here: boolean;
  tag: string | null;
  /** The record, or why there is none. */
  heard: { kind: "answered"; logins: MachineLogins } | { kind: "away"; said: string };
  /** The account that owns it among this device's, who lends what is on it
   *  and sets plans; null on a machine of another's. */
  owner: AccountLogin | null;
}

const TAB = "agents.tab";
/** While a scan or a change runs, its record is read again this often:
 *  reading is a file the node keeps, and the walk reports progress about
 *  once a second. */
const FOLLOW_MS = 1200;

export class AgentLoginsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;
  private readonly sheet: AgentChangeSheet;
  private readonly giving: GiveSheet;
  private agent: Agent = keptTab();
  private record: WorldLogins | null = null;
  private problem: string | null = null;
  private reading = false;
  private following = 0;
  /** What the last thing done on a machine or slip left to say. */
  private notes = new Map<string, { text: string; detail: string }>();
  private adding: string | null = null;
  private openElsewhere = new Set<string>();
  private menu: { element: HTMLElement; popover: Popover } | null = null;
  private drawn = "";

  constructor(private readonly options: AgentLoginsOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "agents-page logins-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}
      <div class="logins-tabs" role="tablist">
        <button type="button" role="tab" data-tab="codex">${glyph("codex", 18)}<span>${escape(words.codex)}</span></button>
        <button type="button" role="tab" data-tab="claude">${glyph("claude", 18)}<span>${escape(words.claude)}</span></button>
      </div>
      <div class="logins-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".logins-body")!;
    this.giving = new GiveSheet(options.access);
    this.sheet = new AgentChangeSheet(options.changeWords, {
      preview: (target) => options.preview(target.machine, target.agent, target.dir, target.into),
      change: (target, raise) => options.change(target.machine, target.agent, target.dir, target.into, raise),
      progress: async (target) => {
        const record = await options.read(target.machine);
        const machine = record.machines.find((one) => one.kind === "answered" && one.logins.machine === target.machine);
        if (machine?.kind !== "answered") return null;
        const agent = machine.logins[target.agent];
        return [...agent.listed, ...agent.candidates].find((login) => login.dir === target.dir)?.change ?? null;
      },
      ended: () => void this.read(),
    });
    this.element.querySelector(".logins-tabs")!.addEventListener("click", (event) => {
      const tab = (event.target as HTMLElement).closest<HTMLElement>("[data-tab]")?.dataset.tab as Agent | undefined;
      if (!tab || tab === this.agent) return;
      this.agent = tab;
      keepTab(tab);
      this.draw();
    });
    this.body.addEventListener("click", (event) => void this.click(event));
    this.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.addDir(event.target as HTMLFormElement);
    });
    this.body.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && (event.target as HTMLElement).closest(".logins-add")) {
        this.adding = null;
        this.draw();
      }
    });
    this.stop = effect(() => {
      options.store.world.get();
      options.store.clock.get();
      this.draw();
    });
    void this.read();
  }

  leave(): void {
    this.stop();
    clearTimeout(this.following);
  }

  /** Reads what the nodes recorded; while a scan or a change runs on a
   *  machine, reads again until it ends. */
  private async read(): Promise<void> {
    if (this.reading) return;
    this.reading = true;
    try {
      this.record = await this.options.read(null);
      this.problem = null;
    } catch (error) {
      this.problem = detailOf(error);
    } finally {
      this.reading = false;
    }
    this.draw();
    clearTimeout(this.following);
    if (this.running()) this.following = window.setTimeout(() => void this.read(), FOLLOW_MS);
  }

  private running(): boolean {
    return (this.record?.machines ?? []).some((machine) => {
      if (machine.kind !== "answered") return false;
      const scan = machine.logins.scan;
      const scanning = scan?.usual.state === "running" || scan?.disks?.stage.state === "running";
      const changing = (["codex", "claude"] as const).some((agent) => [...machine.logins[agent].listed, ...machine.logins[agent].candidates].some((login) => login.change && !login.change.end));
      return scanning || changing;
    });
  }

  /** The machines shown: this computer first, then the others the record
   *  answered for, as the World names them. */
  private shown(world: WorldNow | null): Shown[] {
    const { words } = this.options;
    const logins = loginsOf(world?.accounts);
    const machines = machinesOf(world);
    const shown: Shown[] = (this.record?.machines ?? []).map((heard) => {
      const name = heard.kind === "answered" ? heard.logins.machine : heard.machine;
      const machine = machines.find((one) => one.name === name);
      const here = !!machine?.here || name === this.record?.machine;
      const owner = here
        ? (logins.find((login) => world?.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) ?? null)
        : machine
          ? (logins.find((login) => owns(machine, login)) ?? null)
          : null;
      const sharer = !here && !owner && machine ? (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null) : null;
      return {
        name,
        here,
        tag: here ? words.here : owner ? words.yours : sharer ? words.sharedBy(sharer) : null,
        heard: heard.kind === "answered" ? { kind: "answered", logins: heard.logins } : { kind: "away", said: heard.kind === "offline" ? words.offline : words.noAnswer },
        owner,
      };
    });
    const first = this.options.machine;
    return shown.sort((a, b) => Number(b.here) - Number(a.here) || Number(b.name === first) - Number(a.name === first) || a.name.localeCompare(b.name));
  }

  private draw(): void {
    const { words, store } = this.options;
    for (const tab of this.element.querySelectorAll<HTMLElement>("[data-tab]")) {
      const on = tab.dataset.tab === this.agent;
      tab.setAttribute("aria-selected", String(on));
      tab.classList.toggle("current", on);
    }
    const world = store.world.peek();
    const now = store.clock.peek();
    let html: string;
    if (!this.record) {
      html = this.problem ? `<p class="feature-note" title="${escape(this.problem)}">${escape(words.failed)}</p>` : "";
    } else {
      html = this.shown(world)
        .map((machine) => this.machine(machine, world, now))
        .join("");
    }
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  private machine(machine: Shown, world: WorldNow | null, now: Date): string {
    const { words } = this.options;
    const head = `<header class="logins-machine-head">${glyph(machine.here ? "desktop" : "server", 18)}<h2>${escape(machine.name)}</h2>${machine.tag ? `<span class="tag">${escape(machine.tag)}</span>` : ""}`;
    if (machine.heard.kind === "away") return `<section class="logins-machine">${head}<span class="logins-away">${escape(machine.heard.said)}</span></header></section>`;
    const record = machine.heard.logins;
    const agent = record[this.agent];
    const scan = record.scan;
    const mine = machine.here || !!machine.owner;
    const note = this.notes.get(`m:${machine.name}`);
    // Never scanned: the agent's mark and the one thing to do.
    if (!scan) {
      return `<section class="logins-machine">${head}</header>
        <div class="logins-empty">${glyph(this.agent, 40)}
          ${mine ? `<button class="sheet-go" type="button" data-act="scan" data-machine="${escape(machine.name)}">${escape(machine.here ? words.scanHere : words.scanThere)}</button>` : ""}
          ${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
        </div></section>`;
    }
    const scanning = scan.usual.state === "running";
    const walking = scan.disks?.stage.state === "running";
    const seen = (scan.disks?.disks ?? []).reduce((sum, disk) => sum + disk.directories, 0);
    const status = scanning
      ? `<span class="logins-scanning"><span class="pulse" aria-hidden="true"></span>${escape(words.scanning)}</span>`
      : walking
        ? `<span class="logins-scanning"><span class="pulse" aria-hidden="true"></span>${escape(words.walking(seen))}<button class="text-button quiet-button" type="button" data-act="stop-walk" data-machine="${escape(machine.name)}">${escape(words.stopWalk)}</button></span>`
        : scan.usual.state === "failed"
          ? `<span class="logins-scanning wrong" title="${escape(scan.usual.reason)}">${escape(words.scanFailed)}</span>`
          : "";
    const rescan = mine && !scanning && !walking ? `<button class="text-button quiet-button logins-rescan" type="button" data-act="scan" data-machine="${escape(machine.name)}">${glyph("scan", 14)}<span>${escape(words.rescan)}</span></button>` : "";
    const programs = this.programs(agent);
    const listed = agent.listed.map((login) => this.slip(machine, login, agent, world, now)).join("");
    const candidates = agent.candidates
      .map((login) => {
        const who = login.identity?.who.kind === "known" ? (login.identity.who.value.email ?? null) : null;
        const changing = login.change && !login.change.end;
        return `<li class="logins-candidate">${glyph(this.agent, 16)}<span class="logins-candidate-name" title="${escape(login.dir)}">${escape(who ?? dirName(login.dir))}</span>${login.default ? `<span class="tag">${escape(words.defaultLogin)}</span>` : ""}${who ? `<code class="logins-dir">${escape(dirName(login.dir))}</code>` : ""}
          ${mine ? `<button class="text-button logins-apply" type="button" data-act="apply" data-machine="${escape(machine.name)}" data-dir="${escape(login.dir)}"${changing ? " disabled" : ""}>${escape(changing ? words.applying : words.apply)}</button>` : ""}</li>`;
      })
      .join("");
    const elsewhereOpen = this.openElsewhere.has(machine.name);
    const elsewhere = agent.elsewhere.length
      ? `<div class="logins-elsewhere"><button class="text-button quiet-button" type="button" data-act="elsewhere" data-machine="${escape(machine.name)}" aria-expanded="${elsewhereOpen}">${glyph("chevron", 14)}<span>${escape(words.elsewhere(agent.elsewhere.length))}</span></button>
        ${elsewhereOpen ? `<ul class="logins-candidates">${agent.elsewhere.map((one) => `<li class="logins-candidate"><code class="logins-dir" title="${escape(one.traces.join(", "))}">${escape(one.dir)}</code>${mine ? `<button class="text-button logins-apply" type="button" data-act="apply" data-machine="${escape(machine.name)}" data-dir="${escape(one.dir)}">${escape(words.apply)}</button>` : ""}</li>`).join("")}</ul>` : ""}</div>`
      : "";
    const adding = this.adding === machine.name;
    const add = mine
      ? adding
        ? `<form class="logins-add"><input type="text" name="dir" placeholder="${escape(words.addDirPlaceholder)}" spellcheck="false" autocomplete="off" data-machine="${escape(machine.name)}"><button class="text-button" type="submit">${escape(words.add)}</button></form>`
        : `<button class="text-button quiet-button logins-add-open" type="button" data-act="add" data-machine="${escape(machine.name)}">${glyph("plus", 14)}<span>${escape(words.addDir)}</span></button>`
      : "";
    const empty = !agent.listed.length && !agent.candidates.length && !agent.elsewhere.length && !scanning ? `<p class="feature-quiet">${escape(words.noneFound)}</p>` : "";
    return `<section class="logins-machine">${head}${programs}${status}${rescan}</header>
      ${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
      ${listed ? `<ul class="logins-slips" aria-label="${escape(words.listed)}">${listed}</ul>` : ""}
      ${candidates ? `<h3 class="logins-group">${escape(words.candidates)}</h3><ul class="logins-candidates">${candidates}</ul>` : ""}
      ${elsewhere}${empty}${add}
    </section>`;
  }

  /** The agent's programs on the machine, as one quiet chip with the list
   *  a hover away. */
  private programs(agent: AgentLogins): string {
    const { words } = this.options;
    if (!agent.programs.length) return `<span class="logins-programs quiet">${escape(words.noProgram)}</span>`;
    const lines = agent.programs.map((program) => programLine(program, words)).join("\n");
    return `<span class="logins-programs" title="${escape(lines)}" tabindex="0">${escape(words.programs(agent.programs.length))}</span>`;
  }

  private slip(machine: Shown, login: Login, agent: AgentLogins, world: WorldNow | null, now: Date): string {
    const { words } = this.options;
    const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
    const token = identity?.method === "oauth_token";
    const plan = token ? this.planOf(machine, login, world) : (identity?.plan ?? null);
    const who = identity?.email ?? dirName(login.dir);
    const changing = login.change && !login.change.end;
    const lent = this.lentTo(machine, login, world);
    const versions = agent.programs
      .filter((_, index) => login.programs.includes(index))
      .map((program) => programLine(program, words))
      .join("\n");
    const key = `s:${machine.name}:${login.dir}`;
    const note = this.notes.get(key);
    return `<li class="logins-slip${changing ? " changing" : ""}">
      <div class="logins-slip-head">
        <span class="logins-slip-mark" title="${escape(versions)}">${glyph(this.agent, 18)}</span>
        <span class="logins-slip-who" title="${escape(login.dir)}">${escape(who)}</span>
        ${plan ? `<span class="tag">${escape(planName(plan))}</span>` : ""}
        ${token ? `<span class="tag tag-token" title="${escape(words.tokenSaid)}">${escape(words.token)}</span>` : ""}
        ${identity && !identity.logged_in ? `<span class="tag tag-warn">${escape(words.loggedOut)}</span>` : ""}
        <button class="icon-button logins-more" type="button" data-act="menu" data-machine="${escape(machine.name)}" data-dir="${escape(login.dir)}" aria-label="${escape(words.more)}" title="${escape(words.more)}">${glyph("more", 16)}</button>
      </div>
      ${this.bars(login.limits, now)}
      <div class="logins-slip-foot">
        <span class="logins-stamp${changing ? " busy" : ""}">${changing ? escape(login.change!.into ? words.applying : words.removing) : `${glyph("check", 14)}${escape(words.applied)}`}</span>
        ${lent.length ? `<span class="logins-lent">${escape(words.lentTo)}${lent.map((account) => `<button class="tag tag-removable" type="button" data-act="unlend" data-machine="${escape(machine.name)}" data-dir="${escape(login.dir)}" data-to="${escape(account)}" title="${escape(words.takeBack(account))}">${escape(account)}${glyph("cross", 12)}</button>`).join("")}</span>` : ""}
      </div>
      ${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
    </li>`;
  }

  /** A bar for each usage window: only bars on the slip, the numbers a
   *  hover away; a window never read is a dashed empty bar. */
  private bars(limits: Fact<AgentLimits>, now: Date): string {
    const { words, when } = this.options;
    if (limits.kind !== "known") {
      const why = limits.kind === "failed" ? limits.reason : words.notRead(this.agent);
      return `<div class="logins-bars" title="${escape(why)}"><span class="logins-bar unread"><span class="logins-bar-name">5h</span><span class="logins-bar-track"></span></span><span class="logins-bar unread"><span class="logins-bar-name">7d</span><span class="logins-bar-track"></span></span></div>`;
    }
    const value = limits.value;
    const credits = value.credits;
    const read = words.observed(when(new Date(value.observed_at), now));
    const rows = value.windows.map((window) => {
      const percent = `${(window.used_tenths / 10).toFixed(window.used_tenths % 10 ? 1 : 0)}%`;
      const said = [
        words.used(percent),
        window.resets_at ? words.resets(new Date(window.resets_at), now) : "",
        read,
        credits ? words.credits(credits.balance ?? null, credits.unlimited) : "",
      ]
        .filter(Boolean)
        .join(" · ");
      const width = Math.min(100, window.used_tenths / 10).toFixed(1);
      const spent = window.used_tenths >= 1000;
      const onCredits = spent && !!credits;
      return `<span class="logins-bar${levelOf(window)}" title="${escape(said)}"><span class="logins-bar-name">${escape(words.window(window.name, window.minutes ?? null))}</span><span class="logins-bar-track"><span style="width:${width}%"></span></span>${onCredits ? `<span class="logins-credits" title="${escape(words.creditsMark)}">¢</span>` : ""}</span>`;
    });
    return `<div class="logins-bars">${rows.join("")}</div>`;
  }

  private hubOf(login: AccountLogin, world: WorldNow | null): HubAccounts | null {
    return world?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  /** The plan the network's administrator set on a token login. */
  private planOf(machine: Shown, login: Login, world: WorldNow | null): string | null {
    const home = login.default ? null : login.dir;
    for (const hub of world?.accounts.hubs ?? []) {
      const set = (hub.plans ?? []).find((plan) => plan.machine === machine.name && plan.agent === this.agent && plan.home === home);
      if (set) return set.plan;
    }
    return null;
  }

  private lentTo(machine: Shown, login: Login, world: WorldNow | null): string[] {
    if (!machine.owner) return [];
    const home = login.default ? null : login.dir;
    return (this.hubOf(machine.owner, world)?.lends ?? [])
      .filter((lend) => lend.machine === machine.name && lend.agent === this.agent && (lend.home ?? null) === home)
      .map((lend) => lend.account);
  }

  private find(machineName: string, dir: string): { machine: Shown; login: Login | null; agent: AgentLogins } | null {
    const machine = this.shown(this.options.store.world.peek()).find((one) => one.name === machineName);
    if (!machine || machine.heard.kind !== "answered") return null;
    const agent = machine.heard.logins[this.agent];
    const login = [...agent.listed, ...agent.candidates].find((one) => one.dir === dir) ?? null;
    return { machine, login, agent };
  }

  private target(machineName: string, dir: string, into: boolean): AgentChangeTarget | null {
    const found = this.find(machineName, dir);
    if (!found) return null;
    const identity = found.login?.identity?.who.kind === "known" ? found.login.identity.who.value : null;
    return {
      agent: this.agent,
      machine: machineName,
      dir,
      who: identity?.email ?? dirName(dir),
      programs: found.agent.programs.filter((_, index) => found.login?.programs.includes(index)).map((program) => program.within?.name ?? program.path ?? this.options.words[this.agent]),
      here: found.machine.here,
      into,
    };
  }

  private async click(event: Event): Promise<void> {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const machine = button.dataset.machine ?? "";
    const dir = button.dataset.dir ?? "";
    switch (button.dataset.act) {
      case "scan":
        await this.quietly(button, `m:${machine}`, () => this.options.scan(machine, false));
        return;
      case "stop-walk":
        await this.quietly(button, `m:${machine}`, () => this.options.scan(machine, true));
        return;
      case "apply": {
        const target = this.target(machine, dir, true) ?? { agent: this.agent, machine, dir, who: dirName(dir), programs: [], here: this.shown(this.options.store.world.peek()).find((one) => one.name === machine)?.here ?? false, into: true };
        this.sheet.open(target);
        return;
      }
      case "elsewhere":
        if (this.openElsewhere.has(machine)) this.openElsewhere.delete(machine);
        else this.openElsewhere.add(machine);
        this.draw();
        return;
      case "add":
        this.adding = machine;
        this.draw();
        this.body.querySelector<HTMLInputElement>(".logins-add input")?.focus();
        return;
      case "menu":
        this.openMenu(button, machine, dir);
        return;
      case "unlend": {
        const found = this.find(machine, dir);
        if (!found?.machine.owner || !found.login) return;
        await this.quietly(button, `s:${machine}:${dir}`, async () => {
          await this.options.change_(found.machine.owner!, { change: "lend", machine, to: button.dataset.to ?? "", agent: this.agent, ...(found.login!.default ? {} : { home: dir }), lent: false });
          await this.options.store.refresh();
        });
        return;
      }
    }
  }

  /** The slip's 「⋯」: remove, lend, read the limits again, the directory,
   *  and on a token login the plan the administrator shows. */
  private openMenu(anchor: HTMLElement, machineName: string, dir: string): void {
    const { words, store } = this.options;
    const found = this.find(machineName, dir);
    if (!found?.login) return;
    const login = found.login;
    const world = store.world.peek();
    const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
    const admin = !!found.machine.owner && !!this.hubOf(found.machine.owner, world)?.accounts.some((account) => account.level === "super" && account.name === found.machine.owner!.account);
    const plans: (PlanLabel | null)[] = ["max", "pro", "free", null];
    const mine = found.machine.here || !!found.machine.owner;
    this.menu?.element.remove();
    const element = document.createElement("div");
    element.className = "map-menu logins-menu";
    element.setAttribute("role", "menu");
    element.innerHTML = [
      mine ? `<button class="map-menu-item danger" type="button" role="menuitem" data-menu="remove">${glyph("archive", 16)}<span>${escape(words.remove)}</span></button>` : "",
      found.machine.owner ? `<button class="map-menu-item" type="button" role="menuitem" data-menu="lend">${glyph("person", 16)}<span>${escape(words.lend)}</span></button>` : "",
      `<button class="map-menu-item" type="button" role="menuitem" data-menu="limits">${glyph("reread", 16)}<span>${escape(words.rereadLimits)}</span></button>`,
      `<button class="map-menu-item" type="button" role="menuitem" data-menu="copy" title="${escape(login.dir)}">${glyph("folder", 16)}<span>${escape(words.copyDir)}</span></button>`,
      identity?.method === "oauth_token" && admin
        ? `<div class="logins-menu-plan"><span>${escape(words.plan)}</span>${plans.map((plan) => `<button class="choice" type="button" data-menu="plan" data-plan="${plan ?? ""}">${escape(plan ? planName(plan) : words.planNone)}</button>`).join("")}</div>`
        : "",
    ].join("");
    anchor.closest(".logins-slip")!.append(element);
    const popover = new Popover(element, anchor, () => element.remove());
    this.menu = { element, popover };
    popover.show();
    element.addEventListener("click", (event) => {
      const item = (event.target as HTMLElement).closest<HTMLElement>("[data-menu]");
      if (!item) return;
      popover.hide();
      void this.menuAct(item.dataset.menu!, item.dataset.plan ?? "", machineName, dir);
    });
  }

  private async menuAct(act: string, plan: string, machineName: string, dir: string): Promise<void> {
    const { options } = this;
    const found = this.find(machineName, dir);
    if (!found?.login) return;
    const key = `s:${machineName}:${dir}`;
    switch (act) {
      case "remove": {
        const target = this.target(machineName, dir, false);
        if (target) this.sheet.open(target);
        return;
      }
      case "lend": {
        const owner = found.machine.owner;
        if (!owner) return;
        const home = found.login.default ? undefined : dir;
        const label = this.target(machineName, dir, true)?.who ?? dirName(dir);
        this.giving.open(
          { title: `${options.words.lend.replace("…", "")} ${label} · ${machineName}`, what: options.access.lendWhat, sees: options.access.lendSees, given: options.access.lentTo, go: options.access.lendGo },
          this.lentTo(found.machine, found.login, options.store.world.peek()),
          async (to, lent) => {
            await options.change_(owner, { change: "lend", machine: machineName, to, agent: this.agent, ...(home ? { home } : {}), lent });
            await options.store.refresh();
            const again = this.find(machineName, dir);
            return again?.login ? this.lentTo(again.machine, again.login, options.store.world.peek()) : [];
          },
        );
        return;
      }
      case "limits":
        await this.quietly(null, key, () => options.limits(machineName, this.agent, dir));
        return;
      case "copy":
        await navigator.clipboard.writeText(dir).catch(() => {});
        this.notes.set(key, { text: options.words.copied, detail: dir });
        this.draw();
        setTimeout(() => {
          this.notes.delete(key);
          this.draw();
        }, 1600);
        return;
      case "plan": {
        const owner = found.machine.owner;
        if (!owner) return;
        await this.quietly(null, key, async () => {
          await options.change_(owner, { change: "plan", machine: machineName, agent: this.agent, ...(found.login!.default ? {} : { home: dir }), plan: (plan || null) as PlanLabel | null });
          await options.store.refresh();
        });
        return;
      }
    }
  }

  private async addDir(form: HTMLFormElement): Promise<void> {
    const input = form.querySelector<HTMLInputElement>("input[name=dir]");
    const dir = input?.value.trim();
    const machine = input?.dataset.machine;
    if (!dir || !machine) return;
    this.adding = null;
    await this.quietly(null, `m:${machine}`, () => this.options.add(machine, this.agent, dir));
  }

  /** Does `act`, says on the machine or slip what went wrong if it did,
   *  and reads the record again. */
  private async quietly(button: HTMLButtonElement | null, key: string, act: () => Promise<unknown>): Promise<void> {
    if (button) button.disabled = true;
    this.notes.delete(key);
    try {
      await act();
    } catch (error) {
      this.notes.set(key, { text: this.options.words.failed, detail: detailOf(error) });
    } finally {
      if (button) button.disabled = false;
    }
    await this.read();
  }
}

function keptTab(): Agent {
  try {
    return localStorage.getItem(TAB) === "claude" ? "claude" : "codex";
  } catch {
    return "codex";
  }
}

function keepTab(tab: Agent): void {
  try {
    localStorage.setItem(TAB, tab);
  } catch {
    // Not kept: Codex shows first next time.
  }
}

function dirName(dir: string): string {
  return dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? dir;
}

function planName(plan: string): string {
  return plan.length ? plan[0]!.toUpperCase() + plan.slice(1) : plan;
}

function programLine(program: FoundProgram, words: AgentLoginsWords): string {
  const what = program.within ? words.within(program.within.name, program.within.version ?? null) : words.how[program.how];
  return [program.path ?? program.within?.path ?? "", program.version ?? "", what, program.on_path ? words.onPath : ""].filter(Boolean).join(" · ");
}

function levelOf(window: LimitWindow): string {
  return window.used_tenths >= 1000 ? " spent" : window.used_tenths >= 900 ? " high" : "";
}
