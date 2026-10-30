// One agent's logins (`world-gui.md`, 智能体页; `world-agent-logins.md`):
// Codex or Claude Code, each a section of its own under 「智能体」 in the
// sidebar; one machine at a time, chosen from the head's menu of every
// machine the user may look at. Opening the
// page starts no program: it reads what that machine's node recorded of its
// agents, and a machine never scanned shows only the agent's mark and
// 「扫描这台电脑」. A scan finds candidates in the usual places; the folders
// the user chose are searched below them (`./agent-folders`), and what a
// search finds is asked who it is only when taken up. Hooking a candidate up
// is a real change, made on a card of its own (`./agent-change`), and only
// then does the login join the list the World uses.
//
// A listed login is a row (`./agent-login-row`); with the workbench on it
// carries its usage windows and what it used this week, and opens its own
// page of usage (`./usage/login-page`). The machine last shown is kept on
// this device; coming from a machine's page shows that one.
//
// Where the device is logged in to an account, a switch in the head turns
// the page to the account's preferences for this agent (`./host-prefs`):
// they are the account's, not a machine's, so the machine's pill gives way
// to the account's.

import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { AccountChange, HubAccounts, Login as AccountLogin, PlanLabel } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Folders } from "../world/focus";
import type { AgentLogins, ChangePreview, FolderChange, Login, LoginChange, MachineLogins, WorldLogins } from "../world/logins";
import type { UsageReport } from "../world/usage";
import type { AccessWords } from "./account-words";
import { ActsMenu, type MenuEntry } from "./acts-menu";
import { AgentChangeSheet, type AgentChangeTarget, type AgentChangeWords } from "./agent-change";
import { foldersSection, foundOf, freshness, searching } from "./agent-folders";
import { CLAUDE_OLDEST, claudeTooOld, dirName, loginRow, planName, programLine } from "./agent-login-row";
import type { AgentLoginsWords } from "./agent-logins-words";
import { chosen, pickButton, pickMenu, pickables, type Pickable } from "./agent-machines";
import { keepTab } from "./agent-sections";
import { AssistSheet, assistChange, assistancesIn, fiveHourWindow, namesOn, type Assistance } from "./assist-sheet";
import type { Confirm } from "./confirm";
import { FolderPicker } from "./folder-picker";
import { redrawKeepingFocus } from "./focus";
import type { HostPrefs } from "./host-prefs";
import { detailOf } from "./problem";
import { isVisitor } from "./reach";
import type { AgentTab, ProfileWords } from "./profile-words";
import type { Page } from "./shell";
import { effect, type Signal } from "./signals";
import type { Store } from "./store";
import { keepRecord, keptRecord, keptReport, readerNow, readReport, reportKey } from "./usage/kept";
import { usageOf } from "./usage/model";
import { assistingMe, namer } from "./usage/people";
import type { UsageWords } from "./usage/words";

type Agent = "codex" | "claude";

export interface AgentLoginsOptions {
  store: Store;
  words: AgentLoginsWords;
  changeWords: AgentChangeWords;
  access: AccessWords;
  usage: UsageWords;
  head(title: string, actions?: string): string;
  /** The agent whose logins it shows. */
  agent: Agent;
  /** The machine to show, when one was named. */
  machine: string | null;
  /** The tab shown first. */
  tab: AgentTab;
  /** The account's preferences for an agent, where this device keeps an
   *  account's own configuration. */
  prefs: { words: ProfileWords; make(agent: Agent): HostPrefs } | null;
  /** The machine shown changed, or the agent, as when a folder chosen is the
   *  other agent's login: the address and the sidebar follow. */
  showing(machine: string, agent: Agent): void;
  /** The tab shown changed: the address follows. */
  tabbed(tab: AgentTab, agent: Agent): void;
  when(at: Date, now: Date): string;
  /** Whether this device's workbench is on: the limits and the usage
   *  belong to it, and without it they are neither read nor shown. */
  workbench: Signal<boolean>;
  read(machine: string | null): Promise<WorldLogins>;
  scan(machine: string, stop: boolean): Promise<MachineLogins>;
  add(machine: string, agent: Agent, dir: string): Promise<MachineLogins>;
  folder(machine: string, dir: string, change: FolderChange): Promise<MachineLogins>;
  /** This computer's own folder picker, where it has one. */
  pick: (() => Promise<string | null>) | null;
  /** A machine's folders, walked by its node, where its picker is out of
   *  reach. */
  folders(machine: string, path: string | null): Promise<Folders>;
  preview(machine: string, agent: Agent, dir: string, into: boolean, takeBack: boolean): Promise<ChangePreview>;
  change(machine: string, agent: Agent, dir: string, into: boolean, raise: boolean, takeBack: boolean): Promise<LoginChange>;
  limits(machine: string, agent: Agent, dir: string): Promise<MachineLogins>;
  /** Codex only: turns naming new conversations in a login on or off. */
  setTitle(machine: string, agent: Agent, dir: string, on: boolean): Promise<MachineLogins>;
  usageOf(machine: string, agent: Agent, dir: string): Promise<UsageReport>;
  change_(login: AccountLogin, change: AccountChange): Promise<void>;
  confirm: Confirm;
}

