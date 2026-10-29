// A watcher's own page (`world-gui.md`, 观测): what it waits for, in its own
// words at the top; where it lives and how it stands beneath, in a line;
// then its time drawn across the page (`strip.ts`); and below, all it did,
// read as a conversation is, each wake a turn (`record.ts`). The top stays
// where it is; only the record scrolls, and the wake being read is marked
// on the strip. Nothing is said to it: the user reads, and may withdraw it.

import type { Page } from "../app/shell";
import { glyph, type GlyphName } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Context } from "../conversation/view";
import type { MachineName } from "../world/events";
import type { SentinelPhase, SentinelRecord, SentinelSummary } from "../world/sentinels";
import type { Words } from "../words";
import { RecordColumn } from "./record";
import { stripMarkup, wakesOf, type Wake } from "./strip";
import type { SentinelWords } from "./words";
import "./sentinel.css";

export interface SentinelPageOptions {
  words: SentinelWords;
  /** The conversation's words, for its record drawn as one. */
  talkWords: Words;
  english: boolean;
  here: MachineName;
  head(title: string, actions?: string): string;
  sentinel: { id: string; home: string; generation: number };
  /** How it stood when last listed, drawn until its record is read. */
  known: SentinelSummary | null;
  read(held: string | null): Promise<SentinelRecord>;
  /** Withdraws it; absent where it cannot be done from here. */
  withdraw?: (() => Promise<void>) | undefined;
  confirm: { ask(asked: { title: string; what: string; go: string; gentle?: boolean; act: () => Promise<void>; done: () => void }): void };
  /** The mark of a machine's form. */
  form(machine: string): GlyphName;
  lines?: Context["lines"] | undefined;
  now(): number;
  /** The World's words for why something failed, for the pointer. */
  detail(error: unknown): string;
}

/** How tall the strip is drawn, across its width. */
const STRIP = 56;

