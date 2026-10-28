// The activity page (`world-layers.md`, 活动账本; `world-gui.md`, 活动): what
// agents did with `run` on the user's machines, in eight views, one screen
// each. Every machine keeps its own book; the page asks each it can reach
// and puts their answers together, saying which did not answer. Now and
// the day's lanes are read again every few seconds while the page is open;
// the year's hours each minute. A deed pressed rises as a small card with
// its command whole, where it ran, how it ended and what it changed.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { parts } from "../app/usage/draw";
import type { Page } from "../app/shell";
import { effect, type Signal } from "../app/signals";
import type { BookAnswer, BookQuery, Deed } from "../world/book";
import { elapsed, lines, rhythm, runs, said, timeline, year, type Lane } from "./draw";
import {
  DAY_MS,
  began,
  byDay,
  byHourOfWeek,
  dayOf,
  empty,
  ended,
  failing,
  files,
  headline,
  heeds,
  lasted,
  sessions,
  spread,
  type Counted,
  type Gathered,
  type HeedKind,
  type Placed,
} from "./model";
import type { ActivityWords, Span, View } from "./words";
import "./activity.css";

const VIEWS: readonly View[] = ["now", "timeline", "calendar", "rhythm", "spread", "files", "heed", "sessions"];
const SPANS: readonly Span[] = ["day", "week", "month", "quarter"];
const SPAN_DAYS: Record<Span, number> = { day: 1, week: 7, month: 30, quarter: 90 };
const HEEDS: readonly HeedKind[] = ["failed", "lost", "stopped", "removed", "long", "big"];
const KEPT_VIEW = "activity.view";
const KEPT_SPAN = "activity.span";
/** How often what runs is read again while the page is open, and the year. */
const NOW_MS = 4000;
const YEAR_MS = 60_000;
const LIMIT = 2000;

export interface BookMachine {
  /** None for this computer. */
  written: string | null;
  name: string;
  online: boolean;
}

export interface ActivityOptions {
  words: ActivityWords;
  head(title: string, actions?: string): string;
  machines(): BookMachine[];
  read(machine: string | null, query: BookQuery): Promise<BookAnswer>;
  /** Whether Fastctx's own conversations are shown: projects, and
   *  bringing a session in. */
  workbench: Signal<boolean>;
  bringIn(resident: string): Promise<void>;
  /** The page's own clock, for the specimen. */
  now?(): number;
}

export class ActivityPage implements Page {
  readonly element = document.createElement("section");
  readonly title: string;
  private view: View = kept(KEPT_VIEW, VIEWS, "now");
  private span: Span = kept(KEPT_SPAN, SPANS, "week");
  /** The day the timeline shows, as its local midnight. */
  private day: number;
  private gathered: Gathered = empty();
  private yearHours: Counted[] = [];
  /** Deeds of days older than the span read, read for the timeline. */
  private daysRead = new Map<number, Gathered>();
  private picked: { job: string; machine: string } | null = null;
  private readonly body: HTMLElement;
  private readonly card: HTMLElement;
  private readonly switcher: HTMLElement;
  private timers: number[] = [];
  private readonly resize: ResizeObserver;
  private width = 0;
  private height = 0;
  private stopWatch: () => void;
  private reading = 0;
  private bringing = new Set<string>();

  constructor(private readonly options: ActivityOptions) {
    const { words } = options;
    this.title = words.title;
    this.day = dayOf(this.now());
    this.element.className = "page ay-page";
    this.element.innerHTML = `${options.head(escape(words.title))}
      <nav class="ay-views" aria-label="${escape(words.title)}"></nav>
      <div class="ay-body"></div>
      <aside class="ay-card" hidden></aside>`;
    this.switcher = this.element.querySelector<HTMLElement>(".ay-views")!;
    this.body = this.element.querySelector<HTMLElement>(".ay-body")!;
    this.card = this.element.querySelector<HTMLElement>(".ay-card")!;
    this.drawSwitcher();
    this.element.addEventListener("click", (event) => this.pressed(event));
    this.resize = new ResizeObserver(([entry]) => {
      const width = Math.floor(entry?.contentRect.width ?? 0);
      const height = Math.floor(entry?.contentRect.height ?? 0);
      if (width && (width !== this.width || height !== this.height)) {
        this.width = width;
        this.height = height;
        this.draw();
      }
    });
    this.resize.observe(this.body);
    this.stopWatch = effect(() => {
      options.workbench.get();
      this.draw();
    });
    void this.readYear();
    void this.readSpan();
    this.timers.push(window.setInterval(() => void this.readSpan(), NOW_MS));
    this.timers.push(window.setInterval(() => void this.readYear(), YEAR_MS));
  }

