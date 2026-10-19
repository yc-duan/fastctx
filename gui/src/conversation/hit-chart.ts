// How much of each request's prompt the provider's cache served, across a
// conversation (`world-gui.md`, 对话), drawn as plainly as a chart can be:
// one thin line, a request a point, on the stretch chosen, between two
// hairlines at the stretch's own highest and lowest, their figures at its
// edge and its times below; the latest request a dot. Resting on it, a
// hairline stands at the request under the pointer, a dot on the line, and a
// small label says how much the cache served and when. Under it, always,
// the whole conversation as a fainter line, the stretch shown between two
// short bars and the rest under a veil: dragged, stretched by its bars, or set
// anew with a press; a double press shows it all again. Many requests to a
// pixel keep their lowest and highest, so a request that missed the cache is
// never smoothed away.

import { escape } from "./prose";
import { contextOf, hitOf, type ModelRequest } from "./requests";
import { percentFine } from "./usage-badge";

export interface ChartWords {
  highest: string;
  lowest: string;
  readout(hit: string, time: string): string;
  stretch: string;
}

const HEIGHT = 120;
const LEFT = 46;
const RIGHT = 8;
const TOP = 12;
const BOTTOM = 24;
const BRUSH = 26;

/** Requests drawn as one point: the lowest and highest of them kept. */
interface Point2 {
  first: number;
  last: number;
  lo: number;
  hi: number;
  hit: number;
  at: number;
  context: number;
}

export class HitChart {
  readonly element: HTMLElement;
  private readonly main: SVGSVGElement;
  private readonly brush: SVGSVGElement;
  private readonly readout: HTMLElement;
  private requests: readonly ModelRequest[] = [];
  /** The stretch shown, as shares of the whole; null shows all of it. */
  private range: [number, number] | null = null;
  private points: Point2[] = [];
  /** Where each point is drawn, and the plot's height, for the pointer. */
  private placed: { x: number[]; y: number[] } = { x: [], y: [] };
  private drawn = "";
  private dragging: { mode: "move" | "start" | "end" | "new"; from: number; range: [number, number] } | null = null;

  constructor(
    private readonly words: ChartWords,
    /** The requests of the stretch shown, whenever it changes. */
    private readonly chosen: (requests: readonly ModelRequest[]) => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = "talk-chart";
    this.element.innerHTML = `<svg class="talk-chart-main" aria-hidden="true"></svg><div class="talk-chart-readout" hidden></div><svg class="talk-chart-brush" role="slider" tabindex="0" aria-label="${escape(words.stretch)}"></svg>`;
    this.main = this.element.querySelector<SVGSVGElement>(".talk-chart-main")!;
    this.brush = this.element.querySelector<SVGSVGElement>(".talk-chart-brush")!;
    this.readout = this.element.querySelector<HTMLElement>(".talk-chart-readout")!;
    this.main.addEventListener("pointermove", (event) => this.read(event));
    this.main.addEventListener("pointerleave", () => this.read(null));
    this.brush.addEventListener("pointerdown", (event) => this.grab(event));
    this.brush.addEventListener("pointermove", (event) => this.drag(event));
    this.brush.addEventListener("pointerup", (event) => this.let(event));
    this.brush.addEventListener("pointercancel", (event) => this.let(event));
    this.brush.addEventListener("dblclick", () => this.show(null));
    this.brush.addEventListener("keydown", (event) => this.key(event));
  }

  /** Draws `requests`, keeping the stretch chosen; one that reached the
   *  end follows new requests as they come. */
  update(requests: readonly ModelRequest[]): void {
    this.requests = requests;
    this.draw();
  }

  /** The requests of the stretch shown. */
  visible(): readonly ModelRequest[] {
    const [start, end] = this.indices();
    return this.requests.slice(start, end + 1);
  }

  private indices(): [number, number] {
    const last = Math.max(0, this.requests.length - 1);
    if (!this.range) return [0, last];
    const start = Math.max(0, Math.min(last, Math.floor(this.range[0] * last)));
    const end = Math.max(start, Math.min(last, Math.ceil(this.range[1] * last)));
    return [start, end];
  }

  private show(range: [number, number] | null): void {
    this.range = range && (range[0] > 0.0001 || range[1] < 0.9999) ? range : null;
    this.drawn = "";
    this.draw();
    this.chosen(this.visible());
  }

  private width(): number {
    return Math.max(220, Math.round(this.element.clientWidth || 440));
  }

