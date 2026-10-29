// Every watcher of the user's accounts on one page (`world-gui.md`, 观测):
// each a line across the same span of days, a blot where it woke, the
// clay ring where what it watched for came true; those watching above,
// those that ended beneath, fainter. What they all took in the span heads
// the page; one's own is a hover away, and a line opens that watcher's
// page. Only the list scrolls.

import type { Page } from "../app/shell";
import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { blot, circle, ruled, seeded, type Point } from "../pen/shapes";
import type { SentinelOverview, SentinelWake, SentinelWatched } from "../world/sentinels";
import type { Words } from "../words";
import type { SentinelWords } from "./words";
import "./overview.css";

export interface OverviewOptions {
  words: SentinelWords;
  /** The conversation's words: why a watcher woke. */
  talkWords: Words;
  english: boolean;
  head(title: string, actions?: string): string;
  read(): Promise<SentinelOverview>;
  /** A watcher's own page. */
  link(watched: SentinelWatched): string;
  /** The mark of a machine's form. */
  form(machine: string): GlyphName;
  now(): number;
  /** The World's words for why something failed, for the pointer. */
  detail(error: unknown): string;
}

type Span = "week" | "month";
const SPANS: Record<Span, number> = { week: 7 * 86_400_000, month: 30 * 86_400_000 };
/** Where this device remembers the span the page was last drawn over. */
const SPAN_KEY = "watchers.span";
/** How often the page reads them again while it is open: they wake seldom. */
const EVERY_MS = 60_000;
const LANE = 22;
const PAD = 8;
/** The least room between two days said over the lines. */
const TICK_GAP = 56;

export class SentinelOverviewPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly totals: HTMLElement;
  private readonly axis: HTMLElement;
  private readonly note: HTMLElement;
  private readonly resize: ResizeObserver;
  private overview: SentinelOverview | null = null;
  private span: Span = spanKept();
  private width = 0;
  private timer = 0;
  private left = false;

  constructor(private readonly options: OverviewOptions) {
    const { words } = options;
    this.title = words.overview.title;
    this.element = document.createElement("section");
    this.element.className = "so-page";
    const spans = (Object.keys(SPANS) as Span[])
      .map((span) => `<button class="text-button so-span" type="button" data-span="${span}" aria-pressed="false">${escape(words.overview.span[span])}</button>`)
      .join("");
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.overview.title)}</h1>`)}
      <div class="so-top"><p class="so-totals"></p><div class="so-spans" role="group">${spans}</div></div>
      <div class="so-axis" aria-hidden="true"></div>
      <div class="so-scroll scroller"><p class="so-note" hidden></p><div class="so-list"></div></div>`;
    this.list = this.element.querySelector<HTMLElement>(".so-list")!;
    this.totals = this.element.querySelector<HTMLElement>(".so-totals")!;
    this.axis = this.element.querySelector<HTMLElement>(".so-axis")!;
    this.note = this.element.querySelector<HTMLElement>(".so-note")!;
    this.say(words.reading);
    this.resize = new ResizeObserver(() => {
      const width = Math.round(this.list.getBoundingClientRect().width);
      if (width && width !== this.width) {
        this.width = width;
        this.draw();
      }
    });
    this.resize.observe(this.list);
    this.element.addEventListener("click", (event) => {
      const span = (event.target as HTMLElement).closest<HTMLElement>("[data-span]");
      if (!span) return;
      this.span = span.dataset.span as Span;
      keepSpan(this.span);
      this.draw();
    });
    void this.read();
  }

  leave(): void {
    this.left = true;
    clearTimeout(this.timer);
    this.resize.disconnect();
  }

  private async read(): Promise<void> {
    try {
      this.overview = await this.options.read();
      this.draw();
    } catch (error) {
      if (!this.overview) this.say(this.options.words.unreadable, this.options.detail(error));
    }
    if (!this.left) this.timer = window.setTimeout(() => void this.read(), EVERY_MS);
  }

  private say(text: string | null, why = ""): void {
    this.note.hidden = !text;
    this.note.textContent = text ?? "";
    this.note.title = why;
  }

  private draw(): void {
    const { words } = this.options;
    for (const button of this.element.querySelectorAll<HTMLElement>("[data-span]")) button.setAttribute("aria-pressed", String(button.dataset.span === this.span));
    const overview = this.overview;
    if (!overview || !this.width) return;
    const now = this.options.now();
    const from = now - SPANS[this.span];
    // A goal's watcher is the goal's own; the user left none of them.
    const shown = overview.watched.filter((one) => one.summary.definition.kind !== "goal");
    const watching = shown.filter((one) => live(one)).sort(byLast);
    const ended = shown.filter((one) => !live(one) && one.wakes.some((wake) => wake.at >= from)).sort(byLast);
    const inSpan = shown.flatMap((one) => one.wakes.filter((wake) => wake.at >= from));
    const tokens = inSpan.reduce((sum, wake) => sum + wake.input + wake.output, 0);
    this.totals.innerHTML = inSpan.length
      ? `<span>${escape(words.overview.wakes(inSpan.length))}</span><span>${escape(words.overview.tokens(count(tokens)))}</span>`
      : "";
    const unreachable = overview.unavailable.length ? words.overview.unreachable : null;
    this.say(!watching.length && !ended.length ? (unreachable ?? words.overview.nothing) : unreachable, overview.unavailable.join("\n"));
    const part = (label: string, rows: SentinelWatched[], over: boolean) =>
      rows.length ? `<section class="so-part${over ? " over" : ""}"><h2 class="so-part-title">${escape(label)}</h2><ul class="so-rows">${rows.map((one) => this.row(one, from, now, over)).join("")}</ul></section>` : "";
    const html = part(words.overview.watching, watching, false) + part(words.overview.ended, ended, true);
    if (this.list.innerHTML !== html) this.list.innerHTML = html;
    this.drawAxis(from, now, !!html);
  }

  /** The days the lines run across, over them: each day of a week, each
   *  week of a month, where its lines would cross it. */
  private drawAxis(from: number, now: number, shown: boolean): void {
    const step = this.span === "week" ? 1 : 7;
    const ticks: number[] = [];
    const day = new Date(now);
    day.setHours(0, 0, 0, 0);
    while (day.getTime() > from) {
      ticks.unshift(day.getTime());
      day.setDate(day.getDate() - step);
    }
    const x = (at: number) => PAD + ((at - from) / (now - from)) * (this.width - 2 * PAD);
    const locale = this.options.english ? "en-US" : "zh-CN";
    // A day too near the edge, or to the last one said, goes unsaid.
    let last = -Infinity;
    const said = ticks.filter((at) => {
      const place = x(at);
      if (place < 24 || place > this.width - 24 || place - last < TICK_GAP) return false;
      last = place;
      return true;
    });
    const html = shown
      ? said
          .map((at) => `<span class="so-tick" style="left: ${Math.round(x(at))}px">${escape(new Date(at).toLocaleDateString(locale, { month: "numeric", day: "numeric" }))}</span>`)
          .join("")
      : "";
    if (this.axis.innerHTML !== html) this.axis.innerHTML = html;
  }

  /** One watcher: what it waits for, where it lives and how it stands, and
   *  its line across the span. */
  private row(one: SentinelWatched, from: number, now: number, over: boolean): string {
    const { words, talkWords } = this.options;
    const { definition, phase } = one.summary;
    const said = definition.condition || one.summary.plan;
    const wakes = one.wakes.filter((wake) => wake.at >= from);
    const tokens = wakes.reduce((sum, wake) => sum + wake.input + wake.output, 0);
    const time = wakes.reduce((sum, wake) => sum + wake.elapsed_ms, 0);
    const took = wakes.length ? words.overview.took(wakes.length, count(tokens), duration(time, talkWords)) : "";
    const mark = phase === "satisfied" ? "check" : "sentinel";
    const lane = laneMarkup(one, from, now, this.width, over, (wake) => `${words.at(wake.at, now)} ${woke(wake, talkWords)}`);
    return `<li class="so-row" data-phase="${phase}"><a class="so-link" href="${this.options.link(one)}" title="${escape(took)}">
      <span class="so-head"><span class="so-what">${escape(said)}</span><span class="so-where">${glyph(this.options.form(definition.home), 14)}<span>${escape(definition.home)}</span></span><span class="so-phase">${glyph(mark, 14)}<span>${escape(words.phase[phase])}</span></span></span>
      <svg class="so-lane" viewBox="0 0 ${this.width} ${LANE}" height="${LANE}" aria-hidden="true">${lane}</svg>
    </a></li>`;
  }
}

