// One agent's login and what it was used for (`world-agent-logins.md`, 用量;
// `world-gui.md`, 智能体页): across the top what stands now, its windows as
// rings, this week's use and who used it; below, four drawings to choose
// from, a year of days, the week's limit, the days and the hours of the
// week; at the foot what the use went to and on which models. Claude Code
// counts in dollars as well as tokens, Codex in tokens alone. Everything
// here is Fastctx's own bookkeeping on the login's machine: opening the
// page asks that machine's node, never the model's vendor.
//
// An account the login assists sees its own use alone, with the
// assistance it has; everyone else who may see the machine sees all of it.

import { glyph } from "../../conversation/glyphs";
import { escape } from "../../conversation/prose";
import { modelName } from "../../conversation/view";
import type { AgentLimits, Fact, LimitWindow } from "../../world/agents";
import type { Login, MachineLogins, WorldLogins } from "../../world/logins";
import { wholeDollars } from "../../world/money";
import type { UsageReport } from "../../world/usage";
import type { AgentLoginsWords } from "../agent-logins-words";
import { detailOf } from "../problem";
import type { Page } from "../shell";
import { effect, type Signal } from "../signals";
import type { Store } from "../store";
import { bar, calendar, daily, parts, punch, quota, ring, type Amounts } from "./draw";
import { keepRecord, keptRecord, keptReport, readReport, reportKey } from "./kept";
import { DAY_MS, between, byDay, byHourOfWeek, dayOf, daysBefore, longestRun, pace, total, track, usageOf, valueOf, type Unit, type Usage } from "./model";
import { assistingMe, assistsOf, namer } from "./people";
import type { UsageWords } from "./words";

type Agent = "codex" | "claude";
type Tab = "calendar" | "quota" | "daily" | "punch";
type Range = "week" | "days30" | "days90";

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
  /** Begins a conversation in this login. */
  talk(machine: string, agent: Agent, home: string | null): void;
}

const KEPT = "usage.view";
/** While the page is open, the use is read again this often: a turn
 *  running elsewhere adds to it. It is the machine's own ledger, read from
 *  its node; no model is asked. */
const AGAIN_MS = 60_000;