  leave(): void {
    for (const timer of this.timers) window.clearInterval(timer);
    this.timers = [];
    this.resize.disconnect();
    this.stopWatch();
  }

  private now(): number {
    return this.options.now?.() ?? Date.now();
  }

  /** Every machine's answer to `query`, the ones that did not answer named. */
  private async askAll(query: BookQuery): Promise<{ answers: BookAnswer[]; away: string[] }> {
    const machines = this.options.machines();
    const asked = await Promise.allSettled(
      machines.map((machine) => (machine.online || machine.written === null ? this.options.read(machine.written, query) : Promise.reject(new Error("offline")))),
    );
    const answers: BookAnswer[] = [];
    const away: string[] = [];
    asked.forEach((result, index) => {
      if (result.status === "fulfilled") answers.push(result.value);
      else away.push(machines[index]!.name);
    });
    return { answers, away };
  }

  private async readSpan(): Promise<void> {
    const reading = ++this.reading;
    const now = this.now();
    const days = Math.max(SPAN_DAYS[this.span], 2);
    const from = dayOf(now) - (days - 1) * DAY_MS;
    const { answers, away } = await this.askAll({ ask: "deeds", from: iso(from), to: iso(now + 60_000), limit: LIMIT });
    if (reading !== this.reading) return;
    const deeds: Placed[] = [];
    let more = false;
    for (const answer of answers) {
      if (answer.kind !== "deeds") continue;
      more ||= answer.more;
      for (const deed of answer.deeds) deeds.push({ ...deed, machine: answer.machine });
    }
    this.gathered = { ...this.gathered, deeds, away, more };
    this.draw();
  }

  private async readYear(): Promise<void> {
    const now = this.now();
    const { answers } = await this.askAll({ ask: "hours", from: iso(now - 366 * DAY_MS), to: iso(now + 60_000) });
    const hours: Counted[] = [];
    for (const answer of answers) if (answer.kind === "hours") for (const hour of answer.hours) hours.push({ ...hour, machine: answer.machine });
    this.yearHours = hours;
    if (this.view === "calendar" || this.view === "rhythm" || this.view === "spread") this.draw();
  }

  /** The deeds of the timeline's day: from what is read, or read for it. */
  private deedsOfDay(day: number): Gathered | null {
    const now = this.now();
    const span = Math.max(SPAN_DAYS[this.span], 2);
    if (day >= dayOf(now) - (span - 1) * DAY_MS) {
      return { ...this.gathered, deeds: this.gathered.deeds.filter((deed) => dayOf(began(deed)) === day) };
    }
    const read = this.daysRead.get(day);
    if (read) return read;
    void this.askAll({ ask: "deeds", from: iso(day), to: iso(day + DAY_MS), limit: LIMIT }).then(({ answers, away }) => {
      const deeds: Placed[] = [];
      let more = false;
      for (const answer of answers) {
        if (answer.kind !== "deeds") continue;
        more ||= answer.more;
        for (const deed of answer.deeds) deeds.push({ ...deed, machine: answer.machine });
      }
      this.daysRead.set(day, { deeds, hours: [], away, more });
      if (this.view === "timeline" && this.day === day) this.draw();
    });
    return null;
  }

  private drawSwitcher(): void {
    const { words } = this.options;
    this.switcher.innerHTML = VIEWS.map(
      (view) => `<button class="choice${view === this.view ? " chosen" : ""}" type="button" data-view="${view}"${view === this.view ? ' aria-current="page"' : ""}>${escape(words.views[view])}</button>`,
    ).join("");
  }