  private draw(): void {
    const width = this.width();
    const [start, end] = this.indices();
    const last = this.requests[this.requests.length - 1];
    const key = `${width}:${this.requests.length}:${last?.at}:${start}:${end}`;
    if (key === this.drawn) return;
    this.drawn = key;
    this.drawMain(width, start, end);
    this.drawBrush(width);
  }

  private drawMain(width: number, start: number, end: number): void {
    const { words } = this;
    const rows = this.requests.slice(start, end + 1);
    const plotW = width - LEFT - RIGHT;
    const plotH = HEIGHT - TOP - BOTTOM;
    this.main.setAttribute("viewBox", `0 0 ${width} ${HEIGHT}`);
    this.main.setAttribute("width", String(width));
    this.main.setAttribute("height", String(HEIGHT));
    this.points = gather(rows, Math.max(1, Math.floor(plotW / 2)), start);
    if (!this.points.length) {
      this.main.innerHTML = "";
      this.placed = { x: [], y: [] };
      return;
    }
    let lo = Math.min(...this.points.map((point) => point.lo));
    let hi = Math.max(...this.points.map((point) => point.hi));
    // A line flat to a hundredth of a percent stands in the middle.
    if (hi - lo < 0.0002) {
      const middle = (hi + lo) / 2;
      lo = Math.max(0, middle - 0.0001);
      hi = Math.min(1, middle + 0.0001);
    }
    const span = hi - lo;
    const count = this.points.length;
    const x = (index: number) => LEFT + (count > 1 ? (index / (count - 1)) * plotW : plotW / 2);
    const y = (share: number) => TOP + ((hi - share) / span) * plotH;
    this.placed = { x: this.points.map((_, index) => x(index)), y: this.points.map((point) => y(point.hit)) };
    // A request that stands for several goes down to their lowest and up to
    // their highest, so a miss among them shows.
    const line: string[] = [];
    const cell = count > 1 ? plotW / (count - 1) : 0;
    this.points.forEach((point, index) => {
      if (point.lo === point.hi) line.push(`${f(x(index))},${f(y(point.hit))}`);
      else line.push(`${f(x(index) - cell * 0.3)},${f(y(point.lo))}`, `${f(x(index))},${f(y(point.hi))}`, `${f(x(index) + cell * 0.3)},${f(y(point.hit))}`);
    });
    const axis = (share: number) => axisPercent(share, span);
    const first = rows[0]!.at;
    const final = rows[rows.length - 1]!.at;
    const time = (when: number) => clock(when, first, final);
    const middle = rows[Math.floor((rows.length - 1) / 2)]!.at;
    const bottom = TOP + plotH;
    // A single request is said by the figure above; its height needs no scale.
    const labels =
      count > 1
        ? `<text class="talk-chart-axis" x="${LEFT - 8}" y="${f(TOP + 4)}" text-anchor="end"><title>${escape(words.highest)}</title>${axis(hi)}</text>` +
          `<text class="talk-chart-axis" x="${LEFT - 8}" y="${f(bottom + 4)}" text-anchor="end"><title>${escape(words.lowest)}</title>${axis(lo)}</text>`
        : "";
    const times = [
      `<text class="talk-chart-axis" x="${count > 1 ? LEFT : f(LEFT + plotW / 2)}" y="${HEIGHT - 4}"${count > 1 ? "" : ' text-anchor="middle"'}>${escape(time(first))}</text>`,
      plotW > 300 && rows.length > 2 ? `<text class="talk-chart-axis" x="${f(LEFT + plotW / 2)}" y="${HEIGHT - 4}" text-anchor="middle">${escape(time(middle))}</text>` : "",
      rows.length > 1 ? `<text class="talk-chart-axis" x="${width - RIGHT}" y="${HEIGHT - 4}" text-anchor="end">${escape(time(final))}</text>` : "",
    ].join("");
    const latest = count - 1;
    this.main.innerHTML = `
      <line class="talk-chart-rule" x1="${LEFT}" x2="${LEFT + plotW}" y1="${TOP}" y2="${TOP}"/>
      <line class="talk-chart-rule" x1="${LEFT}" x2="${LEFT + plotW}" y1="${bottom}" y2="${bottom}"/>
      ${count > 1 ? `<polyline class="talk-chart-line" points="${line.join(" ")}"/>` : ""}
      <circle class="talk-chart-latest" cx="${f(x(latest))}" cy="${f(y(this.points[latest]!.hit))}" r="2.5"/>
      ${labels}${times}
      <line class="talk-chart-hair" x1="0" x2="0" y1="${TOP}" y2="${bottom}" visibility="hidden"/>
      <circle class="talk-chart-mark" cx="0" cy="0" r="3" visibility="hidden"/>`;
  }

