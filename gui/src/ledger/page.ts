// The agents' pages in the app: the specimen's views (`home.ts`,
// `login-page.ts`), drawn from the World (`live.ts`) and pressed through to
// it. One page stands for the whole section, from 「额度」 to a login's
// settings, so going between its places moves what is in it the way the
// specimen does: into a login from the right, back from the left, between
// the views under one head from the side the switch moved to.

import { AgentChangeSheet, type AgentChangeTarget } from "../app/agent-change";
import { pickables } from "../app/agent-machines";
import { FolderPicker } from "../app/folder-picker";
import type { AgentChangeWords } from "../app/agent-change";
import type { AgentLoginsWords } from "../app/agent-logins-words";
import { dirName, programLine } from "../app/agent-login-row";
import { LoginIdleSheet } from "../app/login-idle-sheet";
import { LoginModelsSheet } from "../app/login-models-sheet";
import { link, type LoginTab, type Place as AppPlace } from "../app/places";
import type { Page } from "../app/shell";
import { effect } from "../app/signals";
import type { Store } from "../app/store";
import { glyph } from "../conversation/glyphs";
import { sameHome } from "../app/reach";
import { loginsOf, type AccountChange, type Login as AccountLogin } from "../world/accounts";
import { IDLE_DEFAULT, type ChangePreview, type FolderChange, type IdleCompaction, type LoginChange, type MachineLogins, type ModelChange, type WorldLogins } from "../world/logins";
import { followState, LOGINS, type Following } from "../world/topics";
import type { Folders } from "../world/focus";
import type { UsageReport } from "../world/usage";
import type { UsageCallsPage, UsageCallsQuery } from "../world/usage-calls";
import { forget, grantsOf, lines, logins as shownLogins, ME, plateOf, setHost, setNet, SWING, type Deeds, type Feed, type Login, type Looked } from "./data";
import { homePage } from "./home";
import { askedOf, callsFrom, daysFrom, idOf, limitOf, managesOf, netOf, partsOf, reckonedBy, type Seen } from "./live";
import { fillCharts, loginPage, setUntils } from "./login-page";
import "./sheets";
import { actOf, draw, drawnBy, setLinker, state, toast, type How, type Place } from "./state";

export interface AgentsPageOptions {
  store: Store;
  words: AgentLoginsWords;
  changeWords: AgentChangeWords;
  read(machine: string | null): Promise<WorldLogins>;
  scan(machine: string, stop: boolean): Promise<MachineLogins>;
  add(machine: string, agent: string, dir: string): Promise<MachineLogins>;
  folder(machine: string, dir: string, change: FolderChange): Promise<MachineLogins>;
  /** This computer's own folder picker, where it has one. */
  pick: (() => Promise<string | null>) | null;
  /** A machine's folders as its node lists them, to walk where there is
   *  no picker of its own. */
  folders(machine: string, path: string | null, cursor?: string | null): Promise<Folders>;
  preview(machine: string, agent: string, dir: string, into: boolean, takeBack: boolean, workbench?: boolean): Promise<ChangePreview>;
  change(machine: string, agent: string, dir: string, into: boolean, raise: boolean, takeBack: boolean, expected?: string, workbench?: boolean): Promise<LoginChange>;
  limits(machine: string, agent: string, dir: string): Promise<MachineLogins>;
  setTitle(machine: string, agent: string, dir: string, on: boolean): Promise<MachineLogins>;
  setResetInPlace(machine: string, agent: string, dir: string, on: boolean): Promise<MachineLogins>;
  setWeek(machine: string, agent: string, dir: string, week: number | null, swing?: number): Promise<MachineLogins>;
  /** Resets the week's usage to tenths of a percent of the week's figure,
   *  or with none takes the reset `id` back. */
  resetUsage(machine: string, agent: string, dir: string, id: string, toTenths: number | null, weekEnd: string | null, holder: string | null): Promise<MachineLogins>;
  changeModels(machine: string, agent: string, dir: string, change: ModelChange): Promise<MachineLogins>;
  /** Compacting the reader's idle conversations in a login, for them alone. */
  setIdleCompaction(machine: string, agent: string, dir: string, setting: IdleCompaction): Promise<MachineLogins>;
  usage(machine: string, agent: string, dir: string): Promise<UsageReport>;
  calls(machine: string, agent: string, dir: string, query: UsageCallsQuery): Promise<UsageCallsPage>;
  change_(login: AccountLogin, change: AccountChange): Promise<void>;
  benched(open: boolean): void;
  talkOn(machine: string, agent: string, home: string | null): void;
  openTurn(machine: string, conversation: string, turn: string): void;
  /** A conversation's name, where this device knows it. */
  nameOf(conversation: string): string | null;
  /** Now, as the World the page shows reckons it. */
  now(): Date;
  /** What the sidebar's button says, where the sidebar folds away. */
  menuLabel: string;
}

