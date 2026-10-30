// One agent's login and what it was used for (`world-agent-logins.md`, 用量;
// `world-gui.md`, 智能体页): across the top what stands now, its windows as
// rings, this week's use and who used it; below, four drawings to choose
// from, a year of days, the week's limit, the login's reset weeks and the
// hours of the week, and beside them its requests one by one (`calls.ts`);
// at the foot what the use went to and on which models.
// Claude Code counts in dollars as well as tokens, Codex in tokens alone.
// Everything here is Fastctx's own bookkeeping on the login's machine:
// opening the page asks that machine's node, never the model's vendor.
//
// Whoever may see the machine sees everyone's use, and of the seven-day
// window both what the account used and each person's part of it; picking
// a person narrows everything below to them. An account the login assists
// sees its own use alone, against the allowances its owner gave it, and
// nothing of the login's account's windows; whose the assistance is and
// until when is a tag in the head.

import { glyph } from "../../conversation/glyphs";
import { escape } from "../../conversation/prose";
import { modelName } from "../../conversation/view";
import type { AssistSeen, HubAccount } from "../../world/accounts";
import type { AgentLimits, Fact, LimitWindow } from "../../world/agents";
import { isReset } from "../agent-homes";
import type { Login, MachineLogins, WorldLogins } from "../../world/logins";
import { wholeDollars } from "../../world/money";
import type { UsageReport } from "../../world/usage";
import type { UsageCallsPage, UsageCallsQuery } from "../../world/usage-calls";
import type { ResidentId } from "../../world/wire";
import type { ActsMenu } from "../acts-menu";
import type { AgentLoginsWords } from "../agent-logins-words";
import { detailOf } from "../problem";
import type { Page } from "../shell";
import { effect, type Signal } from "../signals";
import type { Store } from "../store";
import { CallsList, type CallsContext } from "./calls";
import type { CallsWords } from "./calls-words";
import { bar, calendar, fitInks, meter, parts, punch, quota, weeks, type Amounts, type RingPart } from "./draw";
import { keepRecord, keptRecord, keptReport, readerNow, readReport, reportKey } from "./kept";
import { allowanceTrack, between, byDay, byHourOfWeek, dayOf, DAY_MS, daysBefore, longestRun, pace, total, track, valueOf, windowOf, type Person, type Span, type Unit, type Usage, type Went } from "./model";
import { assistingMe, assistsOf, namer } from "./people";
import { PreparedUsage } from "./prepared";
import { assistingOwner } from "../reach";
import type { UsageWords } from "./words";

type Agent = "codex" | "claude";
type Tab = "calendar" | "quota" | "weeks" | "punch" | "calls";
type Mine = NonNullable<ReturnType<typeof assistingMe>>;
/** The allowances, in dollars, of the person the page looks at. */
type Caps = { weekly: number | null; daily: number | null };

const TABS: readonly Tab[] = ["calendar", "quota", "weeks", "punch", "calls"];

export interface LoginUsageOptions {
  store: Store;
  words: UsageWords;
  logins: AgentLoginsWords;
  head(title: string, actions?: string): string;
  machine: string;
  agent: Agent;
  dir: string;
  workbench: Signal<boolean>;
  read(machine: string): Promise<WorldLogins>;
  usage(machine: string, agent: Agent, dir: string): Promise<UsageReport>;
  /** A past moment as the pages say it. */
  when(at: Date, now: Date): string;
  /** Begins a conversation in this login. */
  talk(machine: string, agent: Agent, home: string | null): void;
  /** The login's requests one by one; opened on one turn's, when a
   *  conversation asked for them, until that is let go. */
  calls: {
    words: CallsWords;
    menu: ActsMenu;
    ask(machine: string, agent: Agent, dir: string, query: UsageCallsQuery): Promise<UsageCallsPage>;
    open(machine: string, conversation: ResidentId, turn: string | null): void;
    turn: { conversation: ResidentId; turn: string } | null;
    unturn(): void;
  };
}

const KEPT = "usage.view";
/** While the page is open, the use is read again this often: a turn
 *  running elsewhere adds to it. It is the machine's own ledger, read from
 *  its node; no model is asked. */
const AGAIN_MS = 60_000;
/** The narrowest a reset week's bar is drawn: the rest are the oldest. */
const WEEK_SLOT = 26;