  /** The whole conversation small, whatever its length: the stretch shown
   *  between two short bars, the rest under a veil of the paper. */
  private drawBrush(width: number): void {
    const any = this.requests.length > 0;
    this.brush.style.display = any ? "" : "none";
    if (!any) return;
    const plotW = width - LEFT - RIGHT;
    this.brush.setAttribute("viewBox", `0 0 ${width} ${BRUSH}`);
    this.brush.setAttribute("width", String(width));
    this.brush.setAttribute("height", String(BRUSH));
    const all = gather(this.requests, Math.max(1, Math.floor(plotW / 2)), 0);
    const lo = Math.min(...all.map((point) => point.lo));
    const hi = Math.max(...all.map((point) => point.hi), lo + 0.01);
    const x = (index: number) => LEFT + (all.length > 1 ? (index / (all.length - 1)) * plotW : plotW / 2);
    const y = (share: number) => 5 + ((hi - share) / (hi - lo)) * (BRUSH - 10);
    const overview =
      all.length > 1
        ? `<polyline class="talk-chart-overview" points="${all.map((point, index) => `${f(x(index))},${f(y(point.lo))}`).join(" ")}"/>`
        : `<circle class="talk-chart-overview-dot" cx="${f(x(0))}" cy="${f(y(all[0]!.lo))}" r="1.5"/>`;
    const [from, to] = this.range ?? [0, 1];
    const left = LEFT + from * plotW;
    const right = LEFT + to * plotW;
    this.brush.setAttribute("aria-valuetext", `${Math.round(from * 100)}% – ${Math.round(to * 100)}%`);
    const veil = this.range
      ? `<rect class="talk-chart-shade" x="${LEFT - 4}" y="0" width="${f(Math.max(0, left - LEFT + 4))}" height="${BRUSH}"/><rect class="talk-chart-shade" x="${f(right)}" y="0" width="${f(Math.max(0, LEFT + plotW + 4 - right))}" height="${BRUSH}"/>`
      : "";
    const bar = (at: number) => `<rect class="talk-chart-handle" x="${f(at - 1.25)}" y="${(BRUSH - 14) / 2}" width="2.5" height="14" rx="1.25"/>`;
    this.brush.innerHTML = `
      <rect class="talk-chart-track" x="${LEFT - 4}" y="0" width="${plotW + 8}" height="${BRUSH}" rx="6"/>
      ${overview}
      ${veil}
      <g class="talk-chart-handles${this.range ? "" : " whole"}">${bar(left - 2)}${bar(right + 2)}</g>
      <rect x="0" y="0" width="${width}" height="${BRUSH}" fill="transparent"/>`;
  }

  /** Says the request under the pointer, or nothing as it leaves. */
  private read(event: PointerEvent | null): void {
    const hair = this.main.querySelector<SVGLineElement>(".talk-chart-hair");
    const mark = this.main.querySelector<SVGCircleElement>(".talk-chart-mark");
    if (!event || !this.points.length || !hair || !mark) {
      this.readout.hidden = true;
      hair?.setAttribute("visibility", "hidden");
      mark?.setAttribute("visibility", "hidden");
      return;
    }
    const box = this.main.getBoundingClientRect();
    const width = this.width();
    const plotW = width - LEFT - RIGHT;
    const px = ((event.clientX - box.left) / box.width) * width;
    const count = this.points.length;
    const index = Math.max(0, Math.min(count - 1, Math.round(((px - LEFT) / plotW) * (count - 1))));
    const point = this.points[index]!;
    const at = this.placed.x[index]!;
    hair.setAttribute("x1", f(at));
    hair.setAttribute("x2", f(at));
    hair.setAttribute("visibility", "visible");
    mark.setAttribute("cx", f(at));
    mark.setAttribute("cy", f(this.placed.y[index]!));
    mark.setAttribute("visibility", "visible");
    const rows = this.visible();
    const first = rows[0]?.at ?? point.at;
    const final = rows[rows.length - 1]?.at ?? point.at;
    const hit = point.lo === point.hi ? percentFine(point.hit) : `${percentFine(point.lo)} – ${percentFine(point.hi)}`;
    this.readout.textContent = this.words.readout(hit, clock(point.at, first, final));
    this.readout.hidden = false;
    const shown = (at / width) * box.width;
    const half = this.readout.offsetWidth / 2;
    this.readout.style.left = `${Math.max(0, Math.min(box.width - this.readout.offsetWidth, shown - half))}px`;
  }