/** The app's place for a place of the pages, and back. */
export function appPlaceOf(place: Place): AppPlace {
  if (place.kind === "quota") return { kind: "agents", machine: null };
  if (place.kind === "attach") return { kind: "agents", machine: place.machine };
  const { machine, agent, dir } = partsOf(place.id);
  return { kind: "usage", machine, agent: agent as "claude" | "codex", dir, ...(place.tab === "usage" ? {} : { tab: place.tab as LoginTab }) };
}
export function placeOfApp(place: AppPlace): Place {
  if (place.kind === "usage") return { kind: "login", id: idOf(place.machine, place.agent, place.dir), tab: place.turn ? "calls" : (place.tab ?? "usage") };
  if (place.kind === "agents" && place.machine) return { kind: "attach", machine: place.machine };
  return { kind: "quota" };
}

/** Where a place stands among the views under its head. */
const order = (place: Place) => (place.kind === "quota" ? 0 : place.kind === "attach" ? 1 : ["usage", "calls", "assist", "settings"].indexOf(place.tab));
/** How the page comes, going from one place to another. */
function way(from: Place, to: Place): How {
  if (to.kind === "login") {
    if (from.kind !== "login" || from.id !== to.id) return "forward";
  } else if (from.kind === "login") return "back";
  const step = order(to) - order(from);
  return step > 0 ? "right" : step < 0 ? "left" : "quiet";
}

/** While a scan, a search or a change runs somewhere, the records are read
 *  again this often; otherwise twice a minute. */
const BUSY_MS = 1500;
const IDLE_MS = 30_000;

/** Every machine's record of its logins as last read, kept from one visit
 *  of the page to the next: coming back, the page is drawn at once from
 *  what it knew, and read again behind that. */
const kept: Seen = { records: new Map(), plates: new Map(), expects: new Map(), resets: new Map(), weekEnds: new Map() };

export class AgentsPage implements Page {
  readonly element = document.createElement("div");
  readonly title = "智能体";
  private readonly main: HTMLElement;
  private readonly seen: Seen = kept;
  private readonly sheet: AgentChangeSheet;
  private readonly models: LoginModelsSheet;
  private readonly idleSheet: LoginIdleSheet;
  private readonly choosing: FolderPicker;
  private readonly people = { models: new Map<string, Set<string>>(), people: new Map<string, Set<string>>() };
  private following: Following | null = null;
  private readonly stop: () => void;
  private resized = 0;
  private reading = false;
  /** The last read failed: nothing more is on its way. */
  private failed = false;
  /** The machines whose nodes page their ledger by 100 alone. */
  private readonly legacyCalls = new Set<string>();
  /** The logins whose node reckons their week: it keeps a reset of one. */
  private readonly reckoning = new Set<string>();
  /** Changes made to the page but by drawing it, as a field put where a
   *  button stood: the page is no longer what was drawn last. What a chart
   *  says of the mark under the pointer, there and in its legend, is the
   *  chart's own. */
  private readonly touched = new MutationObserver((changes) => {
    const own = (node: Node) => (node instanceof Element ? node : node.parentElement)?.closest("[data-chart], .lg-legend");
    if (changes.some((change) => !own(change.target))) this.painted = "";
  });

