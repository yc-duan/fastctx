// A film that plays on a loop, as a video set in the page would
// (`world-gui.md`, 功能): no controls, the words said under it as it goes.
// It plays only while it can be seen; the page hidden or the film scrolled
// away, it waits where it was. Each frame touches only what is changing:
// a stroke is masked only while it is being written, a group is written
// only when its pose has moved. It plays where the system asks for less
// motion too: the film is what the page has to say, as a video would be.
// A computer's page shows it wide; a phone's, tall, laid out for the
// narrow screen rather than shrunk.

import type { Built, Pose, Shape } from "./reel";
import "./film.css";

/** How long the last frame stays before the film fades and begins again. */
const REST = 2400;
const FADE = 500;
/** Where the page is a phone's: the film's tall form. */
const NARROW = "(max-width: 760px)";

interface Stroke {
  path: SVGPathElement;
  mask: string;
  pen: SVGPathElement;
  start: number;
  end: number;
  /** 0 not begun, 1 being written, 2 written. */
  state: number;
  shown: number;
}

interface Moving {
  group: SVGGElement;
  poses: Required<Pose>[];
  ox: number;
  oy: number;
  last: string;
}

export class Film {
  readonly element: HTMLElement;
  private stage!: SVGGElement;
  private readonly caption: HTMLElement;
  private strokes: Stroke[] = [];
  private moving: Moving[] = [];
  private reel!: Built;
  private loop = 0;
  private said = -1;
  private began = 0;
  private at = 0;
  private frame = 0;
  private seen = false;
  private readonly observer: IntersectionObserver;
  private readonly narrow = matchMedia(NARROW);
  private readonly onVisibility = () => this.wake();
  private readonly onShape = () => this.load();

  /** `still`: only its last frame, never played; for a small picture of
   *  the film that opens it. `shape`: drawn in that composition whatever
   *  the page's, as a band of a picture across a phone's page. */
  constructor(
    private readonly make: (shape: Shape) => Built,
    private readonly label: string,
    private readonly still = false,
    private readonly shape?: Shape,
  ) {
    this.element = document.createElement("figure");
    this.element.className = "film";
    this.element.innerHTML = `<div class="film-stage"></div><figcaption class="film-caption"></figcaption>`;
    this.caption = this.element.querySelector<HTMLElement>(".film-caption")!;
    this.load();
    this.observer = new IntersectionObserver((entries) => {
      if (this.still) return;
      this.seen = entries.some((entry) => entry.isIntersecting);
      this.wake();
    });
    this.observer.observe(this.element);
    document.addEventListener("visibilitychange", this.onVisibility);
    this.narrow.addEventListener("change", this.onShape);
  }

  /** The reel for the page's shape, from the start. */
  private load(): void {
    const shape: Shape = this.shape ?? (this.narrow.matches ? "tall" : "wide");
    this.reel = this.make(shape);
    this.loop = this.reel.duration + REST;
    this.element.dataset.shape = shape;
    const holder = this.element.querySelector<HTMLElement>(".film-stage")!;
    holder.style.aspectRatio = `${this.reel.width} / ${this.reel.height}`;
    holder.innerHTML = `<svg viewBox="0 0 ${this.reel.width} ${this.reel.height}" role="img" aria-label="${escapeAttr(this.label)}"><g class="film-reel">${this.reel.markup}</g></svg>`;
    this.stage = holder.querySelector<SVGGElement>(".film-reel")!;
    const svg = holder.querySelector("svg")!;
    this.strokes = [];
    this.moving = [];
    this.said = -1;
    this.at = 0;
    this.began = performance.now();
    for (const path of this.stage.querySelectorAll<SVGPathElement>("path[data-w]")) {
      const [start, end] = path.dataset.w!.split(",").map(Number) as [number, number];
      const mask = path.dataset.m!;
      const pen = svg.querySelector<SVGPathElement>(`#${CSS.escape(mask)} > path`)!;
      this.strokes.push({ path, mask: `url(#${mask})`, pen, start, end, state: 0, shown: -1 });
    }
    for (const group of this.stage.querySelectorAll<SVGGElement>("g[data-k]")) {
      const [ox, oy] = group.dataset.o!.split(",").map(Number) as [number, number];
      this.moving.push({ group, poses: filled(JSON.parse(group.dataset.k!) as Pose[]), ox, oy, last: "" });
    }
    this.render(this.still ? this.reel.duration : 0);
  }