  private spans(): string {
    const { words } = this.options;
    return `<div class="ay-spans">${SPANS.map((span) => `<button class="choice${span === this.span ? " chosen" : ""}" type="button" data-span="${span}">${escape(words.spans[span])}</button>`).join("")}</div>`;
  }

  /** Deeds within the span chosen, newest first. */
  private inSpan(): Placed[] {
    const from = dayOf(this.now()) - (SPAN_DAYS[this.span] - 1) * DAY_MS;
    return this.gathered.deeds.filter((deed) => began(deed) >= from).sort((a, b) => began(b) - began(a));
  }

  private draw(): void {
    if (!this.width) return;
    const scroll = this.body.querySelector<HTMLElement>(".ay-scroll")?.scrollTop ?? 0;
    this.body.dataset.view = this.view;
    let html: string;
    switch (this.view) {
      case "now":
        html = this.drawNow();
        break;
      case "timeline":
        html = this.drawTimeline();
        break;
      case "calendar":
        html = this.drawCalendar();
        break;
      case "rhythm":
        html = this.drawRhythm();
        break;
      case "spread":
        html = this.drawSpread();
        break;
      case "files":
        html = this.drawFiles();
        break;
      case "heed":
        html = this.drawHeed();
        break;
      case "sessions":
        html = this.drawSessions();
        break;
    }
    const away = this.gathered.away.length ? `<p class="ay-away" title="${escape(this.options.words.unanswered(this.gathered.away.join(", ")))}">${glyph("info", 14)}<span>${escape(this.options.words.unanswered(this.gathered.away.join(", ")))}</span></p>` : "";
    this.body.innerHTML = html + away;
    const scroller = this.body.querySelector<HTMLElement>(".ay-scroll");
    if (scroller) scroller.scrollTop = scroll;
    this.drawCard();
  }

  private drawNow(): string {
    const { words } = this.options;
    const now = this.now();
    const running = this.gathered.deeds.filter((deed) => deed.outcome === "running");
    const machines = new Map<string, Placed[]>();
    for (const deed of running) machines.set(deed.machine, [...(machines.get(deed.machine) ?? []), deed]);
    const desks = [...machines.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([machine, deeds]) => {
        const rows = deeds
          .sort((a, b) => began(a) - began(b))
          .map((deed) => `<li><button class="ay-deed" type="button" data-job="${escape(deed.job)}" data-machine="${escape(deed.machine)}" title="${escape(said(deed, now, words, (at) => words.stamp(at, now)))}">${mark(deed)}<span class="ay-deed-line">${escape(headline(deed.script))}</span>${elapsed(lasted(deed, now), deed.job, 96)}</button></li>`)
          .join("");
        return `<article class="ay-desk"><h3 class="ay-desk-name">${glyph("machine", 16)}<span>${escape(machine)}</span></h3><ul class="ay-desk-deeds">${rows}</ul></article>`;
      })
      .join("");
    const lately = this.gathered.deeds
      .filter((deed) => deed.outcome !== "running")
      .sort((a, b) => (ended(b) ?? began(b)) - (ended(a) ?? began(a)))
      .slice(0, 12);
    const quiet = !running.length ? `<div class="ay-idle"><p>${escape(words.idle)}</p></div>` : `<div class="ay-desks">${desks}</div>`;
    return `<div class="ay-now">
      ${quiet}
      ${lately.length ? `<section class="ay-lately"><h3 class="ay-label">${escape(words.lately)}</h3><ul class="ay-scroll ay-rows">${lately.map((deed) => this.row(deed, now)).join("")}</ul></section>` : ""}
    </div>`;
  }