  private shareAt(event: PointerEvent): number {
    const box = this.brush.getBoundingClientRect();
    const width = this.width();
    const plotW = width - LEFT - RIGHT;
    return Math.max(0, Math.min(1, (((event.clientX - box.left) / box.width) * width - LEFT) / plotW));
  }

  private grab(event: PointerEvent): void {
    const at = this.shareAt(event);
    const range = this.range ?? [0, 1];
    const plotW = this.width() - LEFT - RIGHT;
    const near = 8 / plotW;
    const mode = Math.abs(at - range[0]) < near ? "start" : Math.abs(at - range[1]) < near ? "end" : this.range && at > range[0] && at < range[1] ? "move" : "new";
    this.dragging = { mode, from: at, range };
    this.brush.setPointerCapture(event.pointerId);
  }

  private drag(event: PointerEvent): void {
    const dragging = this.dragging;
    if (!dragging) return;
    const at = this.shareAt(event);
    const [start, end] = dragging.range;
    const least = Math.min(1, 2 / Math.max(1, this.requests.length - 1));
    let next: [number, number];
    if (dragging.mode === "move") {
      const shift = Math.max(-start, Math.min(1 - end, at - dragging.from));
      next = [start + shift, end + shift];
    } else if (dragging.mode === "start") next = [Math.min(at, end - least), end];
    else if (dragging.mode === "end") next = [start, Math.max(at, start + least)];
    else next = [Math.min(at, dragging.from), Math.max(at, dragging.from)];
    if (dragging.mode === "new" && next[1] - next[0] < least) return;
    this.range = next;
    this.drawn = "";
    this.draw();
  }

  private let(event: PointerEvent): void {
    if (!this.dragging) return;
    // A press that drew nothing new sets the stretch round where it fell.
    if (this.dragging.mode === "new" && (!this.range || this.range === this.dragging.range)) {
      const at = this.shareAt(event);
      const [start, end] = this.dragging.range;
      const width = end - start < 1 ? end - start : 0.25;
      const from = Math.max(0, Math.min(1 - width, at - width / 2));
      this.range = [from, from + width];
    }
    this.dragging = null;
    if (this.brush.hasPointerCapture(event.pointerId)) this.brush.releasePointerCapture(event.pointerId);
    this.show(this.range);
  }

  /** The arrows move the stretch a tenth along; Home or Escape shows all. */
  private key(event: KeyboardEvent): void {
    const [start, end] = this.range ?? [0, 1];
    const width = end - start;
    if (event.key === "Home" || (event.key === "Escape" && this.range)) {
      event.preventDefault();
      event.stopPropagation();
      this.show(null);
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const size = width >= 1 ? 0.5 : width;
    const step = (event.key === "ArrowLeft" ? -1 : 1) * Math.max(0.02, size / 10);
    const from = Math.max(0, Math.min(1 - size, (width >= 1 ? 1 - size : start) + step));
    this.show([from, from + size]);
  }
}

/** The requests in at most `room` points, each the requests it stands for
 *  with their lowest and highest kept. */
function gather(rows: readonly ModelRequest[], room: number, offset: number): Point2[] {
  const size = Math.max(1, Math.ceil(rows.length / room));
  const points: Point2[] = [];
  for (let index = 0; index < rows.length; index += size) {
    const group = rows.slice(index, index + size);
    const hits = group.map(hitOf);
    const latest = group[group.length - 1]!;
    points.push({ first: offset + index, last: offset + index + group.length - 1, lo: Math.min(...hits), hi: Math.max(...hits), hit: hits[hits.length - 1]!, at: latest.at, context: contextOf(latest) });
  }
  return points;
}

/** A share on the axis, as finely as the stretch needs to tell its two
 *  edges apart. */
function axisPercent(share: number, span: number): string {
  const digits = span >= 0.02 ? 0 : span >= 0.002 ? 1 : 2;
  return `${(share * 100).toFixed(digits)}%`;
}

/** When a request was, as its stretch needs: the time on a day alone, the
 *  date with it across days. */
function clock(at: number, first: number, final: number): string {
  const when = new Date(at);
  const time = `${String(when.getHours()).padStart(2, "0")}:${String(when.getMinutes()).padStart(2, "0")}`;
  return new Date(first).toDateString() === new Date(final).toDateString() ? time : `${when.getMonth() + 1}/${when.getDate()} ${time}`;
}

const f = (n: number) => n.toFixed(1);