/** The machine shown: what the menu knows of it, and its record or why
 *  there is none. */
interface Shown extends Pickable {
  heard: { kind: "answered"; logins: MachineLogins } | { kind: "away"; said: string; detail: string } | { kind: "reading" };
  /** The user may scan it and change its logins. */
  mine: boolean;
}

const MACHINE = "agents.machine";
/** While a scan, a search or a change runs, its record is read again this
 *  often: reading is a file the node keeps, and a search's count moves on
 *  as it goes. */
const FOLLOW_MS = 1200;
/** How long a row just added shows as new. */
const FRESH_MS = 2400;
/** A login's usage is read again when the page is drawn this long after
 *  it was last read; it is the machine's own ledger, no model is asked. */
const USAGE_STALE_MS = 60_000;

/** One menu for the rows' 「⋯」, however often the page is opened. */
let shared: ActsMenu | null = null;
function rowsMenu(): ActsMenu {
  shared ??= new ActsMenu();
  return shared;
}

export class AgentLoginsPage implements Page {
  readonly element: HTMLElement;
  title: string;
  private readonly body: HTMLElement;
  private readonly pick: HTMLElement;
  private readonly stop: () => void;
  private readonly sheet: AgentChangeSheet;
  private readonly assistSheet: AssistSheet;
  private readonly choosing: FolderPicker;
  private agent: Agent;
  /** The machine shown, as the menu writes it. */
  private machine: string | null;
  private record: MachineLogins | null = null;
  private problem: { machine: string; detail: string; offline: boolean } | null = null;
  private reading: string | null = null;
  /** The machine whose record was asked for since it was shown. */
  private fetched: string | null = null;
  private following = 0;
  /** What the last thing done on a machine, row or folder left to say. */
  private notes = new Map<string, { text: string; detail: string }>();
  /** Machines a folder is being chosen on. */
  private picking = new Set<string>();
  /** Folders whose whole list of what they found is open. */
  private openFolders = new Set<string>();
  /** What a search found that is being taken up, by its full path. */
  private taking = new Set<string>();
  /** Rows just added, shown as new for a moment, each with when. */
  private fresh = new Map<string, number>();
  private readonly menu = rowsMenu();
  private machines: { element: HTMLElement; popover: Popover } | null = null;
  private pickDrawn = "";
  private drawn = "";
  private prefs: HostPrefs | null;
  private readonly tabs: HTMLElement;
  private tab: AgentTab;
  private tabsDrawn = "";

  /** The calls that name a machine, as `options` makes them. */
  private readonly calls: Pick<AgentLoginsOptions, "read" | "scan" | "add" | "folder" | "folders" | "preview" | "change" | "limits" | "setTitle" | "usageOf">;

