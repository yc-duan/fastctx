// Each step of a conversation's context, as one large page over it
// (`world-gui.md`, 对话): how the prompt grew from request to request,
// drawn as a ledger filling up — the context so far a pale wash, and on its
// edge what each step added, stacked in the inks of what made it: what the
// tools returned in clay, the model's own answers in graphite, anything
// else in sage. Under it the five calls that took the most room, each a row
// that opens its place in the conversation; a step chosen on the drawing
// lists its own instead, five rows at most, and its list's head leads back.
// The list is always five rows high, so the page keeps its size whatever is
// chosen. A row opened in the conversation leaves the way back to the page
// as it was (`page.ts`). The drawing is a staircase, a step to each
// request, what a step added standing on the stair before it. All of it is
// read from what the conversation holds.

import type { ComposerWords } from "../app/words";
import { Sheet } from "../app/confirm";
import { circle, outline, seeded, swatch, type Point } from "../pen/shapes";
import { contextSteps, heaviestCalls, type CallSpan, type Cause, type ContextStep } from "./context-steps";
import { glyph, type GlyphName } from "./glyphs";
import { escape } from "./prose";
import type { ModelRequest } from "./requests";
import { tokensShort } from "./usage-badge";

type Words = ComposerWords["usage"];

/** How a call reads in one line. */
export interface StepCall {
  glyph: GlyphName;
  action: string;
  object: string | null;
}

export interface StepsInput {
  requests: readonly ModelRequest[];
  calls: readonly CallSpan[];
  /** A call's line, by its key; none for one no longer held. */
  named(key: string): StepCall | null;
}

/** The inks of what grew the context. */
const INKS: Record<Cause["kind"], string> = { call: "var(--pen-clay)", answer: "var(--pen-graphite)", other: "var(--pen-sage)" };
const ORDER: Cause["kind"][] = ["call", "answer", "other"];
/** The ink of where the context began, or what a compaction left. */
const BASE = "color-mix(in srgb, var(--pen-graphite) 32%, transparent)";
/** How many rows the list under the drawing holds, the heaviest calls or a
 *  step's own parts. */
const ROWS = 5;
const f = (n: number) => (Math.round(n * 10) / 10).toString();

export class ContextSheet {
  private readonly sheet = new Sheet("context-sheet");
  private input: StepsInput | null = null;
  private steps: ContextStep[] = [];
  private chosen: number | null = null;
  /** Where each step stands on the drawing, as last drawn. */
  private columns: { x: number; width: number }[] = [];
  private readonly resizing = new ResizeObserver(() => this.drawChart());