  /** One deed as a line: how it went, its command, how many times where
   *  it stands for several, where, when. */
  private row(deed: Placed, now: number, times = 1): string {
    const { words } = this.options;
    const at = ended(deed) ?? began(deed);
    return `<li><button class="ay-deed ay-row" type="button" data-job="${escape(deed.job)}" data-machine="${escape(deed.machine)}" title="${escape(said(deed, now, words, (moment) => words.stamp(moment, now)))}">${mark(deed)}<span class="ay-deed-line">${escape(headline(deed.script))}${times > 1 ? `<span class="ay-times">×${times}</span>` : ""}</span><span class="ay-where">${escape(deed.machine)}</span><span class="ay-when">${escape(words.stamp(at, now))}</span></button></li>`;
  }

  private drawTimeline(): string {
    const { words } = this.options;
    const now = this.now();
    const today = dayOf(now);
    const gathered = this.deedsOfDay(this.day);
    const lanes: Lane[] = [];
    const deeds = gathered?.deeds ?? [];
    const byMachine = new Map<string, Placed[]>();
    for (const deed of deeds) byMachine.set(deed.machine, [...(byMachine.get(deed.machine) ?? []), deed]);
    for (const [machine, mine] of [...byMachine.entries()].sort(([a], [b]) => a.localeCompare(b))) lanes.push({ machine, deeds: mine, away: false });
    for (const machine of gathered?.away ?? []) lanes.push({ machine, deeds: [], away: true });
    const nav = `<div class="ay-day">
        <button class="icon-button" type="button" data-day-step="-1" aria-label="${escape(words.previousDay)}">${glyph("back", 18)}</button>
        <span class="ay-day-name">${escape(words.date(this.day))}</span>
        <button class="icon-button" type="button" data-day-step="1" aria-label="${escape(words.nextDay)}"${this.day >= today ? " disabled" : ""}><span class="ay-flip">${glyph("back", 18)}</span></button>
        ${this.day !== today ? `<button class="text-button" type="button" data-day-today>${escape(words.today)}</button>` : ""}
      </div>`;
    if (!gathered) return `${nav}<div class="ay-figure"></div>`;
    if (!lanes.length) return `${nav}<div class="ay-quiet"><p>${escape(words.quiet)}</p></div>`;
    const drawn = timeline({ width: this.width, lanes, day: this.day, now, words, when: (at) => words.stamp(at, now), seed: `${this.day}` });
    return `${nav}<div class="ay-figure ay-scroll">${drawn.svg}</div>`;
  }

  private drawCalendar(): string {
    const { words } = this.options;
    const today = dayOf(this.now());
    const days = byDay(this.yearHours);
    const drawn = year({ width: Math.min(this.width, 1100), height: this.height - 24, days, today, words, date: words.date, month: words.month, seed: "year" });
    return `<div class="ay-figure ay-calendar">${drawn.svg}</div>`;
  }

  private drawRhythm(): string {
    const grid = byHourOfWeek(this.yearHours, this.now());
    const width = Math.min(this.width, 1100);
    const height = Math.max(200, Math.min(this.height - 24, Math.round(width * 0.42)));
    return `<div class="ay-figure ay-rhythm-box">${rhythm(width, height, grid, this.options.words, "week")}</div>`;
  }