export class SentinelPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly column: RecordColumn;
  private readonly strip: SVGSVGElement;
  private readonly scroller: HTMLElement;
  private readonly note: HTMLElement;
  private readonly resize: ResizeObserver;
  private summary: SentinelSummary | null;
  private record: SentinelRecord | null = null;
  private wakes: Wake[] = [];
  private width = 0;
  private clock = 0;
  /** The strip was drawn in once; drawn again, it is not drawn in again. */
  private inked = false;
  private withdrawn = false;

  constructor(private readonly options: SentinelPageOptions) {
    const { words } = options;
    this.title = words.title;
    this.summary = options.known;
    this.element = document.createElement("section");
    this.element.className = "sn-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`, `<span class="sn-acts"></span>`)}
      <div class="sn-top">
        <h2 class="sn-condition"></h2>
        <p class="sn-facts"><span class="sn-home"></span><span class="sn-phase"></span></p>
        <svg class="sn-strip" height="${STRIP}" role="group"></svg>
      </div>
      <div class="sn-scroll scroller"><p class="sn-note" hidden></p></div>`;
    this.strip = this.element.querySelector<SVGSVGElement>(".sn-strip")!;
    this.scroller = this.element.querySelector<HTMLElement>(".sn-scroll")!;
    this.note = this.element.querySelector<HTMLElement>(".sn-note")!;
    this.drawFacts();
    this.column = new RecordColumn(this.scroller, {
      words: options.talkWords,
      english: options.english,
      here: options.here,
      home: options.sentinel.home,
      read: options.read,
      changed: (record) => {
        this.record = record;
        if (record.summary) this.summary = record.summary;
        this.wakes = wakesOf(record.events);
        this.drawFacts();
        this.drawStrip();
        this.say(!this.column.drawn ? words.nothing : record.earlier ? words.earlier : null);
      },
      failed: (error) => {
        if (!this.column.drawn) this.say(words.unreadable, options.detail(error));
      },
      moved: () => this.moved(),
      ...(options.lines ? { lines: options.lines } : {}),
    });
    if (!this.record) this.say(words.reading);
    this.resize = new ResizeObserver(() => {
      const width = Math.round(this.strip.getBoundingClientRect().width);
      if (width && width !== this.width) {
        this.width = width;
        this.drawStrip();
      }
    });
    this.resize.observe(this.strip);
    // Now moves along the strip.
    this.clock = window.setInterval(() => this.drawStrip(), 30_000);
    this.strip.addEventListener("click", this.pressed);
    this.strip.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") this.pressed(event);
    });
    this.strip.addEventListener("animationend", () => this.strip.classList.remove("sn-inking"));
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".sn-withdraw")) this.askWithdraw();
    });
  }

  leave(): void {
    clearInterval(this.clock);
    this.resize.disconnect();
    this.column.destroy();
  }

  private phase(): SentinelPhase | null {
    if (this.withdrawn) return "withdrawn";
    return this.summary?.phase ?? null;
  }

  /** What it waits for, where it lives and how it stands; its plan, and why
   *  it could not watch, where the pointer rests on that line; the way to
   *  withdraw it while it watches. */
  private drawFacts(): void {
    const { words, sentinel } = this.options;
    const summary = this.summary;
    const condition = this.element.querySelector<HTMLElement>(".sn-condition")!;
    const said = summary?.definition.condition || summary?.plan || "";
    if (condition.textContent !== said) condition.textContent = said;
    condition.title = said;
    const home = `${glyph(this.options.form(sentinel.home), 16)}<span>${escape(sentinel.home)}</span>`;
    const homeElement = this.element.querySelector<HTMLElement>(".sn-home")!;
    if (homeElement.innerHTML !== home) homeElement.innerHTML = home;
    const phase = this.phase();
    const phaseElement = this.element.querySelector<HTMLElement>(".sn-phase")!;
    const mark = phase === "satisfied" ? "check" : "sentinel";
    const shown = phase ? `${glyph(mark, 16)}<span>${escape(words.phase[phase])}</span>` : "";
    if (phaseElement.innerHTML !== shown) phaseElement.innerHTML = shown;
    phaseElement.dataset.phase = phase ?? "";
    const facts = this.element.querySelector<HTMLElement>(".sn-facts")!;
    facts.title = [summary?.plan, phase === "unable" || phase === "unavailable" ? summary?.reason : null].filter(Boolean).join("\n\n");
    const watching = phase === "starting" || phase === "watching" || phase === "unavailable";
    const acts = this.element.querySelector<HTMLElement>(".sn-acts")!;
    const act = watching && this.options.withdraw ? `<button class="text-button sn-withdraw" type="button">${escape(words.withdraw)}</button>` : "";
    if (acts.innerHTML !== act) acts.innerHTML = act;
  }

  private drawStrip(): void {
    if (!this.width) return;
    const { words } = this.options;
    const now = this.options.now();
    const phase = this.phase();
    this.strip.setAttribute("viewBox", `0 0 ${this.width} ${STRIP}`);
    this.strip.innerHTML = stripMarkup({
      wakes: this.wakes,
      now,
      next: this.record?.next ?? null,
      found: phase === "satisfied",
      over: phase === "satisfied" || phase === "unable" || phase === "withdrawn",
      width: this.width,
      height: STRIP,
      seed: this.options.sentinel.id,
      said: {
        at: (time) => words.at(time, now),
        next: (time) => words.next(time, now),
        every: (ms) => words.every(ms),
        woke: (why) => this.options.talkWords.woke(why),
      },
    });
    // Its time is drawn in once, from its first look on, when first read.
    if (!this.inked && this.wakes.length) {
      this.inked = true;
      this.strip.classList.add("sn-inking");
    }
    this.moved();
  }

  /** The wake being read, marked under it on the strip. */
  private moved(): void {
    const turn = this.column?.inView() ?? null;
    for (const mark of this.strip.querySelectorAll<SVGElement>(".sn-here")) mark.classList.toggle("on", mark.dataset.here === turn);
    this.element.classList.toggle("scrolled", this.scroller.scrollTop > 0);
  }

  /** A wake pressed on the strip: its turn is brought into view. */
  private pressed = (event: Event): void => {
    const turn = (event.target as Element).closest<SVGElement>("[data-turn]")?.dataset.turn;
    if (!turn) return;
    event.preventDefault();
    this.column.show(turn);
  };

  private askWithdraw(): void {
    const { words, withdraw, confirm } = this.options;
    if (!withdraw) return;
    confirm.ask({
      title: words.withdrawTitle,
      what: words.withdrawWhat,
      go: words.withdraw,
      act: withdraw,
      done: () => {
        this.withdrawn = true;
        this.drawFacts();
        this.drawStrip();
      },
    });
  }

  private say(said: string | null, why = ""): void {
    this.note.hidden = !said;
    if (this.note.textContent !== (said ?? "")) this.note.textContent = said ?? "";
    this.note.title = why;
    this.note.classList.toggle("sn-alone", !this.column?.drawn);
  }
}