  constructor(
    private readonly words: Words,
    private readonly opening: (key: string) => void,
  ) {
    const { body } = this.sheet;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const row = target.closest<HTMLElement>("[data-call]");
      if (row) {
        this.left = this.sheet.body.scrollTop;
        this.sheet.close();
        this.opening(row.dataset.call!);
        return;
      }
      if (target.closest("[data-all]")) return this.choose(null);
    });
    this.sheet.dialog.addEventListener("close", () => this.resizing.disconnect());
  }

  /** Where the page was scrolled to when a row opened the conversation. */
  private left = 0;

  /** Opens it over the conversation, on the steps `input` makes; `again`,
   *  as it was left when a row was opened, the step chosen still chosen. */
  show(input: StepsInput, again = false): void {
    this.input = input;
    this.steps = contextSteps(input.requests, input.calls);
    if (!again || (this.chosen !== null && this.chosen >= this.steps.length)) this.chosen = null;
    const { words } = this;
    this.sheet.body.innerHTML = `
      ${this.sheet.head(words.steps, words.close)}
      <div class="ctx-figures">${this.figures()}</div>
      <div class="ctx-chart"><div class="ctx-paper"></div><div class="ctx-tip" hidden></div></div>
      <div class="ctx-keys">${ORDER.map((kind) => `<span class="ctx-key"><i style="background:${INKS[kind]}"></i>${escape(words.causes[kind])}</span>`).join("")}</div>
      <section class="ctx-list"></section>`;
    this.sheet.show();
    // The page itself takes focus, not its first button: nothing is ringed
    // as it opens.
    this.sheet.dialog.tabIndex = -1;
    this.sheet.dialog.focus({ preventScroll: true });
    const chart = this.sheet.body.querySelector<HTMLElement>(".ctx-chart")!;
    chart.addEventListener("pointermove", (event) => this.point(event));
    chart.addEventListener("pointerleave", () => this.point(null));
    chart.addEventListener("click", (event) => {
      const at = this.stepAt(event);
      if (at === null) return;
      // A step that called something leads to the call that grew it most;
      // one that called nothing shows what it was made of.
      const call = this.steps[at]?.causes.find((cause) => cause.kind === "call");
      if (call?.kind === "call") {
        this.left = this.sheet.body.scrollTop;
        this.sheet.close();
        this.opening(call.key);
        return;
      }
      this.choose(at === this.chosen ? null : at);
    });
    this.resizing.observe(chart);
    this.drawChart();
    this.mark();
    this.drawList();
    this.sheet.body.scrollTop = again ? this.left : 0;
  }

  /** The figures at the top: the context now, and what it holds, each
   *  part with its share of it: where it began (or what a compaction left),
   *  and what grew it since; how many times it was compacted. */
  private figures(): string {
    const { words, steps } = this;
    const last = steps[steps.length - 1];
    if (!last || !last.context) return "";
    const since = steps.reduce((from, step, index) => (step.compacted ? index : from), 0);
    const sums: Record<Cause["kind"], number> = { call: 0, answer: 0, other: 0 };
    for (const step of steps.slice(since + 1)) for (const cause of step.causes) sums[cause.kind] += cause.tokens;
    const base = Math.max(0, last.context - sums.call - sums.answer - sums.other);
    const compactions = steps.filter((step) => step.compacted).length;
    const share = (tokens: number) => Math.round((tokens / last.context) * 100);
    const figure = (value: string, label: string, ink?: string) =>
      `<span class="ctx-figure">${ink ? `<i style="background:${ink}"></i>` : ""}<b>${escape(value)}</b><span>${escape(label)}</span></span>`;
    // A part under a hundredth of it is not worth a figure.
    const parts = [
      { tokens: base, label: compactions ? words.afterCompact : words.start, ink: BASE },
      ...ORDER.map((kind) => ({ tokens: sums[kind], label: words.causes[kind], ink: INKS[kind] })),
    ].filter((part) => share(part.tokens) >= 1);
    return [
      figure(tokensShort(last.context), words.stepsNow),
      ...parts.map((part) => figure(`${tokensShort(part.tokens)} · ${share(part.tokens)}%`, part.label, part.ink)),
      compactions ? figure(String(compactions), words.compactions(compactions)) : "",
    ].join("");
  }

  /** The drawing, at the width it is given: a stair to each request, the
   *  context so far a pale wash under them, the stairs' edge drawn in
   *  pencil, each step's growth stacked on the stair before it, a
   *  compaction a broken stroke down. */
  private drawChart(): void {
    const paper = this.sheet.body.querySelector<HTMLElement>(".ctx-paper");
    if (!paper || !this.steps.length) return;
    const width = Math.max(280, Math.round(paper.clientWidth));
    const height = width < 520 ? 210 : 290;
    const [left, right, top, bottom] = [46, 10, 18, 26];
    const steps = this.steps;
    const most = Math.max(1, ...steps.map((step) => step.context));
    const y = (tokens: number) => height - bottom - (tokens / most) * (height - top - bottom);
    const span = (width - left - right) / steps.length;
    // A hair of paper between two steps' blocks, where they are wide enough.
    const gap = span >= 6 ? Math.min(3, span * 0.18) : 0;
    this.columns = steps.map((_, index) => ({ x: left + index * span, width: span }));
    const middle = (index: number) => left + index * span + span / 2;
    const random = seeded(`ctx:${steps.length}:${steps[0]!.request.at}`);
    // The scale: three ruled values in pencil dots.
    const scale = niceStep(most / 3);
    let grid = "";
    for (let value = scale; value <= most * 1.001; value += scale) {
      const at = y(value);
      grid += `<line class="ctx-grid" x1="${left}" x2="${width - right}" y1="${f(at)}" y2="${f(at)}"/><text class="ctx-scale" x="${left - 8}" y="${f(at + 4)}" text-anchor="end">${escape(tokensShort(value))}</text>`;
    }
    // The context so far: a wash under the stairs' edge.
    const edge: Point[] = steps.flatMap((step, index) => {
      const at = y(step.context);
      return [
        [left + index * span, at],
        [left + (index + 1) * span, at],
      ] as Point[];
    });
    const area = `M${f(left)} ${f(y(0))}L${edge.map(([x, at]) => `${f(x)} ${f(at)}`).join("L")}L${f(width - right)} ${f(y(0))}Z`;
    // Each step's growth on the edge before it, in the inks of what made it.
    let ink = "";
    let marks = "";
    steps.forEach((step, index) => {
      if (index === 0) return;
      const before = steps[index - 1]!.context;
      const x = left + index * span + gap / 2;
      if (step.compacted) {
        const from = y(before);
        const to = y(step.context);
        const at = left + index * span;
        for (let down = from + 3; down < to - 3; down += 8) marks += `<path class="ctx-compact" d="${outline([[at, down], [at, Math.min(to - 3, down + 4.5)]], 1.7, random)}"/>`;
        marks += `<text class="ctx-compact-said" x="${f(at)}" y="${f(from - 7)}" text-anchor="middle">${escape(this.words.compacted)}</text>`;
        return;
      }
      let base = before;
      for (const kind of ORDER) {
        const tokens = step.causes.filter((cause) => cause.kind === kind).reduce((sum, cause) => sum + cause.tokens, 0);
        if (tokens <= 0) continue;
        const lower = y(base);
        const upper = y(base + tokens);
        if (lower - upper >= 0.8) ink += `<path fill="${INKS[kind]}" d="${swatch(x, upper, Math.max(1.5, span - gap), lower - upper, random, Math.min(2, span / 4))}"/>`;
        base += tokens;
      }
    });
    const line = outline(edge, 1.4, random, { pressure: 0.18, taper: 0.02 });
    const last: Point = [middle(steps.length - 1), y(steps[steps.length - 1]!.context)];
    paper.innerHTML = `<svg class="ctx-svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-label="${escape(this.words.steps)}">
      <g class="ctx-grids">${grid}</g>
      <path class="ctx-wash" d="${area}"/>
      <rect class="ctx-band" y="${top - 6}" height="${height - top - bottom + 6}" x="0" width="0" visibility="hidden"/>
      <rect class="ctx-chosen" y="${top - 6}" height="${height - top - bottom + 6}" x="0" width="0" visibility="hidden"/>
      <g filter="url(#pen-marker-lite)">${ink}</g>
      <path class="ctx-edge" d="${line}"/>
      <path class="ctx-now" d="${circle(last, 3.2, 1.4, random)}"/>
      ${marks}
      <text class="ctx-axis" x="${left}" y="${height - 6}">1</text>
      <text class="ctx-axis" x="${width - right}" y="${height - 6}" text-anchor="end">${steps.length}</text>
    </svg>`;
    this.mark();
  }

  /** The step under the pointer, by where it is across the drawing. */
  private stepAt(event: MouseEvent): number | null {
    const svg = this.sheet.body.querySelector<SVGSVGElement>(".ctx-svg");
    if (!svg || !this.columns.length) return null;
    const x = event.clientX - svg.getBoundingClientRect().left;
    const index = this.columns.findIndex((column) => x >= column.x && x < column.x + column.width);
    return index >= 0 ? index : null;
  }

  /** A step under the pointer: a pale band behind it, and what it added
   *  beside it. */
  private point(event: PointerEvent | null): void {
    const tip = this.sheet.body.querySelector<HTMLElement>(".ctx-tip");
    const band = this.sheet.body.querySelector<SVGRectElement>(".ctx-band");
    if (!tip || !band) return;
    const index = event ? this.stepAt(event) : null;
    if (index === null) {
      tip.hidden = true;
      band.setAttribute("visibility", "hidden");
      return;
    }
    const column = this.columns[index]!;
    band.setAttribute("x", f(column.x));
    band.setAttribute("width", f(Math.max(3, column.width)));
    band.setAttribute("visibility", "visible");
    tip.innerHTML = this.tipOf(this.steps[index]!);
    tip.hidden = false;
    const chart = tip.parentElement!;
    const box = chart.getBoundingClientRect();
    const across = event!.clientX - box.left;
    const room = box.width - tip.offsetWidth - 8;
    tip.style.left = `${Math.max(8, Math.min(room, across + 14 > room ? across - tip.offsetWidth - 14 : across + 14))}px`;
  }

  /** What one step added, as the pointer reads it: its place, the context
   *  it was answered over, how much larger, and the largest parts. */
  private tipOf(step: ContextStep): string {
    const { words } = this;
    const grew = step.index === 0 ? "" : step.grew >= 0 ? `+${tokensShort(step.grew)}` : `−${tokensShort(-step.grew)}`;
    const head = `<p class="ctx-tip-head">${escape(words.stepSaid(step.index + 1, tokensShort(step.context), grew))}</p>`;
    if (step.compacted) return `${head}<p class="ctx-tip-row">${escape(words.compacted)}</p>`;
    const rows = step.causes.slice(0, 5).map((cause) => `<p class="ctx-tip-row"><i style="background:${INKS[cause.kind]}"></i><span>${escape(this.causeName(cause))}</span><b>+${escape(tokensShort(cause.tokens))}</b></p>`);
    return head + rows.join("");
  }

  private causeName(cause: Cause): string {
    if (cause.kind !== "call") return this.words.causes[cause.kind];
    const call = this.input?.named(cause.key);
    return call ? [call.action, call.object].filter(Boolean).join(" ") : this.words.causes.call;
  }

  private choose(index: number | null): void {
    this.chosen = index;
    this.mark();
    this.drawList();
  }

  /** The step chosen, washed behind. */
  private mark(): void {
    const chosen = this.sheet.body.querySelector<SVGRectElement>(".ctx-chosen");
    if (!chosen) return;
    const column = this.chosen === null ? null : this.columns[this.chosen];
    chosen.setAttribute("visibility", column ? "visible" : "hidden");
    if (column) {
      chosen.setAttribute("x", f(column.x));
      chosen.setAttribute("width", f(Math.max(3, column.width)));
    }
  }

  /** Under the drawing: the heaviest calls of the whole conversation, or
   *  what the step chosen was made of, the largest first; past five rows
   *  the rest of a step's are one row, how many and how much. */
  private drawList(): void {
    const list = this.sheet.body.querySelector<HTMLElement>(".ctx-list");
    if (!list || !this.input) return;
    const { words } = this;
    const step = this.chosen === null ? null : this.steps[this.chosen]!;
    let rows: { cause: Cause; step: number }[];
    let rest: { count: number; tokens: number } | null = null;
    if (step) {
      const causes = [...step.causes].sort((a, b) => b.tokens - a.tokens);
      const shown = causes.length > ROWS ? causes.slice(0, ROWS - 1) : causes;
      const others = causes.slice(shown.length);
      if (others.length) rest = { count: others.length, tokens: others.reduce((sum, cause) => sum + cause.tokens, 0) };
      rows = shown.map((cause) => ({ cause, step: step.index }));
    } else {
      rows = heaviestCalls(this.steps, ROWS).map((call) => ({ cause: { kind: "call" as const, key: call.key, tokens: call.tokens }, step: call.step }));
    }
    const most = Math.max(1, ...rows.map((row) => row.cause.tokens));
    const title = step ? words.stepSaid(step.index + 1, tokensShort(step.context), step.grew > 0 ? `+${tokensShort(step.grew)}` : "") : words.heaviest;
    const body = rows.length
      ? rows.map((row) => this.row(row.cause, row.step, most, !step)).join("") + (rest ? this.restRow(rest) : "")
      : `<p class="ctx-none">${escape(step?.compacted ? words.compacted : words.noCalls)}</p>`;
    // A step's own list leads back from its head, where the eye is.
    const back = step ? `<button class="text-button ctx-back" type="button" data-all>${glyph("chevron", 14)}<span>${escape(words.backToHeaviest)}</span></button>` : "";
    list.innerHTML = `<header class="ctx-list-head">${back}<h3 class="picker-title"${step ? "" : ` title="${escape(words.heaviestWhy)}"`}>${escape(title)}</h3></header><div class="ctx-rows">${body}</div>`;
  }

  /** What a step held past the rows shown, as one quiet row. */
  private restRow(rest: { count: number; tokens: number }): string {
    return `<div class="ctx-row ctx-rest"><span class="ctx-row-mark"></span><span class="ctx-row-what">${escape(this.words.rest(rest.count))}</span><span class="ctx-row-step"></span><span class="ctx-row-ink"></span><span class="ctx-row-tokens">+${escape(tokensShort(rest.tokens))}</span></div>`;
  }

  /** One row: what it was, which step (in the whole list), a bar of its
   *  share, and how much. A call's row opens it in the conversation. */
  private row(cause: Cause, step: number, most: number, steps: boolean): string {
    const { words } = this;
    const call = cause.kind === "call" ? this.input?.named(cause.key) : null;
    const mark = call ? glyph(call.glyph, 16) : `<i class="ctx-dot" style="background:${INKS[cause.kind]}"></i>`;
    const what = call ? `<b>${escape(call.action)}</b>${call.object ? ` <span>${escape(call.object)}</span>` : ""}` : `<b>${escape(words.causes[cause.kind])}</b>`;
    const share = Math.max(0.02, cause.tokens / most);
    const random = seeded(`ctx-row:${cause.kind}:${"key" in cause ? cause.key : step}`);
    const ink = `<svg class="ctx-ink" viewBox="0 0 100 10" preserveAspectRatio="none" aria-hidden="true"><path fill="${INKS[cause.kind]}" d="${swatch(1, 2, Math.max(1.5, share * 98), 6, random, 2)}"/></svg>`;
    // The step's own list keeps the step's place empty, so the columns stay.
    const inner = `<span class="ctx-row-mark">${mark}</span><span class="ctx-row-what">${what}</span><span class="ctx-row-step">${steps ? escape(words.step(step + 1)) : ""}</span><span class="ctx-row-ink">${ink}</span><span class="ctx-row-tokens">+${escape(tokensShort(cause.tokens))}</span>`;
    return call && "key" in cause ? `<button class="ctx-row" type="button" data-call="${escape(cause.key)}">${inner}</button>` : `<div class="ctx-row">${inner}</div>`;
  }
}

/** A round step for a scale of about `rough`: 1, 2 or 5 of a power of ten. */
function niceStep(rough: number): number {
  const power = 10 ** Math.floor(Math.log10(Math.max(1, rough)));
  const scaled = rough / power;
  return (scaled <= 1 ? 1 : scaled <= 2 ? 2 : scaled <= 5 ? 5 : 10) * power;
}