  private drawSpread(): string {
    const { words } = this.options;
    const now = this.now();
    const from = dayOf(now) - (SPAN_DAYS[this.span] - 1) * DAY_MS;
    const inks = ["var(--pen-graphite)", "var(--pen-sky)", "var(--pen-sage)", "var(--pen-rose)", "var(--pen-trace)"];
    const width = Math.min(this.width - 8, 760);
    const row = (label: string, shares: { key: string; runs: number; failed: number; busy: number }[], name: (key: string) => string, inkOf?: (key: string, index: number) => string) => {
      const total = shares.reduce((sum, one) => sum + one.runs, 0);
      if (!total) return "";
      const shown = shares.slice(0, 5);
      const drawn = shown.map((one, index) => ({ value: one.runs, ink: inkOf?.(one.key, index) ?? inks[index % inks.length]!, said: `${name(one.key)}\n${words.count(one.runs)}${one.failed ? `\n${words.failed(one.failed)}` : ""}\n${words.ran(words.duration(one.busy))}` }));
      const keys = shown.map((one, index) => `<span class="ay-key"><i style="background:${drawn[index]!.ink}"></i>${escape(name(one.key))}</span>`).join("");
      return `<div class="ay-spread-row"><h3 class="ay-label">${escape(label)}</h3>${parts(drawn, `${label}:${this.span}`, width)}<div class="ay-keys">${keys}</div></div>`;
    };
    const machines = spread(this.yearHours, from, (hour) => hour.machine);
    const agents = spread(this.yearHours, from, (hour) => hour.agent ?? (hour.session ? "other" : "page"));
    const outcomes = (() => {
      const all = spread(this.yearHours, from, () => "all")[0];
      if (!all) return [];
      return [
        { key: "done", runs: all.runs - all.failed, failed: 0, busy: 0 },
        { key: "failed", runs: all.failed, failed: all.failed, busy: 0 },
      ].filter((one) => one.runs > 0);
    })();
    const projects = this.options.workbench.peek()
      ? (() => {
          const counts = new Map<string, number>();
          for (const deed of this.inSpan()) counts.set(deed.project ?? "", (counts.get(deed.project ?? "") ?? 0) + 1);
          return [...counts.entries()].map(([key, runs]) => ({ key, runs, failed: 0, busy: 0 })).sort((a, b) => b.runs - a.runs);
        })()
      : [];
    const rows = [
      row(words.byMachine, machines, (key) => key),
      row(words.byAgent, agents, (key) => words.agent[key as keyof ActivityWords["agent"]] ?? key),
      row(words.byOutcome, outcomes, (key) => words.outcome[key as "done" | "failed"], (key) => (key === "failed" ? "var(--pen-clay)" : "var(--pen-graphite)")),
      projects.length ? row(words.byProject, projects, (key) => key || words.noProject) : "",
    ].join("");
    return `${this.spans()}${rows ? `<div class="ay-spread">${rows}</div>` : `<div class="ay-quiet"><p>${escape(words.quiet)}</p></div>`}`;
  }

  private drawFiles(): string {
    const { words } = this.options;
    const now = this.now();
    const grouped = files(this.inSpan());
    if (!grouped.size) return `${this.spans()}<div class="ay-quiet"><p>${escape(words.quiet)}</p></div>`;
    const most = Math.max(1, ...[...grouped.values()].flatMap((folders) => [...folders.values()].flat().map((file) => Math.max(file.added ?? 0, file.removed ?? 0))));
    const sections = [...grouped.entries()]
      .map(([machine, folders]) => {
        const items = [...folders.entries()]
          .map(([folder, list]) => {
            const rows = list
              .map((file) => {
                const name = file.path.slice(file.path.replace(/\\/g, "/").lastIndexOf("/") + 1);
                const hover = [file.path, words.how[file.how], file.seen ? words.seen : "", words.stamp(file.at, now)].filter(Boolean).join("\n");
                return `<li><button class="ay-file${file.seen ? " seen" : ""}" type="button" data-job="${escape(file.job)}" data-machine="${escape(file.machine)}" title="${escape(hover)}"><span class="ay-how ay-how-${file.how}" aria-label="${escape(words.how[file.how])}"></span><span class="ay-file-name">${escape(name)}</span>${lines(file.added, file.removed, most, file.path)}</button></li>`;
              })
              .join("");
            return `<li class="ay-folder"><p class="ay-folder-name" title="${escape(folder)}"><span>${escape(folder)}</span></p><ul>${rows}</ul></li>`;
          })
          .join("");
        return `<section class="ay-files-machine"><h3 class="ay-label">${glyph("machine", 14)}${escape(machine)}</h3><ul class="ay-folders">${items}</ul></section>`;
      })
      .join("");
    return `${this.spans()}<div class="ay-scroll ay-files">${sections}</div>`;
  }