export class LoginUsagePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;
  private readonly resize: ResizeObserver;
  private readonly key: string;
  private record: MachineLogins | null;
  private report: UsageReport | null;
  private readonly prepared = new PreparedUsage();
  private problem: string | null = null;
  private again = 0;
  private left = false;
  /** Whether the use was asked for since the page opened. */
  private asked = false;
  private view: { tab: Tab; unit: Unit } = kept();
  /** The person everything below is narrowed to, by key. */
  private only: string | null = null;
  /** Every one who used it listed, not the first few and the rest folded. */
  private everyone = false;
  private width = 0;
  private height = 0;

  private drawn = "";
  /** The requests' panel, kept across drawings once it is first shown. */
  private calls: CallsList | null = null;

  constructor(private readonly options: LoginUsageOptions) {
    this.key = reportKey(options.machine, options.agent, options.dir);
    // A turn's requests asked for open on them, and are not kept as the view.
    if (options.calls.turn) this.view = { ...this.view, tab: "calls" };
    this.record = keptRecord(options.machine);
    this.report = keptReport(this.key)?.report ?? null;
    this.title = this.who();
    this.element = document.createElement("div");
    this.element.className = "ledger-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title ledger-title"></h1>`, `<button class="text-button ledger-talk" type="button">${glyph("plus", 16)}<span>${escape(options.words.newTalk)}</span></button>`)}<div class="ledger-body scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".ledger-body")!;
    this.element.querySelector(".ledger-talk")!.addEventListener("click", () => {
      const login = this.login();
      options.talk(options.machine, options.agent, login && !login.default ? login.dir : login ? null : options.dir);
    });
    this.body.addEventListener("click", (event) => this.click(event));
    this.resize = new ResizeObserver(() => {
      const figure = this.body.querySelector<HTMLElement>(".ledger-figure");
      if (figure && (Math.abs(figure.clientWidth - this.width) >= 1 || Math.abs(figure.clientHeight - this.height) >= 1)) this.draw();
      fitInks(this.body);
    });
    this.resize.observe(this.body);
    this.stop = effect(() => {
      options.store.world.get();
      // The workbench turned on while the page is open reads the use then.
      if (options.workbench.get() && !this.asked) void this.read();
      this.draw();
    });
  }

  leave(): void {
    this.left = true;
    this.stop();
    this.resize.disconnect();
    clearTimeout(this.again);
    this.calls?.leave();
  }

  private async read(): Promise<void> {
    const { options } = this;
    clearTimeout(this.again);
    if (!options.workbench.peek()) return;
    this.asked = true;
    const asked = readerNow();
    const record = options.read(options.machine).then((world) => {
      const heard = world.machines.find((one) => one.kind === "answered" && one.logins.machine === options.machine);
      if (heard?.kind === "answered") {
        this.record = heard.logins;
        keepRecord(heard.logins, asked);
      }
    });
    const report = readReport(this.key, () => options.usage(options.machine, options.agent, options.dir)).then((report) => {
      this.report = report;
      this.problem = null;
    });
    const [, used] = await Promise.allSettled([record, report]);
    // Left while it read: nothing more is drawn or asked.
    if (this.left) return;
    if (used.status === "rejected") this.problem = detailOf(used.reason);
    this.draw();
    if (this.calls?.element.isConnected) void this.calls.again();
    this.again = window.setTimeout(() => void this.read(), AGAIN_MS);
  }

  private login(): Login | null {
    return this.record?.[this.options.agent].listed.find((one) => one.dir === this.options.dir) ?? null;
  }

  private who(): string {
    const login = this.login();
    const identity = login?.identity?.who.kind === "known" ? login.identity.who.value : null;
    return identity?.email ?? dirName(this.options.dir);
  }

  /** A person picked narrows everything below to them; picked again, or
   *  another picked, it widens or moves. */
  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-view], [data-only], [data-everyone]");
    if (!target) return;
    if (target.dataset.everyone !== undefined) this.everyone = target.dataset.everyone === "1";
    else if (target.dataset.only !== undefined) this.only = this.only === target.dataset.only ? null : target.dataset.only;
    else {
      const [what, value] = target.dataset.view!.split(":") as [keyof LoginUsagePage["view"], string];
      this.view = { ...this.view, [what]: value };
      keep(this.view);
    }
    this.draw();
  }

  private draw(): void {
    const { options } = this;
    const { words } = options;
    const world = options.store.world.peek();
    const now = options.store.clock.peek().getTime();
    const login = this.login();
    this.head(login);
    let html: string;
    let context: CallsContext | null = null;
    let panel: { html: string; draw(): string } | null = null;
    if (!options.workbench.peek()) {
      html = `<p class="feature-quiet">${escape(words.workbenchOff)}</p>`;
    } else if (!this.report) {
      html = this.problem ? `<p class="feature-note" title="${escape(this.problem)}">${escape(words.failed)}</p>` : `<p class="feature-quiet ledger-reading">${escape(words.reading)}</p>`;
    } else {
      const usage = this.prepared.get(this.report, namer(world, options.machine, words));
      const mine = assistingMe(world, options.machine, options.agent, options.dir, login?.default ?? false);
      // The one looked at: the reader where the login assists them, else
      // the one picked.
      const only = mine ? null : (usage.people.find((person) => person.key === this.only) ?? null);
      const looked = mine ? (usage.people.find((person) => person.me) ?? null) : only;
      const caps = this.capsOf(mine?.assist ?? (only ? this.assistOf(only) : null));
      const unit: Unit = usage.usd ? this.view.unit : "tokens";
      const amounts = amountsOf(words, unit);
      const tab = this.tab(mine, caps);
      const figure = this.body.querySelector<HTMLElement>(".ledger-figure");
      this.width = figure?.clientWidth || this.body.clientWidth - 2 * 32 || 760;
      this.height = figure?.clientHeight || 280;
      const went: Went = only?.went ?? usage.week;
      // The requests take the drawing's room, and, beside the drawings,
      // the room of the week's cards at its foot.
      const calls = tab === "calls";
      if (calls) context = this.callsContext(usage, mine, only, now);
      const foot = calls && !mine ? "" : `${this.where(went, unit)}${this.models(went, unit, amounts, only?.ink)}`;
      const drawPanel = () => `<section class="ledger-panel">${this.narrow() ? this.panel(usage, tab, only, looked, caps, unit, amounts, now).replace('<div class="ledger-panel-head">', () => `<div class="ledger-panel-head">${this.unitSwitch(usage, tab)}`) : this.panel(usage, tab, only, looked, caps, unit, amounts, now)}</section>`;
      const panelHtml = calls ? `<section class="ledger-panel" data-calls></section>` : drawPanel();
      if (!calls) panel = { html: panelHtml, draw: drawPanel };
      html = `<div class="ledger-sheet${mine ? " alone" : ""}${calls ? " calls" : ""}">${this.strip(usage, login?.limits ?? null, unit, amounts, mine, only, caps, now)}
        ${this.tabs(usage, tab, mine, caps)}
        ${panelHtml}
        ${foot}
        ${this.problem ? `<p class="feature-note ledger-problem" title="${escape(this.problem)}">${escape(words.failed)}</p>` : ""}</div>`;
    }
    if (html === this.drawn) {
      if (context) this.calls?.placed(context);
      return;
    }
    this.drawn = html;
    const top = this.body.scrollTop;
    this.body.innerHTML = html;
    if (context) {
      this.calls ??= new CallsList({
        words: options.calls.words,
        usage: words,
        claude: options.agent === "claude",
        menu: options.calls.menu,
        ask: (query) => options.calls.ask(options.machine, options.agent, options.dir, query),
        open: (conversation, turn) => options.calls.open(options.machine, conversation, turn),
        turn: options.calls.turn,
        unturn: () => options.calls.unturn(),
      });
      this.body.querySelector("[data-calls]")?.replaceWith(this.calls.element);
      this.calls.placed(context);
    }
    this.body.scrollTop = top;
    fitInks(this.body);
    // The figure's size is known once it is laid out: a first drawing at a
    // guess is drawn again, once, at the size it has.
    const figure = this.body.querySelector<HTMLElement>(".ledger-figure");
    if (panel && figure && (Math.abs(figure.clientWidth - this.width) >= 1 || Math.abs(figure.clientHeight - this.height) >= 1)) {
      this.width = figure.clientWidth;
      this.height = figure.clientHeight;
      const fitted = panel.draw();
      this.body.querySelector(".ledger-panel")!.outerHTML = fitted;
      // A function, so the `$` of the amounts in it is never read as a pattern.
      this.drawn = html.replace(panel.html, () => fitted);
    }
  }

  /** The head: the login's mark, who it is and its tags, and the machine. */
  private head(login: Login | null): void {
    const { options } = this;
    const identity = login?.identity?.who.kind === "known" ? login.identity.who.value : null;
    const token = identity?.method === "oauth_token";
    const plan = identity?.plan ?? null;
    const title = this.element.querySelector<HTMLElement>(".ledger-title")!;
    const html = `${glyph(options.agent, 18)}<span class="ledger-who" title="${escape(options.dir)}">${escape(this.who())}</span>${plan ? `<span class="tag">${escape(planName(plan))}</span>` : ""}${token ? `<span class="tag tag-token" title="${escape(options.logins.tokenSaid)}">${escape(options.logins.token)}</span>` : ""}${assistingOwner(options.store.world.peek(), options.machine) === null ? `<span class="ledger-machine">${glyph("machine", 14)}${escape(options.machine)}</span>` : ""}${this.assistedTag(login)}`;
    if (title.innerHTML !== html) title.innerHTML = html;
  }

  /** To an account the login assists: whose the assistance is and until
   *  when, its terms a hover away, as the agents page tags the login. */
  private assistedTag(login: Login | null): string {
    const { options } = this;
    const { words } = options;
    const mine = assistingMe(options.store.world.peek(), options.machine, options.agent, options.dir, login?.default ?? false);
    if (!mine) return "";
    const { assist, owner } = mine;
    const now = options.store.clock.peek();
    const until = assist.until ? new Date(assist.until) : null;
    const said = [
      words.assistUntil(until, now),
      assist.daily ? words.daily(wholeDollars(assist.daily / 1_000_000)) : "",
      assist.weekly ? words.weekly(wholeDollars(assist.weekly / 1_000_000)) : "",
      assist.daily || assist.weekly ? "" : words.noCap,
      assist.five_hour ? words.fiveHourCap(assist.five_hour) : "",
      assist.turns ? words.turnsCap(assist.turns) : "",
    ]
      .filter(Boolean)
      .join("\n");
    return `<span class="tag ledger-from" title="${escape(said)}">${escape(options.logins.assistedFrom(owner ?? "–"))}${until ? `<span class="ledger-from-until">${glyph("history", 12)}${escape(words.untilShort(until, now))}</span>` : ""}</span>`;
  }

  /** The assistance of the login a person uses it by, as its owner sees it. */
  private assistOf(person: Person): AssistSeen | null {
    const world = this.options.store.world.peek();
    const login = this.login();
    const assists = assistsOf(world, this.options.machine, this.options.agent, this.options.dir, login?.default ?? false);
    return assists.find(({ hub, assist }) => person.key === `${hub}:${assist.account}`)?.assist ?? null;
  }

  /** An assistance's allowances in dollars: only Claude Code keeps any. */
  private capsOf(assist: AssistSeen | null): Caps | null {
    if (!assist || this.options.agent !== "claude" || (!assist.weekly && !assist.daily)) return null;
    return { weekly: assist.weekly ? assist.weekly / 1_000_000 : null, daily: assist.daily ? assist.daily / 1_000_000 : null };
  }

  /** The drawing shown: the limit is the account's, which a reader the
   *  login assists does not see, so for them it is their weekly allowance,
   *  and without one there is none to draw. */
  private tab(mine: Mine | null, caps: Caps | null): Tab {
    return this.view.tab === "quota" && mine && !caps?.weekly ? "calendar" : this.view.tab;
  }

  /** What stands now, one table of rows in shared columns: this week's
   *  figure, the account's windows as bars, under the seven-day one a row
   *  for each who used its week, pressed to narrow the page to them; the
   *  one picked has their allowances under their row, the two one block. */
  private strip(usage: Usage, limits: Fact<AgentLimits> | null, unit: Unit, amounts: Amounts, mine: Mine | null, only: Person | null, caps: Caps | null, now: number): string {
    const { words } = this.options;
    const windows = mine ? "" : this.windowMeters(usage, limits, only, now);
    const allowances = mine ? this.allowanceMeters(usage, caps) : "";
    // The figure is always the whole login's week (the reader's own where it
    // assists them); each one's share is on their row.
    const shown = usage.people;
    const week = shown.reduce((sum, person) => sum + valueOf(person.week, unit), 0);
    const weekTokens = shown.reduce((sum, person) => sum + person.week.tokens, 0);
    const { start } = usage.week;
    const elapsed = now - start;
    const before = total(between(usage, unit, start - 7 * DAY_MS, start - 7 * DAY_MS + elapsed, null));
    const sofar = total(between(usage, unit, start, now + 1, null));
    const today = total(between(usage, unit, dayOf(now), now + 1, null));
    const change = before > 0 ? Math.round((sofar / before - 1) * 100) : null;
    // The week's figure is the first row of the same columns as the bars:
    // whose week, the amount, and its change from last week where the bars
    // have their notes; how it is reckoned, its tokens and today a hover away.
    const number = unit === "usd" ? escape(words.money(week)) : `${escape(words.tokens(week))}<span class="ledger-unit">${escape(words.tokenUnit)}</span>`;
    const said = [unit === "usd" ? words.aboutWhy : "", unit === "usd" ? `${words.tokens(weekTokens)} ${words.tokenUnit}` : "", words.today(today > 0 ? amounts.say(today) : null)].filter(Boolean).join("\n");
    const delta = change === null ? "" : `${change > 0 ? "+" : change < 0 ? "−" : "±"}${Math.abs(change)}%`;
    const deltaSaid = change === null ? (sofar > 0 ? words.noLastWeek : "") : words.vsLastWeek(change);
    const span = `${words.weekSpan(new Date(start), new Date(usage.week.end))}\n${words.weekWhy(usage.week.window)}`;
    const figure = `<div class="ledger-meter ledger-week">
        <span class="ledger-meter-name" title="${escape(span)}">${escape(words.thisWeek)}</span>
        <span class="ledger-week-big" title="${escape(said)}">${number}</span>
        <span class="ledger-meter-used"></span>
        <span class="ledger-meter-note${change !== null && change > 0 ? " up" : ""}"${deltaSaid ? ` title="${escape(deltaSaid)}"` : ""}>${escape(delta)}</span>
      </div>`;
    // An account the login assists reads its week in its allowances' bars;
    // without any, in the week's figure.
    const first = mine && allowances ? "" : figure;
    const people = mine ? "" : this.peopleRows(usage, unit, amounts, caps);
    return `<section class="ledger-now"><div class="ledger-meters">${first}${windows}${people}${allowances}</div></section>`;
  }

  /** The login's windows as bars; the seven-day one in the parts of those
   *  who used its week, the one picked in full and the rest faded. */
  private windowMeters(usage: Usage, limits: Fact<AgentLimits> | null, only: Person | null, now: number): string {
    const { words } = this.options;
    const known = limits?.kind === "known" ? limits.value : null;
    const week = current(usage);
    const rows = [300, 10080].map((minutes) => {
      const window = known ? windowOf(known.windows, minutes) : null;
      // A reading without this window: the account has none.
      if (known && !window) return "";
      if (minutes === 300 || !week?.parts.size) return this.windowMeter(minutes, window, known?.observed_at ?? null, now, [], []);
      const users = usage.people.filter((person) => week.parts.has(person.index));
      const ringParts: RingPart[] = users.map((person) => ({ share: week.parts.get(person.index)!, ink: person.ink, faded: !!only && only.index !== person.index }));
      const said = users.map((person) => words.partOf(person.name, Math.round(week.parts.get(person.index)! * 100)));
      return this.windowMeter(minutes, window, known?.observed_at ?? null, now, ringParts, said);
    });
    return rows.join("");
  }

  /** One window's bar: how much is used and how long until it resets, a
   *  pencil mark where the window stands now; a window past the reset it
   *  was read before is empty and says it has reset; one never read is a
   *  dashed empty bar. When it was read, what it had used before a reset,
   *  and each person's part, a hover away. */
  private windowMeter(minutes: number, window: LimitWindow | null, observed: string | null, now: number, ringParts: RingPart[], said: string[]): string {
    const { words, logins } = this.options;
    const name = words.window(minutes, window?.name ?? "");
    const read = observed ? logins.observed(this.options.when(new Date(observed), new Date(now))) : "";
    const percent = window ? `${Math.round(window.used_tenths / 10)}%` : "";
    const reset = window ? isReset(window, now) : false;
    const share = window && !reset ? window.used_tenths / 1000 : null;
    const left = window?.resets_at && !reset ? logins.resetsIn(new Date(window.resets_at), new Date(now)) : "";
    const parted = share !== null && said.length ? [...said, words.partsWhy] : [];
    const hover = !window
      ? `${name}: ${logins.notRead(this.options.agent)}`
      : reset
        ? [`${name}: ${logins.reset}`, logins.usedBefore(percent), read].join("\n")
        : [`${name}: ${logins.used(percent)}`, window.resets_at ? logins.resets(new Date(window.resets_at), new Date(now)) : "", read, ...parted].filter(Boolean).join("\n");
    const note = !window ? escape(words.notRead) : reset ? escape(logins.reset) : left ? `${glyph("history", 13)}<span>${escape(left)}</span>` : "";
    return this.meterRow({ name, share, parts: share === null ? [] : ringParts, note, said: hover, seed: `${this.key}:${minutes}`, unread: !window });
  }

  /** What one assisted has spent against their allowances: a day's, a
   *  week's and Fable's, half the week's (`world-usage.md`, 协助的上限).
   *  The one their own row already shows is `shown`. */
  private capMeters(usage: Usage, person: Person | null, caps: Caps, ink: string | null, shown: "weekly" | "daily" | null = null): string {
    const { words } = this.options;
    const seed = `${this.key}:cap:${person?.key ?? ""}`;
    const parts = (share: number): RingPart[] => (ink ? [{ share, ink }] : []);
    const row = (name: string, spent: number, cap: number, said: string, suffix: string) =>
      this.meterRow({ name, share: spent / cap, parts: parts(spent / cap), note: escape(words.capOf(words.money(spent), wholeDollars(cap))), said, seed: seed + suffix, unread: false });
    const rows: string[] = [];
    if (caps.daily && shown !== "daily") {
      const spent = person?.day?.usd ?? 0;
      rows.push(row(words.dayCap, spent, caps.daily, words.dayCapSaid(wholeDollars(caps.daily), words.money(spent)), ":day"));
    }
    if (caps.weekly) {
      const spent = person?.week.usd ?? 0;
      if (shown !== "weekly") rows.push(row(words.weekCap, spent, caps.weekly, [words.weekCapSaid(wholeDollars(caps.weekly), words.money(spent)), words.weekSpan(new Date(usage.week.start), new Date(usage.week.end))].join("\n"), ":week"));
      if (person?.fable?.usd != null) rows.push(row(words.fable, person.fable.usd, caps.weekly / 2, words.fableCapSaid(wholeDollars(caps.weekly / 2), words.money(person.fable.usd)), ":fable"));
    }
    return rows.join("");
  }

  /** The allowances the reader has of the login, as bars. */
  private allowanceMeters(usage: Usage, caps: Caps | null): string {
    if (!caps) return "";
    return this.capMeters(usage, usage.people.find((person) => person.me) ?? null, caps, null);
  }

  /** One limit's row in the shared columns: its name, the bar, how much
   *  is used, and a short note. */
  private meterRow(row: { name: string; share: number | null; parts: RingPart[]; note: string; said: string; seed: string; unread: boolean }): string {
    const { share } = row;
    const level = share === null ? "" : share >= 1 ? " spent" : share >= 0.9 ? " high" : "";
    return `<div class="ledger-meter${level}${row.unread ? " unread" : ""}" title="${escape(row.said)}">
      <span class="ledger-meter-name">${escape(row.name)}</span>
      <span class="ledger-meter-track">${meter(share, row.seed, row.parts)}</span>
      <span class="ledger-meter-used">${share === null ? "" : `${Math.round(share * 100)}%`}</span>
      <span class="ledger-meter-note">${row.note}</span>
    </div>`;
  }

  /** Who used it this week, a row each under the seven-day bar: their bar
   *  in their ink, and its figure, say how much of what they may use they
   *  used: their own allowance where they have one, the week's where they
   *  have one, else the day's, with what they spent of it beside; else the
   *  login's seven days, with what they spent. Each one's part of the
   *  whole login is the seven-day bar's above; it, their terms and when an
   *  assistance ends are a hover away. The one picked has their other
   *  allowances under their row, the block pressed anywhere to close it.
   *  One alone has nothing to tell apart: the seven-day bar is theirs. */
  private peopleRows(usage: Usage, unit: Unit, amounts: Amounts, caps: Caps | null): string {
    const { words } = this.options;
    if (usage.people.length < 2) return "";
    const week = current(usage);
    const parted = week?.reached !== null && week?.reached !== undefined;
    const clock = this.options.store.clock.peek();
    // Past five, the first four by use and the one picked are rows, the rest
    // one row together until unfolded.
    const folds = usage.people.length > MOST_ROWS;
    const listed = !folds || this.everyone ? usage.people : usage.people.filter((person, index) => index < MOST_ROWS - 1 || person.key === this.only);
    const rest = usage.people.filter((person) => !listed.includes(person));
    const rows = listed
      .map((person) => {
        const assisted = this.assistOf(person);
        const terms = assisted ? [words.assistUntil(assisted.until ? new Date(assisted.until) : null, clock), assisted.daily ? words.daily(wholeDollars(assisted.daily / 1_000_000)) : "", assisted.weekly ? words.weekly(wholeDollars(assisted.weekly / 1_000_000)) : "", assisted.five_hour ? words.fiveHourCap(assisted.five_hour) : "", assisted.turns ? words.turnsCap(assisted.turns) : ""].filter(Boolean) : [];
        const value = valueOf(person.week, unit);
        const part = parted ? (week!.parts.get(person.index) ?? 0) : null;
        const picked = this.only === person.key;
        const said = [words.spentWeek(person.name, value ? amounts.say(value) : "–"), part === null ? "" : words.partSaid(person.name, Math.round(part * 100)), ...terms].filter(Boolean).join("\n");
        const own = this.capsOf(assisted);
        const shown = own?.weekly ? "weekly" : own?.daily ? "daily" : null;
        const cap = shown === "weekly" ? { cap: own!.weekly!, spent: person.week.usd ?? 0 } : shown === "daily" ? { cap: own!.daily!, spent: person.day?.usd ?? 0 } : null;
        // Bar, figure and note say one thing: how much of what they may use
        // they used, their own allowance where they have one, in caution as
        // it runs out, else the login's seven days, as long as their part
        // of the bar above.
        const spent = cap ? cap.spent / cap.cap : null;
        const level = spent === null ? "" : spent >= 1 ? " spent" : spent >= 0.9 ? " high" : "";
        const used = spent ?? part;
        const ends = assisted?.until ? glyph("history", 12) : "";
        const row = `<button class="ledger-meter ledger-person${picked ? " on" : ""}" type="button" data-only="${escape(person.key)}" title="${escape(said)}">
          <span class="ledger-meter-name ledger-person-name"><span>${escape(person.name)}</span>${ends}</span>
          <span class="ledger-meter-track">${used === null ? "" : meter(Math.min(used, 1), `${this.key}:person:${person.key}`, [{ share: Math.min(used, 1), ink: person.ink }], false)}</span>
          <span class="ledger-meter-used">${used === null ? "" : `${Math.round(used * 100)}%`}</span>
          <span class="ledger-meter-note${level}">${escape(cap ? words.capOf(words.money(cap.spent), wholeDollars(cap.cap)) : value ? amounts.say(value) : "–")}</span>
        </button>`;
        const more = picked && caps ? this.capMeters(usage, person, caps, person.ink, shown) : "";
        return more ? `<div class="ledger-group" data-only="${escape(person.key)}">${row}${more}</div>` : row;
      })
      .join("");
    if (rest.length) {
      // Together they have no one allowance: what they spent, each a hover away.
      const value = rest.reduce((sum, person) => sum + valueOf(person.week, unit), 0);
      const said = rest.map((person) => words.spentWeek(person.name, amounts.say(valueOf(person.week, unit)))).join("\n");
      // Their parts of the seven days together, in graphite: no one's ink.
      const part = parted ? rest.reduce((sum, person) => sum + (week!.parts.get(person.index) ?? 0), 0) : null;
      return `${rows}<button class="ledger-meter ledger-person ledger-others" type="button" data-everyone="1" title="${escape(said)}">
          <span class="ledger-meter-name ledger-person-name"><span>${escape(words.others(rest.length))}</span></span>
          <span class="ledger-meter-track">${part === null ? "" : meter(part, `${this.key}:others`, [{ share: part, ink: "color-mix(in srgb, var(--pen-graphite) 70%, var(--bg))" }], false)}</span>
          <span class="ledger-meter-used">${part === null ? "" : `${Math.round(part * 100)}%`}</span>
          <span class="ledger-meter-note">${escape(amounts.say(value))}</span>
        </button>`;
    }
    return folds ? `${rows}<button class="ledger-fewer" type="button" data-everyone="0">${escape(words.fewer)}</button>` : rows;
  }

  /** The drawings' tabs, and beside them the unit, except on a phone,
   *  where the tabs take the row and the unit goes to the drawing's head
   *  (`unitSwitch`). */
  private tabs(usage: Usage, current_: Tab, mine: Mine | null, caps: Caps | null): string {
    const { words } = this.options;
    const shown = TABS.filter((tab) => tab !== "quota" || !mine || caps?.weekly);
    const tab = (value: Tab) => `<button type="button" role="tab" class="${current_ === value ? "current" : ""}" aria-selected="${current_ === value}" data-view="tab:${value}">${escape(words.tabs[value])}</button>`;
    return `<div class="ledger-tabs"><div class="ledger-switch ledger-tablist" role="tablist">${shown.map(tab).join("")}</div>${this.narrow() ? "" : this.unitSwitch(usage, current_)}</div>`;
  }

  /** Dollars or tokens, where the drawing shown is in either. */
  private unitSwitch(usage: Usage, tab: Tab): string {
    const { words } = this.options;
    return usage.usd && tab !== "quota" && tab !== "calls"
      ? `<div class="ledger-switch ledger-units" role="group">${(["usd", "tokens"] as const).map((value) => `<button type="button" class="${this.view.unit === value ? "current" : ""}" aria-pressed="${this.view.unit === value}" data-view="unit:${value}">${escape(value === "usd" ? words.usd : words.tokenTab)}</button>`).join("")}</div>`
      : "";
  }

  /** Whether the page is as narrow as a phone's (the sheet's own break). */
  private narrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width <= 600;
  }

  /** The drawing chosen, at the width it has, narrowed to the one picked. */
  private panel(usage: Usage, tab: Tab, only: Person | null, looked: Person | null, caps: Caps | null, unit: Unit, amounts: Amounts, now: number): string {
    const { words } = this.options;
    const width = Math.max(280, this.width);
    const index = only?.index ?? null;
    const shown = only ? [only] : usage.people;
    const ink = only?.ink ?? "var(--pen-clay)";
    const seed = `${this.key}:${unit}:${index ?? ""}`;
    const today = dayOf(now);
    switch (tab) {
      case "calendar": {
        const days = byDay(usage, unit, index);
        const drawn = calendar({ width, height: this.height, days, people: shown, today, ink, amounts, words, seed });
        const year = [...days.entries()].filter(([day]) => day >= daysBefore(today, 364));
        const sum = year.reduce((all, [, values]) => all + total(values), 0);
        const busiest = year.reduce<[number, number] | null>((best, [day, values]) => (!best || total(values) > best[1] ? [day, total(values)] : best), null);
        const legend = `<span class="ledger-legend">${escape(words.less)}${[0, 0.34, 0.56, 0.78, 1].map((share) => `<i style="background:${share ? `color-mix(in srgb, ${ink} ${share * 100}%, transparent)` : "var(--fill-strong)"}"></i>`).join("")}${escape(words.most)}</span>`;
        const facts = sum
          ? [words.yearTotal(amounts.say(sum)), words.daysUsed(year.length), words.longestRun(longestRun(days, today, 365)), busiest ? words.busiest(words.day(new Date(busiest[0])).split(" ")[0]!, amounts.say(busiest[1])) : ""]
          : [];
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(drawn.weeks >= 53 ? words.year : words.recentWeeks(drawn.weeks))}</span><span class="ledger-grow"></span>${legend}</div>
          <div class="ledger-figure">${drawn.svg}</div>
          ${facts.length ? `<div class="ledger-facts-row">${facts.filter(Boolean).map((fact) => `<span class="ledger-fact">${escape(fact)}</span>`).join("")}</div>` : ""}`;
      }
      case "quota": {
        const height = Math.max(200, this.height);
        // One with a weekly allowance: what they spent of it through the week.
        if (looked && caps?.weekly && usage.usd) {
          const cap = caps.weekly;
          const spent = allowanceTrack(usage, looked.index, cap, now);
          const drawn = quota({ width, height, start: usage.week.start, end: usage.week.end, now, window: usage.week.window, week: spent, five: null, pace: spent ? pace(spent, now) : null, words, seed: `${this.key}:cap:${looked.key}`, ink: looked.ink, at: (at, percent) => words.capAt(at, percent ?? 0, words.money(((percent ?? 0) / 100) * cap)) });
          return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.capTitle(wholeDollars(cap)))}</span>${drawn.said ? `<span class="ledger-panel-said">${escape(drawn.said)}</span>` : ""}</div><div class="ledger-figure">${drawn.svg}</div>`;
        }
        const week = track(usage, 10080);
        const five = track(usage, 300);
        if (!week && !five) return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.quotaTitle)}</span></div><div class="ledger-figure ledger-empty-figure"><p class="feature-quiet">${escape(words.quotaUnread)}</p></div>`;
        const drawn = quota({ width, height, start: usage.week.start, end: usage.week.end, now, window: usage.week.window, week, five, pace: week ? pace(week, now) : null, words, seed: this.key });
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.quotaTitle)}</span>${drawn.said ? `<span class="ledger-panel-said">${escape(drawn.said)}</span>` : ""}</div><div class="ledger-figure">${drawn.svg}</div>`;
      }
      case "weeks": {
        if (!usage.weeks.length) return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.tabs.weeks)}</span></div><div class="ledger-figure ledger-empty-figure"><p class="feature-quiet">${escape(words.weeksNone)}</p></div>`;
        const fit = Math.max(1, Math.floor((width - 52) / WEEK_SLOT));
        const columns = usage.weeks.slice(-fit).map((span) => weekColumn(span, unit, index));
        const legend = shown.length > 1 ? `<span class="ledger-legend">${shown.map((person) => `<span class="ledger-keyed"><i style="background:${person.ink}"></i>${escape(person.name)}</span>`).join("")}</span>` : "";
        const cap = unit === "usd" && caps?.weekly ? { value: caps.weekly, said: wholeDollars(caps.weekly) } : null;
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.weeksTitle(columns.length))}</span><span class="ledger-grow"></span>${legend}</div><div class="ledger-figure">${weeks({ width, height: Math.max(200, this.height), columns, people: shown, amounts, words, seed, cap, only: only ? { index: only.index, ink: only.ink } : null })}</div>`;
      }
      case "punch": {
        const grid = byHourOfWeek(usage, unit, now, 90, index);
        const all = grid.flat().reduce((sum, value) => sum + value, 0);
        const night = grid.reduce((sum, row) => sum + row.slice(0, 6).reduce((a, b) => a + b, 0), 0);
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.punchTitle)}</span>${all ? `<span class="ledger-panel-said">${escape(words.punchSaid(Math.round((night / all) * 100)))}</span>` : ""}</div><div class="ledger-figure">${punch({ width, height: Math.max(200, this.height), grid, ink, amounts, words, seed })}</div>`;
      }
      case "calls":
        return "";
    }
  }

  /** Whose the requests are, named and inked as the rest of the page
   *  draws them, and the one it is narrowed to. */
  private callsContext(usage: Usage, mine: Mine | null, only: Person | null, now: number): CallsContext {
    const { options } = this;
    const name = namer(options.store.world.peek(), options.machine, options.words);
    const inks = new Map(usage.people.map((person) => [person.key, person.ink]));
    const who = (account: HubAccount | null) => {
      const named = name(account);
      return { key: named.key, name: named.name, ink: inks.get(named.key) ?? "var(--pen-graphite)" };
    };
    // Where the page counts the machine's own sessions as its owner's, the
    // machine cannot narrow to both at once: the rows are narrowed here.
    const local = !!only && name(null).key === only.key;
    const person = mine || !only ? undefined : { key: only.key, account: { account: accountOf(only.key), ...(local ? { with_machine: true } : {}) }, local };
    return { who, several: !mine && usage.people.length > 1, person, models: usage.week.models.map((one) => one.model), now };
  }

  /** What the week's use went to: by what it cost, or by tokens. */
  private where(went: Went, unit: Unit): string {
    const { words } = this.options;
    const { tokens, kinds } = went;
    const claude = this.options.agent === "claude";
    let shares: { value: number; ink: string; label: string; said: string }[];
    let note: string;
    if (claude && unit === "usd" && kinds) {
      const sum = kinds.input + kinds.cache_read + kinds.cache_write + kinds.output || 1;
      const percent = (value: number) => `${Math.round((value / sum) * 100)}%`;
      shares = [
        { value: kinds.cache_read, ink: "var(--pen-graphite)", label: words.kinds.cache_read, said: words.money(kinds.cache_read) },
        { value: kinds.cache_write, ink: "var(--pen-sky)", label: words.kinds.cache_write, said: words.money(kinds.cache_write) },
        { value: kinds.input, ink: "var(--pen-sage)", label: words.kinds.input, said: words.money(kinds.input) },
        { value: kinds.output, ink: "var(--pen-clay)", label: words.kinds.output, said: words.money(kinds.output) },
      ].map((one) => ({ ...one, said: `${one.label} ${one.said}`, label: `${one.label} ${percent(one.value)}` }));
      note = words.byMoney;
    } else {
      const input = tokens.input + tokens.cache_read + tokens.cache_write;
      shares = claude
        ? [
            { value: tokens.cache_read, ink: "var(--pen-graphite)", label: words.kinds.cache_read, said: "" },
            { value: tokens.cache_write, ink: "var(--pen-sky)", label: words.kinds.cache_write, said: "" },
            { value: tokens.input, ink: "var(--pen-sage)", label: words.kinds.input, said: "" },
            { value: tokens.output + tokens.reasoning, ink: "var(--pen-clay)", label: words.kinds.output, said: "" },
          ]
        : [
            { value: tokens.input, ink: "var(--pen-sage)", label: words.kinds.fresh, said: "" },
            { value: tokens.cache_read, ink: "var(--pen-graphite)", label: words.kinds.cached, said: "" },
            { value: tokens.output, ink: "var(--pen-clay)", label: words.kinds.output, said: "" },
            { value: tokens.reasoning, ink: "var(--pen-rose)", label: words.kinds.reasoning, said: "" },
          ];
      shares = shares.map((one) => ({ ...one, said: `${one.label} ${words.tokens(one.value)}`, label: `${one.label} ${words.tokens(one.value)}` }));
      note = input ? words.fromCache(Math.round((tokens.cache_read / input) * 100)) : "";
    }
    const any = shares.some((one) => one.value > 0);
    return `<section class="ledger-card ledger-where">
      <div class="ledger-card-head"><span>${escape(words.where)}</span><span class="ledger-card-note">${escape(any ? note : words.noneThisWeek)}</span></div>
      ${any ? `${parts(shares, `${this.key}:${unit}`)}<div class="ledger-keys">${shares.map((one) => `<span class="ledger-keyed"><i style="background:${one.ink}"></i>${escape(one.label)}</span>`).join("")}</div>` : ""}
    </section>`;
  }

  private models(went: Went, unit: Unit, amounts: Amounts, ink = "var(--pen-clay)"): string {
    const { words } = this.options;
    const rows = went.models.filter((one) => valueOf(one.sum, unit) > 0);
    const sum = rows.reduce((all, one) => all + valueOf(one.sum, unit), 0) || 1;
    return `<section class="ledger-card ledger-models">
      <div class="ledger-card-head"><span>${escape(words.models)}</span><span class="ledger-card-note">${escape(words.thisWeekShort)}</span></div>
      ${
        rows.length
          ? rows
              .slice(0, 5)
              .map((one) => {
                const share = valueOf(one.sum, unit) / sum;
                return `<div class="ledger-model"><span class="ledger-model-name" title="${escape(one.model)}">${escape(modelName(one.model))}</span>${bar(share, `${this.key}:${one.model}`, 200, 12, ink)}<span class="ledger-model-share">${Math.round(share * 100)}%</span><span class="ledger-model-value">${escape(amounts.say(valueOf(one.sum, unit)))}</span></div>`;
              })
              .join("")
          : `<p class="ledger-none">${escape(words.noneThisWeek)}</p>`
      }
    </section>`;
  }
}