export class LoginUsagePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly stop: () => void;
  private readonly resize: ResizeObserver;
  private readonly key: string;
  private record: MachineLogins | null;
  private report: UsageReport | null;
  private problem: string | null = null;
  private again = 0;
  /** Whether the use was asked for since the page opened. */
  private asked = false;
  private view: { tab: Tab; unit: Unit; range: Range } = kept();
  /** The person the drawings are narrowed to, by key. */
  private only: string | null = null;
  private width = 0;
  private refitting = false;
  private drawn = "";

  constructor(private readonly options: LoginUsageOptions) {
    this.key = reportKey(options.machine, options.agent, options.dir);
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
      const width = this.body.querySelector<HTMLElement>(".ledger-figure")?.clientWidth ?? 0;
      if (width && Math.abs(width - this.width) >= 1) this.draw();
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
    this.stop();
    this.resize.disconnect();
    clearTimeout(this.again);
  }

  private async read(): Promise<void> {
    const { options } = this;
    clearTimeout(this.again);
    if (!options.workbench.peek()) return;
    this.asked = true;
    const record = options.read(options.machine).then((world) => {
      const heard = world.machines.find((one) => one.kind === "answered" && one.logins.machine === options.machine);
      if (heard?.kind === "answered") {
        this.record = heard.logins;
        keepRecord(heard.logins);
      }
    });
    const report = readReport(this.key, () => options.usage(options.machine, options.agent, options.dir)).then((report) => {
      this.report = report;
      this.problem = null;
    });
    const [, used] = await Promise.allSettled([record, report]);
    if (used.status === "rejected") this.problem = detailOf(used.reason);
    this.draw();
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

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-view], [data-only]");
    if (!target) return;
    if (target.dataset.only !== undefined) this.only = this.only === target.dataset.only || !target.dataset.only ? null : target.dataset.only;
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
    if (!options.workbench.peek()) {
      html = `<p class="feature-quiet">${escape(words.workbenchOff)}</p>`;
    } else if (!this.report) {
      html = this.problem ? `<p class="feature-note" title="${escape(this.problem)}">${escape(words.failed)}</p>` : `<p class="feature-quiet ledger-reading">${escape(words.reading)}</p>`;
    } else {
      const usage = usageOf(this.report, namer(world, options.machine, words));
      const only = usage.people.find((person) => person.key === this.only) ?? null;
      const unit: Unit = usage.usd ? this.view.unit : "tokens";
      const amounts = amountsOf(words, unit);
      const mine = assistingMe(world, options.machine, options.agent, options.dir, login?.default ?? false);
      this.width = this.body.querySelector<HTMLElement>(".ledger-figure")?.clientWidth || this.body.clientWidth - 2 * 32 || 760;
      html = `${this.strip(usage, login?.limits ?? null, unit, amounts, mine, now)}
        ${this.tabs(usage)}
        <section class="ledger-panel">${this.panel(usage, only?.index ?? null, unit, amounts, now)}</section>
        <div class="ledger-cards">${this.where(usage, unit)}${this.models(usage, unit, amounts)}</div>
        ${this.problem ? `<p class="feature-note" title="${escape(this.problem)}">${escape(words.failed)}</p>` : ""}`;
    }
    if (html === this.drawn) return;
    this.drawn = html;
    const top = this.body.scrollTop;
    this.body.innerHTML = html;
    this.body.scrollTop = top;
    // The figure's width is known once it is laid out: a first drawing at a
    // guess is drawn again, once, at the width it has.
    const width = this.body.querySelector<HTMLElement>(".ledger-figure")?.clientWidth ?? 0;
    if (!this.refitting && width && Math.abs(width - this.width) >= 1) {
      this.refitting = true;
      this.draw();
      this.refitting = false;
    }
  }

  /** The head: the login's mark, who it is and its tags, and the machine. */
  private head(login: Login | null): void {
    const { options } = this;
    const identity = login?.identity?.who.kind === "known" ? login.identity.who.value : null;
    const token = identity?.method === "oauth_token";
    const plan = identity?.plan ?? null;
    const title = this.element.querySelector<HTMLElement>(".ledger-title")!;
    const html = `${glyph(options.agent, 18)}<span class="ledger-who" title="${escape(options.dir)}">${escape(this.who())}</span>${plan ? `<span class="tag">${escape(planName(plan))}</span>` : ""}${token ? `<span class="tag tag-token" title="${escape(options.logins.tokenSaid)}">${escape(options.logins.token)}</span>` : ""}<span class="ledger-machine">${glyph("machine", 14)}${escape(options.machine)}</span>`;
    if (title.innerHTML !== html) title.innerHTML = html;
  }

  /** What stands now: the windows, this week, and who used it. */
  private strip(usage: Usage, limits: Fact<AgentLimits> | null, unit: Unit, amounts: Amounts, mine: ReturnType<typeof assistingMe>, now: number): string {
    const { words } = this.options;
    const windows = limits?.kind === "known" ? limits.value.windows : [];
    const rings = [300, 10080]
      .map((minutes) => {
        const window = windows.find((one) => one.minutes === minutes || one.name.startsWith(minutes === 300 ? "five_hour" : "seven_day")) ?? null;
        return this.ring(minutes, window, now);
      })
      .join("");
    const only = usage.people.find((person) => person.key === this.only) ?? null;
    const shown = only ? [only] : usage.people;
    const week = shown.reduce((sum, person) => sum + valueOf(person.week, unit), 0);
    const weekTokens = shown.reduce((sum, person) => sum + person.week.tokens, 0);
    const { start } = usage.week;
    const elapsed = now - start;
    const before = total(between(usage, unit, start - 7 * DAY_MS, start - 7 * DAY_MS + elapsed, only?.index ?? null));
    const sofar = total(between(usage, unit, start, now + 1, only?.index ?? null));
    const today = total(between(usage, unit, dayOf(now), now + 1, only?.index ?? null));
    const change = before > 0 ? Math.round((sofar / before - 1) * 100) : null;
    const big =
      unit === "usd"
        ? `<span class="ledger-about" title="${escape(words.aboutWhy)}">${escape(words.about)}</span>${escape(words.money(week))}`
        : `${escape(words.tokens(week))}<span class="ledger-unit">${escape(words.tokenUnit)}</span>`;
    const facts = [
      unit === "usd" ? `<span>${escape(words.tokens(weekTokens))} ${escape(words.tokenUnit)}</span>` : "",
      change === null ? (sofar > 0 ? `<span>${escape(words.noLastWeek)}</span>` : "") : `<span class="${change > 0 ? "ledger-up" : ""}">${escape(words.vsLastWeek(change))}</span>`,
      `<span>${escape(words.today(today > 0 ? amounts.say(today) : null))}</span>`,
    ].join("");
    const totals = `<div class="ledger-totals">
        <span class="ledger-key" title="${escape(`${words.weekSpan(new Date(start), new Date(usage.week.end))}\n${words.weekWhy(usage.week.window)}`)}">${escape(usage.people.length === 1 && usage.people[0]!.me && mine ? words.myWeek : words.thisWeek)}</span>
        <span class="ledger-big">${big}</span>
        <span class="ledger-facts">${facts}</span>
      </div>`;
    return `<section class="ledger-strip"><div class="ledger-rings">${rings}</div>${totals}${mine ? this.assisted(usage, mine, now) : this.people(usage, unit, amounts)}</section>`;
  }

  private ring(minutes: number, window: LimitWindow | null, now: number): string {
    const { words, logins } = this.options;
    const share = window ? window.used_tenths / 1000 : null;
    const left = window?.resets_at ? logins.resetsIn(new Date(window.resets_at), new Date(now)) : "";
    const name = words.window(minutes, window?.name ?? "");
    const said = window ? `${name} · ${logins.used(`${Math.round(window.used_tenths / 10)}%`)}${window.resets_at ? `\n${logins.resets(new Date(window.resets_at), new Date(now))}` : ""}` : `${name} · ${words.notRead}`;
    return `<div class="ledger-ring" title="${escape(said)}">
      <span class="ledger-ring-face">${ring(share, `${this.key}:${minutes}`)}<span class="ledger-ring-num"><span>${share === null ? "–" : `${Math.round(share * 100)}<small>%</small>`}</span></span></span>
      <span class="ledger-ring-name">${escape(name)}</span>
      <span class="ledger-ring-left">${escape(left ? words.resetsIn(left) : window ? "" : words.notRead)}</span>
    </div>`;
  }

  /** Who used it this week, each a row to narrow the drawings to. */
  private people(usage: Usage, unit: Unit, amounts: Amounts): string {
    const { words } = this.options;
    const world = this.options.store.world.peek();
    const login = this.login();
    const assists = assistsOf(world, this.options.machine, this.options.agent, this.options.dir, login?.default ?? false);
    const most = Math.max(1e-9, ...usage.people.map((person) => valueOf(person.week, unit)));
    const now = this.options.store.clock.peek();
    const rows = usage.people
      .map((person) => {
        const assisted = assists.find(({ hub, assist }) => person.key === `${hub}:${assist.account}`)?.assist;
        const why = assisted ? [words.assistUntil(assisted.until ? new Date(assisted.until) : null, now), assisted.weekly ? words.weekly(wholeDollars(assisted.weekly / 1_000_000)) : ""].filter(Boolean).join(" · ") : "";
        const value = valueOf(person.week, unit);
        const state = this.only === person.key ? " on" : this.only ? " faded" : "";
        return `<button class="ledger-person${state}" type="button" data-only="${escape(person.key)}">
          <span class="ledger-swatch" style="background:${person.ink}"></span>
          <span class="ledger-person-name">${escape(person.name)}${why ? `<span class="ledger-person-why">${escape(why)}</span>` : ""}</span>
          <span class="ledger-person-value">${escape(value ? amounts.say(value) : "–")}</span>
          ${bar(value / most, `${this.key}:${person.key}`, 92, 12, person.ink)}
        </button>`;
      })
      .join("");
    const all = this.only ? `<button class="text-button ledger-all" type="button" data-only="">${escape(words.all)}</button>` : "";
    return `<div class="ledger-people"><div class="ledger-key-row"><span class="ledger-key">${escape(words.who)}</span>${all}</div>${rows || `<p class="ledger-none">${escape(words.noneThisWeek)}</p>`}</div>`;
  }

  /** The assistance the reader uses the login by: from whom, until when,
   *  and what it may spend of it a week. */
  private assisted(usage: Usage, mine: NonNullable<ReturnType<typeof assistingMe>>, now: number): string {
    const { words } = this.options;
    const { assist, owner } = mine;
    const spent = usage.people.find((person) => person.me)?.week.usd ?? 0;
    const weekly = assist.weekly ? assist.weekly / 1_000_000 : null;
    return `<div class="ledger-assisted">
      <span class="ledger-key">${escape(words.assistedBy(owner ?? "–"))}</span>
      <span class="ledger-assisted-until">${glyph("history", 14)}${escape(words.assistUntil(assist.until ? new Date(assist.until) : null, new Date(now)))}</span>
      ${
        weekly
          ? `<span class="ledger-assisted-weekly">${escape(words.weeklyUsed(words.money(spent), wholeDollars(weekly)))}</span>${bar(spent / weekly, `${this.key}:allowance`, 200, 12, spent >= weekly ? "var(--wrong)" : "var(--pen-clay)")}`
          : `<span class="ledger-assisted-weekly">${escape(words.weeklyNone)}</span>`
      }
      <span class="ledger-assisted-note">${escape(words.onlyYours)}</span>
    </div>`;
  }

  private tabs(usage: Usage): string {
    const { words } = this.options;
    const tab = (value: Tab) => `<button type="button" role="tab" class="${this.view.tab === value ? "current" : ""}" aria-selected="${this.view.tab === value}" data-view="tab:${value}">${escape(words.tabs[value])}</button>`;
    const unit = usage.usd && this.view.tab !== "quota"
      ? `<div class="ledger-switch" role="group">${(["usd", "tokens"] as const).map((value) => `<button type="button" class="${this.view.unit === value ? "current" : ""}" aria-pressed="${this.view.unit === value}" data-view="unit:${value}">${escape(value === "usd" ? words.usd : words.tokenTab)}</button>`).join("")}</div>`
      : "";
    return `<div class="ledger-tabs"><div class="ledger-switch" role="tablist">${(["calendar", "quota", "daily", "punch"] as const).map(tab).join("")}</div>${unit}</div>`;
  }

  /** The drawing chosen, at the width it has. */
  private panel(usage: Usage, only: number | null, unit: Unit, amounts: Amounts, now: number): string {
    const { words } = this.options;
    const width = Math.max(280, this.width);
    const shown = only === null ? usage.people : usage.people.filter((person) => person.index === only);
    const ink = only === null ? "var(--pen-clay)" : usage.people[only]!.ink;
    const seed = `${this.key}:${unit}:${only ?? ""}`;
    const today = dayOf(now);
    switch (this.view.tab) {
      case "calendar": {
        const days = byDay(usage, unit, only);
        const drawn = calendar({ width, days, people: shown, today, ink, amounts, words, seed });
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
        const week = track(usage, 10080);
        const five = track(usage, 300);
        const height = width < 520 ? 230 : 290;
        if (!week && !five) return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.quotaTitle)}</span></div><div class="ledger-figure ledger-empty-figure"><p class="feature-quiet">${escape(words.quotaUnread)}</p></div>`;
        const drawn = quota({ width, height, start: usage.week.start, end: usage.week.end, now, window: usage.week.window, week, five, pace: week ? pace(week, now) : null, words, seed: this.key });
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.quotaTitle)}</span>${drawn.said ? `<span class="ledger-panel-said">${escape(drawn.said)}</span>` : ""}</div><div class="ledger-figure">${drawn.svg}</div>`;
      }
      case "daily": {
        const range = this.view.range;
        const first = range === "week" ? dayOf(usage.week.start) : daysBefore(today, range === "days30" ? 29 : 89);
        const days = byDay(usage, unit, only);
        const columns: { day: number; values: Map<number, number> }[] = [];
        for (let day = first; day <= today; day = daysBefore(day, -1)) columns.push({ day, values: days.get(day) ?? new Map() });
        const ranges = `<div class="ledger-switch small" role="group">${(["week", "days30", "days90"] as const).map((value) => `<button type="button" class="${range === value ? "current" : ""}" aria-pressed="${range === value}" data-view="range:${value}">${escape(words.ranges[value])}</button>`).join("")}</div>`;
        const legend = shown.length > 1 ? `<span class="ledger-legend">${shown.map((person) => `<span class="ledger-keyed"><i style="background:${person.ink}"></i>${escape(person.name)}</span>`).join("")}</span>` : "";
        return `<div class="ledger-panel-head">${ranges}<span class="ledger-grow"></span>${legend}</div><div class="ledger-figure">${daily({ width, height: width < 520 ? 220 : 280, columns, people: shown, amounts, words, seed: `${seed}:${range}` })}</div>`;
      }
      case "punch": {
        const grid = byHourOfWeek(usage, unit, now, 90, only);
        const all = grid.flat().reduce((sum, value) => sum + value, 0);
        const night = grid.reduce((sum, row) => sum + row.slice(0, 6).reduce((a, b) => a + b, 0), 0);
        return `<div class="ledger-panel-head"><span class="ledger-panel-title">${escape(words.punchTitle)}</span>${all ? `<span class="ledger-panel-said">${escape(words.punchSaid(Math.round((night / all) * 100)))}</span>` : ""}</div><div class="ledger-figure">${punch({ width, height: width < 520 ? 230 : 270, grid, ink, amounts, words, seed })}</div>`;
      }
    }
  }

  /** What the week's use went to: by what it cost, or by tokens. */
  private where(usage: Usage, unit: Unit): string {
    const { words } = this.options;
    const { tokens, kinds } = usage.week;
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
      ].map((one) => ({ ...one, said: `${one.label} ${words.about} ${one.said}`, label: `${one.label} ${percent(one.value)}` }));
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
    return `<section class="ledger-card">
      <div class="ledger-card-head"><span>${escape(words.where)}</span><span class="ledger-card-note">${escape(any ? note : words.noneThisWeek)}</span></div>
      ${any ? `${parts(shares, `${this.key}:${unit}`)}<div class="ledger-keys">${shares.map((one) => `<span class="ledger-keyed"><i style="background:${one.ink}"></i>${escape(one.label)}</span>`).join("")}</div>` : ""}
    </section>`;
  }

  private models(usage: Usage, unit: Unit, amounts: Amounts): string {
    const { words } = this.options;
    const rows = usage.week.models.filter((one) => valueOf(one.sum, unit) > 0);
    const sum = rows.reduce((all, one) => all + valueOf(one.sum, unit), 0) || 1;
    return `<section class="ledger-card">
      <div class="ledger-card-head"><span>${escape(words.models)}</span><span class="ledger-card-note">${escape(words.thisWeekShort)}</span></div>
      ${
        rows.length
          ? rows
              .slice(0, 5)
              .map((one) => {
                const share = valueOf(one.sum, unit) / sum;
                return `<div class="ledger-model"><span class="ledger-model-name" title="${escape(one.model)}">${escape(modelName(one.model))}</span>${bar(share, `${this.key}:${one.model}`, 200, 12)}<span class="ledger-model-share">${Math.round(share * 100)}%</span><span class="ledger-model-value">${escape(amounts.say(valueOf(one.sum, unit)))}</span></div>`;
              })
              .join("")
          : `<p class="ledger-none">${escape(words.noneThisWeek)}</p>`
      }
    </section>`;
  }
}