  constructor(private readonly options: AgentLoginsOptions) {
    // Every call names the machine shown as this computer writes it, its
    // hub with it where another hub has a different computer of that name.
    const as = (machine: string) => {
      const shown = this.current(this.options.store.world.peek()).machine;
      return shown?.name === machine ? shown.written : machine;
    };
    this.calls = {
      read: (machine) => options.read(machine === null ? null : as(machine)),
      scan: (machine, stop) => options.scan(as(machine), stop),
      add: (machine, agent, dir) => options.add(as(machine), agent, dir),
      folder: (machine, dir, change) => options.folder(as(machine), dir, change),
      folders: (machine, path) => options.folders(as(machine), path),
      preview: (machine, agent, dir, into, takeBack) => options.preview(as(machine), agent, dir, into, takeBack),
      change: (machine, agent, dir, into, raise, takeBack) => options.change(as(machine), agent, dir, into, raise, takeBack),
      limits: (machine, agent, dir) => options.limits(as(machine), agent, dir),
      setTitle: (machine, agent, dir, on) => options.setTitle(as(machine), agent, dir, on),
      usageOf: (machine, agent, dir) => options.usageOf(as(machine), agent, dir),
    };
    const { words } = options;
    this.agent = options.agent;
    this.title = options.agent === "claude" ? words.claude : words.codex;
    this.machine = options.machine ?? keptMachine();
    if (options.machine) keepMachine(options.machine);
    this.tab = options.tab;
    this.prefs = options.prefs?.make(options.agent) ?? null;
    this.element = document.createElement("div");
    this.element.className = "agents-page logins-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title logins-title">${titleHtml(this.agent, this.title)}</h1><span class="host-tabs-place"></span><span class="agents-pick-place"></span><span class="host-account-place"></span>`, `<div class="logins-machine-state"></div><span class="host-status-place"></span>`)}
      <div class="logins-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".logins-body")!;
    this.pick = this.element.querySelector<HTMLElement>(".agents-pick-place")!;
    this.tabs = this.element.querySelector<HTMLElement>(".host-tabs-place")!;
    if (this.prefs) {
      this.element.querySelector(".host-account-place")!.replaceWith(this.prefs.account);
      this.element.querySelector(".host-status-place")!.replaceWith(this.prefs.status);
      this.element.append(this.prefs.element);
    }
    this.tabs.addEventListener("click", (event) => {
      const tab = (event.target as HTMLElement).closest<HTMLElement>("[data-host-tab]")?.dataset.hostTab as AgentTab | undefined;
      if (tab) this.chooseTab(tab);
    });
    this.assistSheet = new AssistSheet(options.access);
    this.sheet = new AgentChangeSheet(options.changeWords, {
      preview: (target) => this.calls.preview(target.machine, target.agent, target.dir, target.into, !!target.takeBack),
      change: (target, raise) => this.calls.change(target.machine, target.agent, target.dir, target.into, raise, !!target.takeBack),
      progress: async (target) => {
        const record = await this.calls.read(target.machine);
        const machine = record.machines.find((one) => one.kind === "answered" && one.logins.machine === target.machine);
        if (machine?.kind !== "answered") return null;
        const agent = machine.logins[target.agent];
        return [...agent.listed, ...agent.candidates].find((login) => login.dir === target.dir)?.change ?? null;
      },
      ended: () => void this.read(),
    });
    this.pick.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const item = target.closest<HTMLElement>("[data-pick]");
      if (item) {
        this.machines?.popover.hide();
        this.show(item.dataset.pick!);
      } else if (target.closest("[data-pick-open]")) this.openMachines();
    });
    this.element.querySelector(".logins-machine-state")!.addEventListener("click", (event) => void this.click(event));
    this.choosing = new FolderPicker(options.words.picker);
    this.body.addEventListener("click", (event) => void this.click(event));
    this.stop = effect(() => {
      const world = options.store.world.get();
      options.store.clock.get();
      options.workbench.get();
      this.prefs?.draw(world);
      this.drawTabs();
      this.draw();
      this.ensure();
    });
  }

  leave(): void {
    this.stop();
    clearTimeout(this.following);
    this.machines?.element.remove();
    this.menu.close();
    this.prefs?.leave();
  }

  /** The tab shown: the preferences only where there is an account whose
   *  they are. */
  private shownTab(): AgentTab {
    if (this.tab === "prefs" && this.prefs?.available()) return "prefs";
    return "logins";
  }

  private chooseTab(tab: AgentTab): void {
    if (tab === this.shownTab()) return;
    this.tab = tab;
    keepTab(tab);
    this.drawTabs();
    this.options.tabbed(tab, this.agent);
  }

  /** The preferences, made anew for the agent shown once it changed. */
  private remakePrefs(): void {
    const old = this.prefs;
    if (!old || !this.options.prefs) return;
    const next = this.options.prefs.make(this.agent);
    old.leave();
    old.account.replaceWith(next.account);
    old.status.replaceWith(next.status);
    old.element.replaceWith(next.element);
    this.prefs = next;
    next.draw(this.options.store.world.peek());
    this.drawTabs();
  }

  /** The switch between the logins and the preferences, and what each
   *  shows in the head and the body. */
  private drawTabs(): void {
    const shown = this.shownTab();
    const words = this.options.prefs?.words;
    const html =
      this.prefs?.available() && words
        ? `<div class="segmented host-tabs" role="tablist">${(["logins", "prefs"] as const)
            .map((tab) => `<button class="segment${tab === shown ? " chosen" : ""}" type="button" role="tab" aria-selected="${tab === shown}" data-host-tab="${tab}">${escape(words.tabs[tab])}</button>`)
            .join("")}</div>`
        : "";
    if (html !== this.tabsDrawn) {
      this.tabs.innerHTML = html;
      this.tabsDrawn = html;
    }
    const prefs = shown === "prefs";
    this.element.classList.toggle("host-tabbed", html !== "");
    this.body.hidden = shown !== "logins";
    this.pick.hidden = shown !== "logins";
    this.element.querySelector<HTMLElement>(".logins-machine-state")!.hidden = shown !== "logins";
    if (this.prefs) {
      this.prefs.element.hidden = !prefs;
      this.prefs.account.hidden = !prefs;
      this.prefs.status.hidden = !prefs;
      if (prefs) this.prefs.shown();
    }
  }

  /** Reads the machine shown once the World knows it, unless its record
   *  was asked for since it was shown, or is being read. */
  private ensure(): void {
    const machine = this.current(this.options.store.world.peek()).machine;
    if (!machine || this.reading === machine.name || this.fetched === machine.name) return;
    this.fetched = machine.name;
    // What was last read of it shows at once, while it is read anew.
    if (this.record?.machine !== machine.name) {
      this.record = keptRecord(machine.name);
      if (this.record) this.draw();
    }
    void this.read();
  }

  /** The machines of the menu, and the one shown among them. */
  private current(world: WorldNow | null): { machines: Pickable[]; machine: Pickable | null } {
    const machines = pickables(world);
    return { machines, machine: chosen(machines, this.machine, null) };
  }

  /** Shows `machine`: its record as last kept at once, then as read now. */
  private show(machine: string): void {
    if (machine === this.machine) return;
    this.machine = machine;
    keepMachine(machine);
    const shown = this.current(this.options.store.world.peek()).machine;
    if (shown) this.options.showing(shown.written, this.agent);
    this.record = shown ? keptRecord(shown.name) : null;
    this.problem = null;
    this.fetched = null;
    this.draw();
    this.ensure();
  }

  /** Reads what the machine shown recorded; while a scan or a change runs
   *  there, reads again until it ends. Then its logins' usage. */
  private async read(): Promise<void> {
    const machine = this.current(this.options.store.world.peek()).machine;
    if (!machine || this.reading === machine.name) return;
    const name = machine.name;
    this.reading = name;
    const asked = readerNow();
    try {
      const world = await this.calls.read(name);
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
    void this.readUsage();
  }

  /** Reads the usage of the logins listed under the tab shown, those not
   *  read for a while; each row fills in as its own comes. */
  private async readUsage(): Promise<void> {
    const record = this.record;
    if (!record || !this.options.workbench.peek()) return;
    const agent = this.agent;
    await Promise.allSettled(
      record[agent].listed.map(async (login) => {
        const key = reportKey(record.machine, agent, login.dir);
        const kept = keptReport(key);
        if (kept && Date.now() - kept.at < USAGE_STALE_MS) return;
        await readReport(key, () => this.calls.usageOf(record.machine, agent, login.dir));
        this.draw();
      }),
    );
  }

  private running(): boolean {
    const logins = this.record;
    if (!logins) return false;
    const scanning = logins.scan?.usual.state === "running" || searching(logins.folders ?? []);
    const changing = (["codex", "claude"] as const).some((agent) => [...logins[agent].listed, ...logins[agent].candidates].some((login) => login.change && !login.change.end));
    return scanning || changing;
  }

  /** The machine shown, with its record. */
  private shown(world: WorldNow | null): Shown | null {
    const { words } = this.options;
    const machine = this.current(world).machine;
    if (!machine) return null;
    // Its logins are listed and changed by those who hold it: its owner, and
    // those it is shared with whole.
    const mine = machine.hold === "mine" || machine.hold === "shared";
    if (this.record?.machine === machine.name) return { ...machine, mine, heard: { kind: "answered", logins: this.record } };
    if (this.problem?.machine === machine.name) {
      const said = this.problem.offline || machine.online === false ? words.offline : words.noAnswer;
      return { ...machine, mine, heard: { kind: "away", said, detail: this.problem.detail } };
    }
    return { ...machine, mine, heard: { kind: "reading" } };
  }

  private draw(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const now = store.clock.peek();
    const machine = this.shown(world);
    this.drawPick(world);
    const state = this.element.querySelector<HTMLElement>(".logins-machine-state")!;
    const stateHtml = machine ? this.state(machine) : "";
    if (state.innerHTML !== stateHtml) state.innerHTML = stateHtml;
    const html = machine ? this.machineBody(machine, world, now) : world ? `<p class="feature-quiet">${escape(words.noneFound)}</p>` : "";
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
  }

  /** The head's machine and, while it is open, its menu. */
  private drawPick(world: WorldNow | null): void {
    const { machines, machine } = this.current(world);
    const button = pickButton(machine, this.options.words);
    if (button !== this.pickDrawn) {
      // The pill is drawn anew: a menu open on it opens again on the new.
      const open = this.machines?.popover.open ?? false;
      this.machines?.element.remove();
      this.machines = null;
      this.pick.innerHTML = button;
      this.pickDrawn = button;
      if (open) this.openMachines();
    }
    if (this.machines) {
      const menu = pickMenu(machines, machine, this.options.words);
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
    const menu = pickMenu(machines, machine, this.options.words);
    element.innerHTML = menu;
    element.dataset.drawn = menu;
    this.pick.append(element);
    const popover = new Popover(element, anchor, () => {});
    this.machines = { element, popover };
    popover.show();
    element.querySelector<HTMLElement>(".agents-pick-item.current")?.focus({ preventScroll: true });
  }

  /** Beside the tabs: the agent's programs on the machine, how its scan
   *  goes, and scanning it again. */
  private state(machine: Shown): string {
    const { words } = this.options;
    if (machine.heard.kind !== "answered") return "";
    const record = machine.heard.logins;
    const scan = record.scan;
    if (!scan) return "";
    const scanning = scan.usual.state === "running";
    const status = scanning
      ? `<span class="logins-scanning"><span class="pulse" aria-hidden="true"></span>${escape(words.scanning)}</span>`
      : scan.usual.state === "failed"
        ? `<span class="logins-scanning wrong" title="${escape(scan.usual.reason)}">${escape(words.scanFailed)}</span>`
        : "";
    const rescan = machine.mine && !scanning ? `<button class="text-button quiet-button logins-rescan" type="button" data-act="scan" data-machine="${escape(machine.name)}" title="${escape(words.rescan)}">${glyph("scan", 14)}<span>${escape(words.rescan)}</span></button>` : "";
    return `${machine.hold === "assist" ? "" : this.programs(record[this.agent])}${status}${rescan}`;
  }

  private machineBody(machine: Shown, world: WorldNow | null, now: Date): string {
    const { words } = this.options;
    if (machine.heard.kind === "reading") return `<p class="feature-quiet logins-reading">${escape(words.reading)}</p>`;
    if (machine.heard.kind === "away") {
      return `<div class="logins-empty">${glyph(machine.here ? "desktop" : "server", 40)}<p class="feature-quiet" title="${escape(machine.heard.detail)}">${escape(machine.heard.said)}</p></div>`;
    }
    const record = machine.heard.logins;
    const agent = record[this.agent];
    const scan = record.scan;
    const note = this.notes.get(`m:${machine.name}`);
    // Never scanned: the agent's mark and the one thing to do.
    if (!scan && machine.hold !== "assist") {
      return `<div class="logins-empty">${glyph(this.agent, 40)}
          ${machine.mine ? `<button class="sheet-go" type="button" data-act="scan" data-machine="${escape(machine.name)}">${escape(machine.here ? words.scanHere : words.scanThere)}</button>` : ""}
          ${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
        </div>`;
    }
    const scanning = scan?.usual.state === "running";
    const listed = agent.listed.map((login) => this.row(machine, login, agent, world, now)).join("");
    const candidates = machine.mine
      ? agent.candidates
          .map((login) => {
            const who = login.identity?.who.kind === "known" ? (login.identity.who.value.email ?? null) : null;
            const changing = login.change && !login.change.end;
            const fresh = freshness(this.fresh, `d:${machine.name}:${login.dir}`);
            const old = claudeTooOld(this.agent, agent.programs, login);
            return `<li class="logins-candidate${fresh}>${glyph(this.agent, 16)}<span class="logins-candidate-name" title="${escape(login.dir)}">${escape(who ?? dirName(login.dir))}</span>${login.default ? `<span class="tag">${escape(words.defaultLogin)}</span>` : ""}${who ? `<span class="logins-dir">${escape(dirName(login.dir))}</span>` : ""}${old ? `<span class="tag tag-warn" title="${escape(words.tooOldSaid(old, CLAUDE_OLDEST))}">${escape(words.tooOld)}</span>` : ""}
          <button class="text-button logins-apply" type="button" data-act="apply" data-machine="${escape(machine.name)}" data-dir="${escape(login.dir)}"${changing ? " disabled" : ""}>${escape(changing ? words.applying : words.apply)}</button></li>`;
          })
          .join("")
      : "";
    const folders = record.folders ?? [];
    const searched = machine.mine
      ? foldersSection({
          machine: machine.name,
          agent: this.agent,
          mine: true,
          folders,
          words,
          now,
          when: this.options.when,
          open: this.openFolders,
          taking: this.taking,
          fresh: this.fresh,
          notes: this.notes,
          choosing: this.picking.has(machine.name),
        })
      : "";
    const none = machine.hold === "assist" ? words.noneForYou(this.agent) : words.noneFound;
    const empty = !agent.listed.length && !candidates && !(machine.mine && foundOf(folders, this.agent)) && !scanning ? `<p class="feature-quiet logins-none">${escape(none)}</p>` : "";
    return `${note ? `<p class="feature-note" title="${escape(note.detail)}">${escape(note.text)}</p>` : ""}
      ${listed ? `<ul class="logins-rows${this.options.workbench.peek() ? "" : " plain"}" aria-label="${escape(words.listed)}">${listed}</ul>` : ""}
      ${candidates ? `<h3 class="logins-group">${escape(words.candidates)}</h3><ul class="logins-candidates">${candidates}</ul>` : ""}
      ${empty}${searched}`;
  }

  /** The agent's programs on the machine, as one quiet chip with the list
   *  a hover away. */
  private programs(agent: AgentLogins): string {
    const { words } = this.options;
    if (!agent.programs.length) return `<span class="logins-programs quiet">${escape(words.noProgram)}</span>`;
    const lines = agent.programs.map((program) => programLine(program, words)).join("\n");
    return `<span class="logins-programs" title="${escape(lines)}" tabindex="0">${escape(words.programs(agent.programs.length))}</span>`;
  }

  private row(machine: Shown, login: Login, agent: AgentLogins, world: WorldNow | null, now: Date): string {
    const { options } = this;
    const report = keptReport(reportKey(machine.name, this.agent, login.dir))?.report ?? null;
    return loginRow(
      {
        machine: machine.name,
        agent: this.agent,
        login,
        versions: agent.programs
          .filter((_, index) => login.programs.includes(index))
          .map((program) => programLine(program, options.words))
          .join("\n"),
        old: claudeTooOld(this.agent, agent.programs, login),
        plan: login.identity?.who.kind === "known" && login.identity.who.value.method === "oauth_token" ? this.planOf(machine, login, world) : login.identity?.who.kind === "known" ? (login.identity.who.value.plan ?? null) : null,
        assistsMe: machine.hold === "assist" ? assistingMe(world, machine.name, this.agent, login.dir, login.default) : null,
        usage: report ? usageOf(report, namer(world, machine.name, options.usage)) : null,
        workbench: options.workbench.peek(),
        fresh: freshness(this.fresh, `d:${machine.name}:${login.dir}`),
        note: this.notes.get(`s:${machine.name}:${login.dir}`),
        now,
      },
      { words: options.words, access: options.access, usage: options.usage, when: options.when },
    );
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

  private assistancesOf(machine: Shown, login: Login, world: WorldNow | null): Assistance[] {
    if (!machine.ownerLogin) return [];
    return assistancesIn(this.hubOf(machine.ownerLogin, world)?.assists ?? [], machine.name, this.agent, login.default ? null : login.dir);
  }

  /** The assist sheet for one login, with `editing`'s assistance open. */
  private openAssist(machineName: string, dir: string, editing: string | null): void {
    const { options } = this;
    const found = this.find(machineName, dir);
    const owner = found?.machine.ownerLogin;
    if (!found?.login || !owner) return;
    const home = found.login.default ? null : dir;
    const agent = this.agent;
    const label = this.target(machineName, dir, true)?.who ?? dirName(dir);
    const limits = found.login.limits;
    this.assistSheet.open({
      title: options.access.assistTitle(label, machineName),
      assistances: this.assistancesOf(found.machine, found.login, options.store.world.peek()),
      assist: async (to, terms) => {
        await options.change_(owner, assistChange(machineName, agent, home, to, terms));
        await options.store.refresh();
        const again = this.find(machineName, dir);
        return again?.login ? this.assistancesOf(again.machine, again.login, options.store.world.peek()) : [];
      },
      editing,
      costs: agent === "claude",
      names: namesOn(this.hubOf(owner, options.store.world.peek()), owner),
      window: limits.kind === "known" ? fiveHourWindow(limits.value.windows) : null,
    });
  }

  private find(machineName: string, dir: string): { machine: Shown; login: Login | null; agent: AgentLogins } | null {
    const machine = this.shown(this.options.store.world.peek());
    if (!machine || machine.name !== machineName || machine.heard.kind !== "answered") return null;
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
      programs: found.agent.programs
        .filter((_, index) => found.login?.programs.includes(index))
        .map((program) => ({
          name: program.within?.name ?? this.options.words.commandLine(program.version ?? null),
          detail: programLine(program, this.options.words),
          within: !!program.within,
        })),
      here: found.machine.here,
      into,
    };
  }

  private async click(event: Event): Promise<void> {
    const target = event.target as HTMLElement;
    const button = target.closest<HTMLButtonElement>("[data-act]");
    if (!button) {
      // A row opens the login's usage, as its name does; a hover's text or
      // a link inside it keeps its own.
      const row = target.closest<HTMLElement>("[data-open]");
      if (row && !target.closest("a") && !getSelection()?.toString()) location.hash = row.dataset.open!;
      return;
    }
    if (button.disabled) return;
    const machine = button.dataset.machine ?? "";
    const dir = button.dataset.dir ?? "";
    switch (button.dataset.act) {
      case "scan":
        await this.quietly(button, `m:${machine}`, () => this.calls.scan(machine, false));
        return;
      case "apply": {
        const target = this.target(machine, dir, true) ?? { agent: this.agent, machine, dir, who: dirName(dir), programs: [], here: this.shown(this.options.store.world.peek())?.here ?? false, into: true };
        this.sheet.open(target);
        return;
      }
      // A listed login whose tools wait for the user: put in again, what
      // they changed of them taken back where that is what holds them.
      case "drift": {
        const target = this.target(machine, dir, true);
        if (target) this.sheet.open({ ...target, takeBack: this.find(machine, dir)?.login?.drift?.kind === "changed" });
        return;
      }
      case "add-folder":
        this.chooseFolder(machine);
        return;
      case "stop-search":
        await this.quietly(button, `f:${machine}:${dir}`, () => this.calls.folder(machine, dir, { change: "stop" }));
        return;
      case "forget":
        await this.quietly(button, `f:${machine}:${dir}`, () => this.calls.folder(machine, dir, { change: "forget" }));
        return;
      case "more":
        this.openFolders.add(`${machine}:${dir}`);
        this.draw();
        return;
      case "take":
        await this.takeUp(machine, dir, button.dataset.folder ?? "");
        return;
      case "menu":
        this.openMenu(button, machine, dir);
        return;
    }
  }

  /** The row's 「⋯」: assist others, read the limits again, copy where its
   *  directory is, on a token login the plan the administrator shows, and
   *  last, apart from the rest, removing it. */
  private openMenu(anchor: HTMLElement, machineName: string, dir: string): void {
    const { words, store } = this.options;
    const found = this.find(machineName, dir);
    if (!found?.login) return;
    const login = found.login;
    const world = store.world.peek();
    const identity = login.identity?.who.kind === "known" ? login.identity.who.value : null;
    const owner = found.machine.ownerLogin;
    const admin = !!owner && !!this.hubOf(owner, world)?.accounts.some((account) => account.level === "super" && account.name === owner.account);
    const entries: MenuEntry[] = [];
    // A visitor lets none of its agents assist anyone (`world-network.md`,
    // 一个中枢，几个账号).
    if (owner && !isVisitor(world, owner)) entries.push({ act: "assist", icon: "person", label: words.assist });
    // The windows of a login that only assists the reader are its owner's.
    if (this.options.workbench.peek() && found.machine.hold !== "assist") entries.push({ act: "limits", icon: "reread", label: words.rereadLimits });
    entries.push({ act: "copy", icon: "folder", label: words.copyDir });
    // Codex names its own conversations when the login's owner asks it to;
    // Claude Code does its own. Only a login the World's tools are in hosts
    // World conversations, so only such a login shows it.
    if (this.agent === "codex" && found.machine.mine && login.integration === "in") {
      entries.push({ rule: true }, { act: "title", icon: "edit", label: words.autoTitle, on: !!login.auto_title, hint: words.autoTitleWhy });
    }
    if (identity?.method === "oauth_token" && admin) {
      const current = this.planOf(found.machine, login, world);
      const plans: (PlanLabel | null)[] = ["max", "pro", "free", null];
      entries.push(
        { rule: true },
        { note: words.plan },
        ...plans.map((plan) => ({ act: `plan:${plan ?? ""}`, label: plan ? planName(plan) : words.planNone, checked: current === plan })),
      );
    }
    if (found.machine.mine) entries.push({ rule: true }, { act: "remove", icon: "archive", label: words.remove, danger: true });
    this.menu.open(anchor, identity?.email ?? dirName(login.dir), entries, (act) => {
      if (act.startsWith("plan:")) void this.menuAct("plan", act.slice("plan:".length), machineName, dir);
      else void this.menuAct(act, "", machineName, dir);
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
      case "assist":
        this.openAssist(machineName, dir, null);
        return;
      case "limits":
        await this.quietly(null, key, () => this.calls.limits(machineName, this.agent, dir));
        return;
      case "title":
        await this.quietly(null, key, () => this.calls.setTitle(machineName, this.agent, dir, !found.login!.auto_title));
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
        const owner = found.machine.ownerLogin;
        if (!owner) return;
        await this.quietly(null, key, async () => {
          await options.change_(owner, { change: "plan", machine: machineName, agent: this.agent, ...(found.login!.default ? {} : { home: dir }), plan: (plan || null) as PlanLabel | null });
          await options.store.refresh();
        });
        return;
      }
    }
  }

  /** 「添加文件夹…」: this computer's own folder picker where it has one,
   *  the machine's folders walked by its node anywhere else. */
  private chooseFolder(machine: string): void {
    const here = this.shown(this.options.store.world.peek())?.here ?? false;
    const walk = () => this.choosing.open({ machine, folders: (path) => this.calls.folders(machine, path), chosen: (dir) => void this.choose(machine, dir) });
    const pick = this.options.pick;
    if (!here || !pick) {
      walk();
      return;
    }
    this.picking.add(machine);
    this.draw();
    pick().then(
      (dir) => {
        this.picking.delete(machine);
        if (dir) void this.choose(machine, dir);
        else this.draw();
      },
      // A computer whose picker cannot open walks its folders as any
      // other machine does.
      () => {
        this.picking.delete(machine);
        this.draw();
        walk();
      },
    );
  }

  /** Chooses `dir` on `machine` for logins, then shows what it became: a
   *  login of an agent, its row marked new, under that agent's tab; or a
   *  folder being searched. */
  private async choose(machine: string, dir: string): Promise<void> {
    const before = this.record?.machine === machine ? this.record : null;
    this.picking.add(machine);
    this.notes.delete(`m:${machine}`);
    this.draw();
    try {
      const after = await this.calls.folder(machine, dir, { change: "choose" });
      const marked = this.became(before, after, dir);
      if (marked.agent && marked.agent !== this.agent) {
        this.agent = marked.agent;
        this.title = marked.agent === "claude" ? this.options.words.claude : this.options.words.codex;
        const heading = this.element.querySelector<HTMLElement>(".logins-title");
        if (heading) heading.innerHTML = titleHtml(this.agent, this.title);
        this.remakePrefs();
        this.options.showing(machine, marked.agent);
      }
      this.mark(marked.keys);
    } catch (error) {
      this.notes.set(`m:${machine}`, { text: this.options.words.failed, detail: detailOf(error) });
    } finally {
      this.picking.delete(machine);
    }
    await this.read();
  }

  /** What choosing `dir` made of it on a machine: the logins that are new,
   *  or the one it already was, with their agent; else the folder. */
  private became(before: MachineLogins | null, after: MachineLogins, dir: string): { agent: Agent | null; keys: string[] } {
    const machine = after.machine;
    for (const agent of [this.agent, this.agent === "codex" ? "claude" : "codex"] as const) {
      const logins = [...after[agent].listed, ...after[agent].candidates];
      const known = before ? [...before[agent].listed, ...before[agent].candidates] : null;
      const fresh = logins.filter((login) => sameDir(login.dir, dir) || (known !== null && !known.some((one) => sameDir(one.dir, login.dir))));
      if (fresh.length) return { agent, keys: fresh.map((login) => `d:${machine}:${login.dir}`) };
    }
    const folder = (after.folders ?? []).find((one) => sameDir(one.dir, dir));
    return { agent: null, keys: folder ? [`f:${machine}:${folder.dir}`] : [] };
  }

  /** Takes up what a folder's search found, asking who is logged in
   *  there; the change card for it opens once it is known. */
  private async takeUp(machine: string, dir: string, folder: string): Promise<void> {
    const agent = this.agent;
    this.taking.add(dir);
    this.notes.delete(`f:${machine}:${folder}`);
    this.draw();
    try {
      await this.calls.add(machine, agent, dir);
      await this.read();
      const target = this.agent === agent ? this.target(machine, dir, true) : null;
      if (target) this.sheet.open(target);
    } catch (error) {
      this.notes.set(`f:${machine}:${folder}`, { text: this.options.words.failed, detail: detailOf(error) });
    } finally {
      this.taking.delete(dir);
    }
    this.draw();
  }

  /** Marks rows as new for a moment. */
  private mark(keys: string[]): void {
    const at = Date.now();
    for (const key of keys) this.fresh.set(key, at);
    window.setTimeout(() => {
      for (const key of keys) if (this.fresh.get(key) === at) this.fresh.delete(key);
      this.draw();
    }, FRESH_MS);
  }

  /** Does `act`, says on the machine or row what went wrong if it did,
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

/** The head's name of the agent shown: its mark, then its name. */
function titleHtml(agent: Agent, title: string): string {
  return `<span class="logins-title-mark">${glyph(agent, 20)}</span><span class="logins-title-name">${escape(title)}</span>`;
}

function keptMachine(): string | null {
  try {
    return localStorage.getItem(MACHINE);
  } catch {
    return null;
  }
}

function keepMachine(machine: string): void {
  try {
    localStorage.setItem(MACHINE, machine);
  } catch {
    // Not kept: this computer shows first next time.
  }
}

/** Whether two paths a machine wrote name the same directory: a Windows
 *  path without regard to case, slash or the verbatim prefix. */
function sameDir(a: string, b: string): boolean {
  const plain = (path: string) => {
    const bare = path.replace(/^\\\\\?\\/, "");
    const trimmed = bare.replace(/[\\/]+$/, "") || bare;
    return /^[A-Za-z]:|\\/.test(bare) ? trimmed.replace(/\//g, "\\").toLowerCase() : trimmed;
  };
  return plain(a) === plain(b);
}