  constructor(private readonly options: AgentsPageOptions, place: AppPlace) {
    this.element.className = "page lg-app";
    this.element.innerHTML = `<main class="lg-main"></main>`;
    this.main = this.element.querySelector<HTMLElement>(".lg-main")!;
    this.touched.observe(this.main, { childList: true, subtree: true, characterData: true });
    this.sheet = new AgentChangeSheet(options.changeWords, {
      preview: (target) => options.preview(target.machine, target.agent, target.dir, target.into, !!target.takeBack, target.workbench),
      change: (target, raise, expected) => options.change(target.machine, target.agent, target.dir, target.into, raise, !!target.takeBack, expected, target.workbench),
      progress: async (target) => {
        const record = await options.read(target.machine);
        const heard = record.machines.find((one) => one.kind === "answered" && one.logins.machine === target.machine);
        if (heard?.kind !== "answered") return null;
        const side = target.agent === "claude" ? heard.logins.claude : target.agent === "codex" ? heard.logins.codex : heard.logins.others?.[target.agent];
        return side ? ([...side.listed, ...side.candidates].find((login) => login.dir === target.dir)?.change ?? null) : null;
      },
      ended: () => void this.read(),
      benched: (open) => options.benched(open),
    });
    this.models = new LoginModelsSheet(options.words);
    this.idleSheet = new LoginIdleSheet(options.words);
    this.choosing = new FolderPicker(options.words.picker);
    setLinker((to) => link(appPlaceOf(to)));
    setHost({ feed: this.feed(), deeds: this.deeds() });
    drawnBy((how) => this.paint(how));
    setUntils((login, person) => {
      const { machine, agent, home } = this.recordOf(login);
      const seen = options.store.world.peek()?.accounts.hubs.flatMap((hub) => hub.assists).find((one) => one.machine === machine && one.agent === agent && sameHome(one.home, home) && one.account === person);
      return seen?.until ? new Date(seen.until) : null;
    });
    state.place = placeOfApp(place);
    state.turn = place.kind === "usage" ? (place.turn ?? null) : null;
    if (state.place.kind === "attach") state.machine = state.place.machine;
    else state.machine = pickables(options.store.world.peek()).find((one) => one.here)?.name ?? state.machine;
    this.element.addEventListener("click", (event) => this.click(event));
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if ((event.key === "Enter" || event.key === " ") && target.matches("[role='link'], [role='button']")) {
        event.preventDefault();
        target.click();
      }
    });
    this.stop = effect(() => {
      options.store.world.get();
      this.rebuild();
    });
    this.following = followState(LOGINS, () => this.read(), () => (this.busy() ? BUSY_MS : IDLE_MS), null);
    void this.read();
  }

  /** Goes to another of the section's places, as the address now says. */
  go(place: AppPlace): void {
    const from = state.place;
    const to = placeOfApp(place);
    if (to.kind === "attach") state.machine = to.machine;
    if (to.kind === "login" && (from.kind !== "login" || from.id !== to.id)) Object.assign(state, { page: 1, who: null, model: null, source: null, outcome: null, split: "none", chart: "days" });
    state.opened = null;
    state.editing = null;
    state.turn = place.kind === "usage" ? (place.turn ?? null) : null;
    state.place = to;
    draw(way(from, to));
  }

  leave(): void {
    this.touched.disconnect();
    this.stop();
    this.following?.();
    this.sized.disconnect();
    window.clearTimeout(this.resized);
    document.querySelector(".lg-pop")?.remove();
  }

  /** The chart's room changing width, with the window, as the sidebar comes
   *  and goes, or as the page lays its parts out otherwise: the chart is
   *  drawn for the width it has, so it alone is drawn again once its room is
   *  still. Nothing else of the page is, a page number being typed
   *  included (codex-f9af46d4). */
  private readonly sized = new ResizeObserver(() => {
    window.clearTimeout(this.resized);
    this.resized = window.setTimeout(() => {
      const holder = this.main.querySelector<HTMLElement>("[data-chart]");
      if (!holder || Math.floor(holder.clientWidth) === this.chartWidth) return;
      this.chartWidth = Math.floor(holder.clientWidth);
      fillCharts(this.main);
      this.touched.takeRecords();
    }, 120);
  });

  /** The width the chart was last drawn at. */
  private chartWidth = 0;

  /** What was drawn last, to draw nothing again where nothing changed. */
  private painted = "";

  private paint(how: How): void {
    const html = state.place.kind === "login" ? loginPage() : homePage();
    // Drawn again as what it shows comes in, or the window changes size, it
    // keeps the region that scrolls: what it holds is set anew in it, so a
    // finger on it, its glide and where it stands go on (the user,
    // 2026-11-20: as the agents' windows came in, a phone's page would not
    // scroll and went back to its top). Where nothing changed nothing is
    // drawn at all: a chart says which reading it draws, and the window
    // changing size draws the page again.
    if (how === "quiet" && html === this.painted) return;
    this.painted = html;
    const kept = this.main.querySelector<HTMLElement>(".lg-sheet")?.scrollTop ?? 0;
    this.main.dataset.coming = how;
    if (how !== "quiet" || !redrawAround(this.main, html, ".lg-sheet")) {
      this.main.innerHTML = html;
      if (how === "quiet") this.main.querySelector<HTMLElement>(".lg-sheet")?.scrollTo({ top: kept });
    }
    // The sidebar's button, where the sidebar folds away (`shell.ts`).
    this.main.querySelector(".lg-head")?.insertAdjacentHTML("afterbegin", `<button class="icon-button menu-button" type="button" aria-label="${this.options.menuLabel}">${glyph("menu", 20)}</button>`);
    fillCharts(this.main, how !== "quiet");
    // The chart drawn is watched for the room it has; a chart drawn anew
    // for the first time says only what width it was drawn at.
    this.sized.disconnect();
    const holder = this.main.querySelector<HTMLElement>("[data-chart]");
    if (holder) {
      this.chartWidth = Math.floor(holder.clientWidth);
      this.sized.observe(holder);
    }
    // What drawing changed is no change of the page's own.
    this.touched.takeRecords();
  }

  private click(event: MouseEvent): void {
    const target = event.target as HTMLElement;
    const act = target.closest<HTMLElement>("[data-act]");
    const go = target.closest<HTMLElement>("[data-go]");
    // The nearer of the two is the one pressed.
    if (act && (!go || go.contains(act))) {
      if (!(act as HTMLButtonElement).disabled) actOf(act.dataset.act!)?.(act, event);
      return;
    }
    if (go) location.hash = go.dataset.go!;
  }

  /** Builds the pages' network again from what is now known. */
  private rebuild(): void {
    setNet({ ...netOf(this.options.store.world.peek(), this.seen, this.options.now()), unread: this.seen.records.size === 0 && !this.failed });
    draw();
  }

  private busy(): boolean {
    return [...this.seen.records.values()].some((heard) => {
      if (heard.kind !== "answered") return false;
      const record = heard.logins;
      if (record.scan?.usual.state === "running") return true;
      if ((record.folders ?? []).some((folder) => folder.search?.stage.state === "running")) return true;
      return [record.claude, record.codex, ...Object.values(record.others ?? {})].some((side) => [...side.listed, ...side.candidates].some((login) => login.change && !login.change.end));
    });
  }

  /** Reads every machine's record of its logins. */
  private async read(): Promise<void> {
    if (this.reading) return;
    this.reading = true;
    try {
      const world = await this.options.read(null);
      for (const heard of world.machines) this.seen.records.set(heard.kind === "answered" ? heard.logins.machine : heard.machine, heard);
      this.failed = false;
      this.rebuild();
    } catch (error) {
      // What could not be read is said, and the page no longer waits for it.
      this.failed = true;
      this.rebuild();
      toast(error instanceof Error ? error.message : "读取失败");
    } finally {
      this.reading = false;
      this.following?.refresh();
    }
  }

  /** What a machine answered of a deed, taken in at once. */
  private took(record: MachineLogins): void {
    this.seen.records.set(record.machine, { kind: "answered", logins: record });
    this.rebuild();
  }

  /** The World's own record of a login, with its agent's side and machine. */
  private recordOf(login: Login) {
    const { machine, agent, dir } = partsOf(login.id.replace(/^found:/, ""));
    const heard = this.seen.records.get(machine);
    const record = heard?.kind === "answered" ? heard.logins : null;
    const side = !record ? null : agent === "claude" ? record.claude : agent === "codex" ? record.codex : (record.others?.[agent] ?? null);
    const one = side ? ([...side.listed, ...side.candidates].find((each) => each.dir === dir) ?? null) : null;
    const picked = pickables(this.options.store.world.peek()).find((each) => each.name === machine) ?? null;
    return { machine, agent, dir, side, one, picked, home: one?.default ? null : dir };
  }

  private target(login: Login, into: boolean): AgentChangeTarget {
    const { machine, agent, dir, side, one, picked } = this.recordOf(login);
    return {
      agent,
      ...(agent === "claude" || agent === "codex" ? {} : { title: login.family }),
      machine,
      dir,
      who: login.who || dirName(dir),
      programs: (side?.programs ?? [])
        .filter((_, index) => one?.programs.includes(index))
        .map((program) => ({ name: program.within?.name ?? this.options.words.commandLine(program.version ?? null), detail: programLine(program, this.options.words), within: !!program.within })),
      here: !!picked?.here,
      into,
    };
  }

  /** The login of this device that holds the machine: its owner's, or the
   *  one it is shared with whole. */
  private holder(login: Login): AccountLogin {
    const holder = this.recordOf(login).picked?.ownerLogin ?? this.managing(login);
    if (!holder) throw new Error("无权管理该机器的登录");
    return holder;
  }

  /** The login of this device that manages `login` with its owner: the
   *  account its assistance is given to, where the hub says it manages. */
  private managing(login: Login): AccountLogin | null {
    const { machine, agent, home } = this.recordOf(login);
    const accounts = this.options.store.world.peek()?.accounts;
    for (const mine of loginsOf(accounts)) {
      const hub = accounts?.hubs.find((hub) => hub.hub === mine.hub);
      if (hub?.assists.some((one) => one.account === mine.account && one.machine === machine && one.agent === agent && sameHome(one.home, home) && managesOf(one))) return mine;
    }
    return null;
  }

  private feed(): Feed {
    const { options } = this;
    return {
      days: async (login) => {
        const { machine, agent, dir } = this.recordOf(login);
        const report = await options.usage(machine, agent, dir);
        // What the node reckons the week to hold is what its parts are of;
        // the pages drawn from what was known before are drawn again.
        const changed = reckonedBy(this.seen, login, report);
        if (report.reckoning) this.reckoning.add(login.id);
        else this.reckoning.delete(login.id);
        if (changed) queueMicrotask(() => this.rebuild());
        const plate = report.reckoning ? report.reckoning.week / 1_000_000 : plateOf(login);
        return daysFrom(report, login, plate, options.store.world.peek(), options.now());
      },
      calls: async (login, query) => {
        const { machine, agent, dir, picked } = this.recordOf(login);
        const holder = picked?.ownerLogin ?? null;
        const person = query.who === null ? undefined : query.who === "我" ? (holder ? { account: { hub: holder.hub, account: holder.account }, with_machine: true } : { account: null }) : holder ? { account: { hub: holder.hub, account: query.who } } : undefined;
        let legacy = this.legacyCalls.has(machine);
        let { ask, skip } = askedOf(query, person, legacy);
        let page = await options.calls(machine, agent, dir, ask);
        // A node that says no size paged by 100: asked again as it pages.
        if (!legacy && page.size === undefined && query.size !== 100) {
          this.legacyCalls.add(machine);
          legacy = true;
          ({ ask, skip } = askedOf(query, person, legacy));
          page = await options.calls(machine, agent, dir, ask);
        }
        const models = this.people.models.get(login.id) ?? new Set<string>();
        const people = this.people.people.get(login.id) ?? new Set<string>();
        this.people.models.set(login.id, models);
        this.people.people.set(login.id, people);
        return callsFrom(page, query, skip, login, options.store.world.peek(), { models, people }, options.nameOf);
      },
    };
  }

  private deeds(): Deeds {
    const { options } = this;
    const refreshed = async (record: Promise<MachineLogins>) => this.took(await record);
    const accounts = async (change: AccountChange, login: Login) => {
      try {
        await options.change_(this.holder(login), change);
      } catch (error) {
        // A hub of before keeps no one managing a login with its owner.
        throw new Error(/upgrade it for that/.test(String(error)) ? "中枢的 Fastctx 版本较旧，暂不支持共管" : error instanceof Error ? error.message : String(error));
      }
      await options.store.refresh();
      this.rebuild();
    };
    const tooOld = (error: unknown) => /does not know|unknown variant|older/i.test(String(error));
    /** Resets the week's usage, or takes a reset back; what everyone used
     *  is read again behind it. */
    const reset = async (login: Login, id: string, toTenths: number | null, looked?: Looked) => {
      const { machine, agent, dir } = this.recordOf(login);
      // The week looked at: one that has turned since, or of another of the
      // vendor's accounts, is not reset. A sheet opened before the week was
      // read says so, never taking a week read since for the one it showed.
      const weekEnd = looked ? looked.weekEnd : (this.seen.weekEnds.get(login.id) ?? null);
      if (weekEnd === null || (looked && looked.holder === null)) throw new Error("用量还没读到，稍后再试");
      // A node that reckons no week keeps no reset of one.
      if (!this.reckoning.has(login.id)) throw new Error("该机器离线，或其 Fastctx 版本较旧，暂不能重置用量");
      try {
        const record = await options.resetUsage(machine, agent, dir, id, toTenths, weekEnd, looked?.holder ?? null);
        forget(login);
        this.took(record);
      } catch (error) {
        throw new Error(tooOld(error) ? "该机器的 Fastctx 版本较旧，暂不支持重置用量" : error instanceof Error ? error.message : String(error));
      }
    };
    const assisted = (login: Login, person: string, limit: number | null, until: Date | null, manages?: boolean, line = lines[login.id], direct?: boolean): AccountChange => {
      const { machine, agent, home } = this.recordOf(login);
      // The assistance as the hub keeps it: what of it is not changed now
      // goes back as it was, never worked out again from what is shown (an
      // earlier release's dollars, a daily limit this page does not show).
      const kept = options.store.world
        .peek()
        ?.accounts.hubs.flatMap((hub) => hub.assists)
        .find((one) => one.account === person && one.machine === machine && one.agent === agent && (one.home ?? null) === (home ?? null));
      const same = !!kept && limitOf(kept, plateOf(login)) === limit;
      const limits = same
        ? { ...(kept.weekly ? { weekly: kept.weekly } : {}), ...(kept.share ? { share: kept.share } : {}), ...(kept.daily ? { daily: kept.daily } : {}) }
        : limit !== null
          ? // Dollars for a node that knows no shares; a share for one that does.
            { weekly: Math.round(((plateOf(login) * limit) / 100) * 1_000_000), share: limit, ...(kept?.daily ? { daily: kept.daily } : {}) }
          : {};
      return {
        change: "assist",
        machine,
        to: person,
        agent,
        ...(home ? { home } : {}),
        assisting: true,
        ...(until ? { until: until.toISOString() } : {}),
        ...limits,
        ...(line?.fiveStop ? { five_hour: line.fiveStop } : {}),
        ...(line?.turns ? { turns: line.turns } : {}),
        // Who manages is its owner's to say; what is not said now stays.
        manages: manages ?? (!!kept && managesOf(kept)),
        // So is whether their machines link with this one directly.
        ...(direct === undefined ? {} : { direct }),
      } as AccountChange;
    };
    return {
      attach: (login) => {
        if (!login.id.startsWith("found:")) return this.sheet.open({ ...this.target(login, true), ...(login.drift ? { takeBack: this.recordOf(login).one?.drift?.kind === "changed" } : {}) });
        const { machine, agent, dir } = this.recordOf(login);
        options.add(machine, agent, dir).then(
          (record) => {
            this.took(record);
            const taken = shownLogins.find((one) => one.id === idOf(machine, agent, dir));
            if (taken) this.sheet.open(this.target(taken, true));
          },
          (error: unknown) => toast(error instanceof Error ? error.message : "添加失败"),
        );
      },
      detach: (login) => this.sheet.open(this.target(login, false)),
      bench: (login, open) => this.sheet.open({ ...this.target(login, true), workbench: open }),
      scan: (machine) => {
        forget();
        refreshed(options.scan(machine.id, false)).catch((error: unknown) => toast(error instanceof Error ? error.message : "扫描失败"));
      },
      // This computer's own picker where it has one; any other machine's
      // folders walked as its node lists them, as a picker that cannot
      // open is.
      addFolder: (machine) => {
        const choose = (dir: string) => {
          refreshed(options.folder(machine.id, dir, { change: "choose" })).catch((error: unknown) => toast(error instanceof Error ? error.message : "添加失败"));
        };
        const walk = () => this.choosing.open({ machine: machine.id, folders: (path, cursor) => options.folders(machine.id, path, cursor), chosen: choose });
        if (!options.pick || !machine.here) return walk();
        options.pick().then((dir) => (dir ? choose(dir) : undefined), walk);
      },
      folder: (machine, dir, change) => {
        refreshed(options.folder(machine.id, dir, { change })).catch((error: unknown) => toast(error instanceof Error ? error.message : "操作失败"));
      },
      newTalk: (login) => {
        const { machine, agent, home } = this.recordOf(login);
        options.talkOn(machine, agent, home);
      },
      openTurn: (login, call) => {
        if (call.opens) options.openTurn(login.machine, call.opens.conversation, call.opens.turn);
      },
      reread: (login) => {
        const { machine, agent, dir } = this.recordOf(login);
        forget(login);
        refreshed(options.limits(machine, agent, dir)).then(() => toast("已重新读取额度"), (error: unknown) => toast(error instanceof Error ? error.message : "读取失败"));
      },
      models: (login) => {
        const { machine, dir, one } = this.recordOf(login);
        if (!one) return;
        this.models.open({
          title: options.words.modelsOf(login.who),
          login: one,
          change: async (change) => {
            const record = await options.changeModels(machine, "claude", dir, change);
            this.took(record);
            return record;
          },
        });
      },
      idleOn: async (login, on) => {
        const { machine, agent, dir } = this.recordOf(login);
        // Its conditions as its machine holds them now, not as last read:
        // another device, or another account signed in here before, may
        // have changed them since.
        const record = await options.read(machine);
        const heard = record.machines.find((one) => one.kind === "answered" && one.logins.machine === machine);
        const side = heard?.kind !== "answered" ? null : agent === "claude" ? heard.logins.claude : agent === "codex" ? heard.logins.codex : null;
        const now = side?.listed.find((one) => one.dir === dir)?.idle_compaction;
        return refreshed(options.setIdleCompaction(machine, agent, dir, { ...(now ?? IDLE_DEFAULT), on }));
      },
      idle: (login) => {
        const { machine, agent, dir, one } = this.recordOf(login);
        if (!one || (agent !== "claude" && agent !== "codex")) return;
        this.idleSheet.open({
          title: options.words.idleOf(login.who),
          agent,
          login: one,
          set: async (setting) => {
            const record = await options.setIdleCompaction(machine, agent, dir, setting);
            this.took(record);
            return record;
          },
        });
      },
      autoTitle: (login, on) => {
        const { machine, agent, dir } = this.recordOf(login);
        return refreshed(options.setTitle(machine, agent, dir, on));
      },
      plan: (login, plan) => {
        const { machine, agent, home } = this.recordOf(login);
        // Ultra goes as Max marked so: a hub or node that knows no Ultra
        // keeps it as Max (`fastctx_types::accounts::PlanSeen`).
        return accounts({ change: "plan", machine, agent, ...(home ? { home } : {}), plan: plan === "ultra" ? "max" : plan, ...(plan === "ultra" ? { ultra: true } : {}) } as AccountChange, login);
      },
      expect: async (login, expect) => {
        const { machine, agent, dir } = this.recordOf(login);
        try {
          const record = await options.setWeek(machine, agent, dir, Math.round(expect.base * 1_000_000), expect.swing);
          // A node of before keeps the week and not its reach: said so,
          // rather than taken for kept.
          const side = record.claude.listed.find((one) => one.dir === dir) ?? record.codex.listed.find((one) => one.dir === dir);
          if (side && (side.swing ?? SWING) !== expect.swing) throw new Error("该机器的 Fastctx 版本较旧：预期周额度已保存，波动幅度未生效");
          this.seen.plates.delete(login.id);
          this.seen.expects.delete(login.id);
          forget(login);
          this.took(record);
        } catch (error) {
          throw new Error(tooOld(error) ? "该机器的 Fastctx 版本较旧，暂不支持设置周额度" : error instanceof Error ? error.message : String(error));
        }
      },
      resetUsage: (login, to, id, looked) => reset(login, id, Math.round(to * 10), looked),
      undoReset: (login) => {
        const newest = this.seen.resets.get(login.id)?.find((made) => !made.undone);
        return newest ? reset(login, newest.id, null) : Promise.reject(new Error("没有可撤回的重置"));
      },
      assist: (login, person, limit, until, manages, direct) => {
        // The reader's own share, set by one who manages the login.
        const to = person === ME ? this.managing(login)?.account : person;
        if (!to) return Promise.reject(new Error("无权管理该机器的登录"));
        return accounts(assisted(login, to, limit, until, manages, undefined, direct), login);
      },
      endAssist: (login, person, shown) => {
        const { machine, agent, home } = this.recordOf(login);
        // Ended for good, as the sheet asked: what was born under it is
        // deleted, the assistance as it was shown then.
        return accounts({ change: "assist", machine, to: person, agent, ...(home ? { home } : {}), assisting: false, purge: true, ...(shown.length ? { shown } : {}) }, login);
      },
      assistGrants: (login, person) => {
        const { machine, agent, home } = this.recordOf(login);
        const world = options.store.world.peek();
        const seen = world?.accounts.hubs.flatMap((hub) => hub.assists).find((one) => one.account === person && one.machine === machine && one.agent === agent && sameHome(one.home, home));
        return seen?.grants ?? [];
      },
      line: async (login, line) => {
        const world = options.store.world.peek();
        for (const grant of grantsOf(login)) {
          const { agent, home } = this.recordOf(login);
          const seen = world?.accounts.hubs.flatMap((hub) => hub.assists).find((one) => one.account === grant.person && one.machine === login.machine && one.agent === agent && sameHome(one.home, home));
          await options.change_(this.holder(login), assisted(login, grant.person, grant.limit, seen?.until ? new Date(seen.until) : null, grant.manages ?? false, line));
        }
        await options.store.refresh();
        this.rebuild();
      },
      say: toast,
    };
  }
}