/** The report's week among the login's reset weeks, where it is one. */
function current(usage: Usage): Span | null {
  const last = usage.weeks[usage.weeks.length - 1];
  return last && Math.abs(last.end - usage.week.end) < 5 * 60_000 ? last : null;
}

/** A reset week as its bar draws it, narrowed to one person. */
function weekColumn(span: Span, unit: Unit, only: number | null): { start: number; end: number; values: Map<number, number>; reached: number | null; parts: Map<number, number> } {
  const values = new Map<number, number>();
  for (const [person, sum] of span.values) if (only === null || person === only) values.set(person, valueOf(sum, unit));
  return { start: span.start, end: span.end, values, reached: span.reached, parts: span.parts };
}

/** How amounts read in a unit: dollars marked as an estimate. */
/** Who used it are rows up to this many; past it, the rest fold into one. */
const MOST_ROWS = 5;

export function amountsOf(words: UsageWords, unit: Unit): Amounts {
  return unit === "usd"
    ? { say: (value) => words.money(value), axis: (value) => words.money(value).replace(/\.00$/, "") }
    : { say: (value) => words.tokens(value), axis: (value) => words.tokens(value) };
}

function kept(): { tab: Tab; unit: Unit } {
  try {
    const view = JSON.parse(localStorage.getItem(KEPT) ?? "{}") as Partial<{ tab: string; unit: Unit }>;
    // The days gave way to the reset weeks.
    const tab = view.tab === "daily" ? "weeks" : view.tab;
    return {
      tab: TABS.includes(tab as Tab) ? (tab as Tab) : "calendar",
      unit: view.unit === "tokens" ? "tokens" : "usd",
    };
  } catch {
    return { tab: "calendar", unit: "usd" };
  }
}

function keep(view: { tab: Tab; unit: Unit }): void {
  try {
    localStorage.setItem(KEPT, JSON.stringify(view));
  } catch {
    // Not kept: the calendar shows first next time.
  }
}

/** The account a person's key names (`people.ts`). */
function accountOf(key: string): HubAccount | null {
  const at = key.indexOf(":");
  return at > 0 ? { hub: key.slice(0, at), account: key.slice(at + 1) } : null;
}

function dirName(dir: string): string {
  return dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? dir;
}

function planName(plan: string): string {
  return plan.length ? plan[0]!.toUpperCase() + plan.slice(1) : plan;
}