  private drawHeed(): string {
    const { words } = this.options;
    const now = this.now();
    const found = heeds(this.inSpan(), now);
    const sections = HEEDS.filter((kind) => found.get(kind)?.length)
      .map((kind) => {
        const deeds = found.get(kind)!;
        // The same command on the same machine once, with how many times.
        const folded = new Map<string, Placed[]>();
        for (const deed of deeds) {
          const key = `${deed.machine}\n${headline(deed.script)}`;
          folded.set(key, [...(folded.get(key) ?? []), deed]);
        }
        const rows = [...folded.values()]
          .sort((a, b) => b.length - a.length || began(b[0]!) - began(a[0]!))
          .slice(0, 24)
          .map((same) => this.row(same[0]!, now, same.length))
          .join("");
        return `<section class="ay-heed"><h3 class="ay-label">${escape(words.heed[kind])}<span class="ay-count">${deeds.length}</span></h3><ul class="ay-rows">${rows}</ul></section>`;
      })
      .join("");
    return `${this.spans()}${sections ? `<div class="ay-scroll ay-heeds">${sections}</div>` : `<div class="ay-quiet"><p>${escape(words.quiet)}</p></div>`}`;
  }

  private drawSessions(): string {
    const { words } = this.options;
    const now = this.now();
    const seen = sessions(this.inSpan());
    if (!seen.length) return `${this.spans()}<div class="ay-quiet"><p>${escape(words.quiet)}</p></div>`;
    const workbench = this.options.workbench.peek();
    const from = dayOf(now) - (SPAN_DAYS[this.span] - 1) * DAY_MS;
    const rows = seen
      .map((one) => {
        const resident = one.session.slice(0, one.session.lastIndexOf("@"));
        const short = resident.length > 14 ? `${resident.slice(0, 13)}…` : resident;
        const hover = [
          one.session,
          one.held ? words.held : words.own,
          `${words.stamp(one.first, now)} – ${words.stamp(one.last, now)}`,
          words.count(one.runs),
          one.failed ? words.failed(one.failed) : "",
          one.files ? words.files(one.files) : "",
        ]
          .filter(Boolean)
          .join("\n");
        const canBring = workbench && !one.held && one.agent === "codex" && one.home === this.homeName();
        const bring = canBring
          ? `<button class="text-button ay-bring" type="button" data-bring="${escape(resident)}" title="${escape(words.bringInWhy)}"${this.bringing.has(resident) ? " disabled" : ""}>${escape(words.bringIn)}</button>`
          : "";
        return `<li class="ay-session${one.held ? " held" : ""}${one.running ? " running" : ""}" title="${escape(hover)}">
            <span class="ay-agent">${one.agent ? glyph(one.agent, 16) : glyph("machine", 16)}</span>
            <span class="ay-session-name">${escape(short)}</span>
            <span class="ay-where">${escape(one.home)}</span>
            <span class="ay-span">${runs(one.times, from, now, one.session)}</span>
            ${bring}
          </li>`;
      })
      .join("");
    return `${this.spans()}<ul class="ay-scroll ay-sessions">${rows}</ul>`;
  }

  private homeName(): string {
    return this.options.machines().find((machine) => machine.written === null)?.name ?? "";
  }

  private find(job: string, machine: string): Placed | null {
    const all = [this.gathered.deeds, ...[...this.daysRead.values()].map((day) => day.deeds)].flat();
    return all.find((deed) => deed.job === job && deed.machine === machine) ?? null;
  }