/** Draws `html` into `root` around the element `keep` names, which stays
 *  where it is, its content and attributes set anew: what stands on the way
 *  to it is the same kind of element in both, and only what is beside that
 *  way is drawn again. Whether it could. */
function redrawAround(root: HTMLElement, html: string, keep: string): boolean {
  const old = root.querySelector<HTMLElement>(keep);
  const next = document.createElement("div");
  next.innerHTML = html;
  const fresh = next.querySelector<HTMLElement>(keep);
  if (!old || !fresh) return false;
  const way = (element: HTMLElement, top: HTMLElement) => {
    const steps: HTMLElement[] = [];
    for (let at: HTMLElement | null = element; at && at !== top; at = at.parentElement) steps.unshift(at);
    return steps;
  };
  const was = way(old, root);
  const now = way(fresh, next);
  if (was.length !== now.length || was.some((step, at) => step.tagName !== now[at]!.tagName || step.className !== now[at]!.className)) return false;
  [root, ...was].forEach((parent, at) => {
    const stays = was[at];
    const comes = now[at];
    const from = at === 0 ? next : now[at - 1]!;
    if (!stays || !comes) {
      // The element kept: what it holds, anew.
      parent.replaceChildren(...from.childNodes);
      return;
    }
    for (const child of [...parent.childNodes]) if (child !== stays) child.remove();
    let before = true;
    for (const child of [...from.childNodes]) {
      if (child === comes) {
        before = false;
        continue;
      }
      if (before) parent.insertBefore(child, stays);
      else parent.append(child);
    }
    for (const name of stays.getAttributeNames()) if (!comes.hasAttribute(name)) stays.removeAttribute(name);
    for (const name of comes.getAttributeNames()) stays.setAttribute(name, comes.getAttribute(name)!);
  });
  return true;
}
