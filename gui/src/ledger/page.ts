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
import { LoginModelsSheet } from "../app/login-models-sheet";
import { link, type LoginTab, type Place as AppPlace } from "../app/places";
import type { Page } from "../app/shell";
import { effect } from "../app/signals";
import type { Store } from "../app/store";
import { glyph } from "../conversation/glyphs";
import type { AccountChange, Login as AccountLogin } from "../world/accounts";
import type { ChangePreview, FolderChange, LoginChange, MachineLogins, ModelChange, WorldLogins } from "../world/logins";
import { followState, LOGINS, type Following } from "../world/topics";
import type { Folders } from "../world/focus";
import type { UsageReport } from "../world/usage";
import type { UsageCallsPage, UsageCallsQuery } from "../world/usage-calls";
import { forget, grantsOf, lines, logins as shownLogins, plateOf, setHost, setNet, type Deeds, type Feed, type Login } from "./data";
import { homePage } from "./home";
import { askedOf, callsFrom, daysFrom, idOf, netOf, partsOf, type Seen } from "./live";
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
  setWeek(machine: string, agent: string, dir: string, week: number | null): Promise<MachineLogins>;
  changeModels(machine: string, agent: string, dir: string, change: ModelChange): Promise<MachineLogins>;
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

export class AgentsPage implements Page {
  readonly element = document.createElement("div");
  readonly title = "智能体";
  private readonly main: HTMLElement;
  private readonly seen: Seen = { records: new Map(), plates: new Map() };
  private readonly sheet: AgentChangeSheet;
  private readonly models: LoginModelsSheet;
  private readonly choosing: FolderPicker;
  private readonly people = { models: new Map<string, Set<string>>(), people: new Map<string, Set<string>>() };
  private following: Following | null = null;
  private readonly stop: () => void;
  private resized = 0;
  private reading = false;
  /** The machines whose nodes page their ledger by 100 alone. */
  private readonly legacyCalls = new Set<string>();

  constructor(private readonly options: AgentsPageOptions, place: AppPlace) {
    this.element.className = "page lg-app";
    this.element.innerHTML = `<main class="lg-main"></main>`;
    this.main = this.element.querySelector<HTMLElement>(".lg-main")!;
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
    this.choosing = new FolderPicker(options.words.picker);
    setLinker((to) => link(appPlaceOf(to)));
    setHost({ feed: this.feed(), deeds: this.deeds() });
    drawnBy((how) => this.paint(how));
    setUntils((login, person) => {
      const { machine, agent } = this.recordOf(login);
      const seen = options.store.world.peek()?.accounts.hubs.flatMap((hub) => hub.assists).find((one) => one.machine === machine && one.agent === agent && one.account === person);
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
    window.addEventListener("resize", this.onResize);
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
    this.stop();
    this.following?.();
    window.removeEventListener("resize", this.onResize);
    document.querySelector(".lg-pop")?.remove();
  }

  private readonly onResize = () => {
    window.clearTimeout(this.resized);
    this.resized = window.setTimeout(() => draw(), 120);
  };

  private paint(how: How): void {
    const kept = this.main.querySelector<HTMLElement>(".lg-sheet")?.scrollTop ?? 0;
    this.main.dataset.coming = how;
    this.main.innerHTML = state.place.kind === "login" ? loginPage() : homePage();
    // The sidebar's button, where the sidebar folds away (`shell.ts`).
    this.main.querySelector(".lg-head")?.insertAdjacentHTML("afterbegin", `<button class="icon-button menu-button" type="button" aria-label="${this.options.menuLabel}">${glyph("menu", 20)}</button>`);
    if (how === "quiet") this.main.querySelector<HTMLElement>(".lg-sheet")?.scrollTo({ top: kept });
    fillCharts(this.main, how !== "quiet");
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
    setNet(netOf(this.options.store.world.peek(), this.seen, this.options.now()));
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
      this.rebuild();
    } catch (error) {
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
    const holder = this.recordOf(login).picked?.ownerLogin;
    if (!holder) throw new Error("无权管理该机器的登录");
    return holder;
  }

  private feed(): Feed {
    const { options } = this;
    return {
      days: async (login) => {
        const { machine, agent, dir } = this.recordOf(login);
        const report = await options.usage(machine, agent, dir);
        return daysFrom(report, login, plateOf(login), options.store.world.peek(), options.now());
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
      await options.change_(this.holder(login), change);
      await options.store.refresh();
      this.rebuild();
    };
    const assisted = (login: Login, person: string, limit: number | null, until: Date | null, line = lines[login.id]): AccountChange => {
      const { machine, agent, home } = this.recordOf(login);
      return {
        change: "assist",
        machine,
        to: person,
        agent,
        ...(home ? { home } : {}),
        assisting: true,
        ...(until ? { until: until.toISOString() } : {}),
        // Dollars for a node that knows no shares; a share for one that does.
        ...(limit !== null ? { weekly: Math.round(((plateOf(login) * limit) / 100) * 1_000_000), share: limit } : {}),
        ...(line?.fiveStop ? { five_hour: line.fiveStop } : {}),
        ...(line?.turns ? { turns: line.turns } : {}),
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
      autoTitle: (login, on) => {
        const { machine, agent, dir } = this.recordOf(login);
        return refreshed(options.setTitle(machine, agent, dir, on));
      },
      resetInPlace: (login, on) => {
        const { machine, agent, dir } = this.recordOf(login);
        return refreshed(options.setResetInPlace(machine, agent, dir, on));
      },
      plan: (login, plan) => {
        const { machine, agent, home } = this.recordOf(login);
        // Ultra goes as Max marked so: a hub or node that knows no Ultra
        // keeps it as Max (`fastctx_types::accounts::PlanSeen`).
        return accounts({ change: "plan", machine, agent, ...(home ? { home } : {}), plan: plan === "ultra" ? "max" : plan, ...(plan === "ultra" ? { ultra: true } : {}) } as AccountChange, login);
      },
      plate: async (login, dollars) => {
        const { machine, agent, dir } = this.recordOf(login);
        try {
          this.took(await options.setWeek(machine, agent, dir, Math.round(dollars * 1_000_000)));
        } catch (error) {
          throw new Error(/does not know|unknown variant|older/i.test(String(error)) ? "该机器的 Fastctx 版本较旧，暂不支持设置周额度" : error instanceof Error ? error.message : String(error));
        }
      },
      assist: (login, person, limit, until) => accounts(assisted(login, person, limit, until), login),
      endAssist: (login, person) => {
        const { machine, agent, home } = this.recordOf(login);
        return accounts({ change: "assist", machine, to: person, agent, ...(home ? { home } : {}), assisting: false }, login);
      },
      line: async (login, line) => {
        const world = options.store.world.peek();
        for (const grant of grantsOf(login)) {
          const seen = world?.accounts.hubs.flatMap((hub) => hub.assists).find((one) => one.account === grant.person && one.machine === login.machine);
          await options.change_(this.holder(login), assisted(login, grant.person, grant.limit, seen?.until ? new Date(seen.until) : null, line));
        }
        await options.store.refresh();
        this.rebuild();
      },
      say: toast,
    };
  }
}