  /** Stops for good; the film is leaving the page. */
  destroy(): void {
    cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.observer.disconnect();
    document.removeEventListener("visibilitychange", this.onVisibility);
    this.narrow.removeEventListener("change", this.onShape);
  }

  /** Plays while it can be seen, waits where it was while it cannot. */
  private wake(): void {
    const playing = this.seen && document.visibilityState === "visible";
    if (playing && !this.frame) {
      this.began = performance.now() - this.at;
      this.frame = requestAnimationFrame(this.tick);
    } else if (!playing && this.frame) {
      cancelAnimationFrame(this.frame);
      this.frame = 0;
    }
  }

  private readonly tick = (now: number) => {
    this.at = (now - this.began) % this.loop;
    this.render(this.at);
    this.frame = requestAnimationFrame(this.tick);
  };

  /** The film as it stands at `t`. */
  private render(t: number): void {
    for (const stroke of this.strokes) {
      const p = t <= stroke.start ? 0 : t >= stroke.end ? 1 : ease((t - stroke.start) / (stroke.end - stroke.start));
      const state = p <= 0 ? 0 : p >= 1 ? 2 : 1;
      if (state !== stroke.state) {
        if (state === 0) stroke.path.setAttribute("visibility", "hidden");
        else stroke.path.removeAttribute("visibility");
        // Only a stroke being written carries its mask: a written one is a
        // plain shape, cheap to paint as the drawing moves.
        if (state === 1) stroke.path.setAttribute("mask", stroke.mask);
        else stroke.path.removeAttribute("mask");
        stroke.state = state;
      }
      if (state === 1) {
        const shown = Math.round(p * 1000) / 1000;
        if (shown !== stroke.shown) {
          stroke.pen.setAttribute("stroke-dashoffset", String(1 - shown));
          stroke.shown = shown;
        }
      }
    }
    for (const one of this.moving) {
      const pose = poseAt(one.poses, t);
      const transform = `translate(${r2(pose.x + one.ox)} ${r2(pose.y + one.oy)}) rotate(${r2(pose.r)}) scale(${r3(pose.sx)} ${r3(pose.sy)}) translate(${-one.ox} ${-one.oy})`;
      const written = `${transform}|${r3(pose.o)}`;
      if (written === one.last) continue;
      one.group.setAttribute("transform", transform);
      one.group.setAttribute("opacity", String(r3(pose.o)));
      one.last = written;
    }
    // The last frame rests, then fades before the film begins again.
    const fade = t > this.loop - FADE ? 1 - (t - (this.loop - FADE)) / FADE : 1;
    this.stage.setAttribute("opacity", String(r3(fade)));
    let said = -1;
    for (let index = 0; index < this.reel.captions.length; index += 1) if (this.reel.captions[index]!.at <= t) said = index;
    if (said !== this.said) {
      this.said = said;
      this.caption.textContent = said >= 0 ? this.reel.captions[said]!.text : "";
      this.caption.classList.remove("fresh");
      void this.caption.offsetWidth;
      this.caption.classList.add("fresh");
    }
  }
}

/** Poses with every value written out, each taken from the pose before
 *  where it was left out. */
function filled(poses: Pose[]): Required<Pose>[] {
  let last: Required<Pose> = { t: 0, x: 0, y: 0, sx: 1, sy: 1, r: 0, o: 1 };
  return poses.map((pose) => (last = { ...last, ...pose }));
}

function poseAt(poses: Required<Pose>[], t: number): Required<Pose> {
  if (t <= poses[0]!.t) return poses[0]!;
  for (let index = 1; index < poses.length; index += 1) {
    const to = poses[index]!;
    if (t >= to.t) continue;
    const from = poses[index - 1]!;
    const p = ease((t - from.t) / (to.t - from.t));
    const mix = (a: number, b: number) => a + (b - a) * p;
    return { t, x: mix(from.x, to.x), y: mix(from.y, to.y), sx: mix(from.sx, to.sx), sy: mix(from.sy, to.sy), r: mix(from.r, to.r), o: mix(from.o, to.o) };
  }
  return poses[poses.length - 1]!;
}

/** In and out gently, as a hand starts and stops. */
function ease(p: number): number {
  return p < 0.5 ? 2 * p * p : 1 - (-2 * p + 2) ** 2 / 2;
}

const r2 = (n: number) => Math.round(n * 100) / 100;
const r3 = (n: number) => Math.round(n * 1000) / 1000;

function escapeAttr(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}