function live(one: SentinelWatched): boolean {
  return one.summary.phase === "starting" || one.summary.phase === "watching" || one.summary.phase === "unavailable";
}

function byLast(a: SentinelWatched, b: SentinelWatched): number {
  return (b.wakes.at(-1)?.at ?? 0) - (a.wakes.at(-1)?.at ?? 0);
}

/** A watcher's line across the span: the pencil line of its time there, a
 *  blot for each wake, its first a ring, a wake that went wrong in the
 *  caution colour, and the clay ring of the wake that found what it waited
 *  for. Seeded by the watcher, so it is drawn the same each time. */
function laneMarkup(one: SentinelWatched, from: number, now: number, width: number, over: boolean, said: (wake: SentinelWake) => string): string {
  const random = seeded(`lane:${one.summary.definition.id}`);
  const x = (at: number) => PAD + ((Math.max(from, Math.min(now, at)) - from) / (now - from)) * (width - 2 * PAD);
  const y = LANE / 2;
  const first = one.wakes[0]?.at ?? now;
  const last = one.wakes.at(-1)?.at ?? now;
  const start = Math.max(first, from);
  const end = over ? last : now;
  const parts: string[] = [];
  if (end > start) parts.push(`<path class="so-line" d="${ruled([x(start), y], [x(end), y], 1.1, random)}"/>`);
  one.wakes.forEach((wake, index) => {
    if (wake.at < from) return;
    const at: Point = [x(wake.at), y];
    const found = over && one.summary.phase === "satisfied" && index === one.wakes.length - 1;
    const kind = found ? "found" : wake.failed || wake.why.kind === "broken" ? "wrong" : index === 0 && wake.why.kind === "started" ? "begun" : "wake";
    const shape = kind === "found" || kind === "begun" ? circle(at, kind === "found" ? 5 : 3.2, 1.2, random) : blot(at, 2.4, random);
    parts.push(`<path class="so-${kind}" d="${shape}"><title>${escape(said(wake))}</title></path>`);
  });
  return parts.join("");
}

/** Why a watcher woke, in the conversation's words. */
function woke(wake: SentinelWake, words: Words): string {
  return words.woke(wake.why);
}

/** A count of tokens as a person reads it. */
function count(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}k`;
  return String(tokens);
}

function duration(ms: number, words: Words): string {
  return words.took(ms);
}

function spanKept(): Span {
  try {
    const kept = localStorage.getItem(SPAN_KEY);
    return kept === "month" ? "month" : "week";
  } catch {
    return "week";
  }
}

function keepSpan(span: Span): void {
  try {
    localStorage.setItem(SPAN_KEY, span);
  } catch {
    // A device that keeps nothing draws the week next time.
  }
}