/** How amounts read in a unit: dollars marked as an estimate. */
export function amountsOf(words: UsageWords, unit: Unit): Amounts {
  return unit === "usd"
    ? { say: (value) => `${words.about} ${words.money(value)}`, axis: (value) => words.money(value).replace(/\.00$/, "") }
    : { say: (value) => words.tokens(value), axis: (value) => words.tokens(value) };
}

function kept(): { tab: Tab; unit: Unit; range: Range } {
  try {
    const view = JSON.parse(localStorage.getItem(KEPT) ?? "{}") as Partial<{ tab: Tab; unit: Unit; range: Range }>;
    return {
      tab: (["calendar", "quota", "daily", "punch"] as const).includes(view.tab as Tab) ? view.tab! : "calendar",
      unit: view.unit === "tokens" ? "tokens" : "usd",
      range: (["week", "days30", "days90"] as const).includes(view.range as Range) ? view.range! : "days30",
    };
  } catch {
    return { tab: "calendar", unit: "usd", range: "days30" };
  }
}

function keep(view: { tab: Tab; unit: Unit; range: Range }): void {
  try {
    localStorage.setItem(KEPT, JSON.stringify(view));
  } catch {
    // Not kept: the calendar shows first next time.
  }
}

function dirName(dir: string): string {
  return dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? dir;
}

function planName(plan: string): string {
  return plan.length ? plan[0]!.toUpperCase() + plan.slice(1) : plan;
}