  private drawCard(): void {
    const deed = this.picked ? this.find(this.picked.job, this.picked.machine) : null;
    if (!deed) {
      this.card.hidden = true;
      return;
    }
    const { words } = this.options;
    const now = this.now();
    const time = deed.outcome === "running" ? words.running(words.duration(lasted(deed, now))) : words.ran(words.duration(lasted(deed, now)));
    const where = [deed.machine, deed.from ? `← ${deed.from}` : "", deed.went?.length ? words.went(deed.went.join(", ")) : ""].filter(Boolean).join(" · ");
    const touched = (deed.touched ?? [])
      .slice(0, 12)
      .map((file) => `<li title="${escape([file.path, words.how[file.how], file.seen ? words.seen : ""].filter(Boolean).join("\n"))}"><span class="ay-how ay-how-${file.how}"></span><span class="ay-file-name">${escape(file.path)}</span></li>`)
      .join("");
    this.card.hidden = false;
    this.card.innerHTML = `<header class="ay-card-head">${mark(deed)}<span class="ay-card-state">${escape(words.outcome[deed.outcome])}${deed.code !== undefined && deed.code !== 0 ? ` · ${deed.code}` : ""}</span><span class="ay-when" title="${escape(new Date(began(deed)).toLocaleString())}">${escape(words.stamp(began(deed), now))} · ${escape(time)}</span><button class="icon-button ay-card-close" type="button" data-card-close aria-label="${escape(words.title)}">${glyph("close", 16)}</button></header>
      <pre class="ay-script">${escape(deed.script)}</pre>
      <p class="ay-card-line" title="${escape(deed.session ?? "")}">${deed.agent ? glyph(deed.agent, 14) : ""}<span>${escape(where)}</span>${deed.output ? `<span class="ay-card-size">${escape(words.output)} ${escape(words.bytes(deed.output))}</span>` : ""}</p>
      ${touched ? `<ul class="ay-card-files">${touched}</ul>` : ""}`;
  }

  private pressed(event: MouseEvent): void {
    const target = event.target as Element | null;
    if (!target) return;
    const view = target.closest<HTMLElement>("[data-view]")?.dataset.view as View | undefined;
    if (view && view !== this.view) {
      this.view = view;
      keep(KEPT_VIEW, view);
      this.drawSwitcher();
      this.draw();
      return;
    }
    const span = target.closest<HTMLElement>("[data-span]")?.dataset.span as Span | undefined;
    if (span && span !== this.span) {
      this.span = span;
      keep(KEPT_SPAN, span);
      this.draw();
      void this.readSpan();
      return;
    }
    const step = target.closest<HTMLElement>("[data-day-step]")?.dataset.dayStep;
    if (step) {
      const today = dayOf(this.now());
      this.day = Math.min(today, dayOf(this.day + Number(step) * DAY_MS + DAY_MS / 2));
      this.draw();
      return;
    }
    if (target.closest("[data-day-today]")) {
      this.day = dayOf(this.now());
      this.draw();
      return;
    }
    const day = target.closest<SVGElement>("[data-day]")?.dataset.day;
    if (day) {
      this.day = Number(day);
      this.view = "timeline";
      keep(KEPT_VIEW, "timeline");
      this.drawSwitcher();
      this.draw();
      return;
    }
    const bring = target.closest<HTMLElement>("[data-bring]")?.dataset.bring;
    if (bring) {
      this.bringing.add(bring);
      this.draw();
      void this.options.bringIn(bring).then(
        () => void this.readSpan(),
        (error: unknown) => {
          this.bringing.delete(bring);
          console.error("bringing a session in", error);
          this.draw();
        },
      );
      return;
    }
    if (target.closest("[data-card-close]")) {
      this.picked = null;
      this.drawCard();
      return;
    }
    const deed = target.closest<HTMLElement | SVGElement>("[data-job]");
    if (deed?.dataset.job && deed.dataset.machine) {
      const picked = { job: deed.dataset.job, machine: deed.dataset.machine };
      this.picked = this.picked?.job === picked.job && this.picked.machine === picked.machine ? null : picked;
      this.drawCard();
    }
  }
}

/** How a deed went, as a small drawn mark: a ring while it runs, a tick
 *  when done, a cross when it failed, a bar when stopped. */
function mark(deed: Deed): string {
  const kind = deed.outcome === "running" ? "running" : failing(deed.outcome) ? "failed" : deed.outcome === "stopped" ? "stopped" : "done";
  const drawn = kind === "done" ? glyph("check", 14) : kind === "failed" ? glyph("cross", 14) : kind === "stopped" ? glyph("stop", 14) : "";
  return `<span class="ay-mark ay-mark-${kind}" aria-hidden="true">${drawn}</span>`;
}

function iso(at: number): string {
  return new Date(at).toISOString();
}

function kept<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const value = localStorage.getItem(key) as T | null;
    return value && allowed.includes(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

function keep(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not kept in this window.
  }
}
