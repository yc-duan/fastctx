// The reasoning effort as a hand-drawn slider (`world-gui.md`, 对话: 推理强度
// 是一根手绘的滑条，一档一个刻点): a pencil line ruled across, an ink dot
// for each step, the stretch up to the knob gone over with a clay marker,
// a clay knob on it, and the step's name above. The knob follows the
// finger smoothly and, let go, glides to the nearest step; a press on a
// step, or the arrow keys, sends it there the same way, the marker drawn
// along behind it like a pen across the paper. The glide is the slider
// answering the hand, short and along the line, so it is kept where the
// system asks for less motion too (用户 2026-10-12: 点一档要划过去，不要瞬移).
//
// The same slider stands for any short row of steps, and may hold none
// chosen: a preference left to its default rests its knob, faint, where the
// default stands, and the first press or drag chooses.

import { blot, outline, ruled, seeded, type Point } from "../pen/shapes";

export interface EffortStep {
  /** The host's own name for it: "high". */
  value: string;
  /** What the page calls it: "高". */
  label: string;
}

/** The interface's names for the efforts the hosts offer; any other is
 *  shown as the host writes it. */
export function effortLabel(value: string, english: boolean): string {
  if (english) return value === "xhigh" ? "extra high" : value;
  return ({ none: "无", minimal: "最低", low: "低", medium: "中", high: "高", xhigh: "很高", max: "最高" } as Record<string, string>)[value] ?? value;
}

const WIDTH = 240;
const HEIGHT = 40;
const LINE_Y = 27;
const PAD = 10;
/** How far a press may wander before it is a drag rather than a press on
 *  a step, in pixels. */
const SLOP = 4;
/** How long the knob takes to glide one step, and at most. */
const GLIDE_STEP_MS = 200;
const GLIDE_MAX_MS = 520;

const coarse = matchMedia("(pointer: coarse)");

export class EffortSlider {
  readonly element: HTMLElement;
  private steps: EffortStep[] = [];
  /** The chosen step. */
  private index = 0;
  /** Where the knob is drawn, in steps; between two while it moves. */
  private at = 0;
  private press: { id: number; x: number; dragging: boolean } | null = null;
  private glide = 0;
  private marker: SVGPathElement | null = null;
  private knob: SVGPathElement | null = null;
  private name: HTMLElement | null = null;
  /** With none chosen: the name of what is taken instead. */
  private resting: string | null = null;

  constructor(
    name: string,
    private readonly chosen: (value: string) => void,
  ) {
    this.element = document.createElement("div");
    this.element.className = "effort-slider";
    this.element.tabIndex = 0;
    this.element.setAttribute("role", "slider");
    this.element.setAttribute("aria-label", name);
    this.element.dataset.ownPress = "";
    this.element.addEventListener("pointerdown", (event) => {
      if (!this.steps.length || event.button !== 0) return;
      // No text is selected and nothing else takes the press.
      event.preventDefault();
      this.element.focus({ preventScroll: true });
      this.element.setPointerCapture(event.pointerId);
      cancelAnimationFrame(this.glide);
      this.press = { id: event.pointerId, x: event.clientX, dragging: false };
    });
    this.element.addEventListener("pointermove", (event) => {
      const press = this.press;
      if (!press || press.id !== event.pointerId) return;
      if (!press.dragging && Math.abs(event.clientX - press.x) < SLOP) return;
      press.dragging = true;
      this.element.classList.add("dragging");
      this.place(this.stepsAt(event.clientX));
    });
    const release = (event: PointerEvent) => {
      const press = this.press;
      if (!press || press.id !== event.pointerId) return;
      this.press = null;
      this.element.classList.remove("dragging");
      const target = press.dragging ? Math.round(this.at) : Math.round(this.stepsAt(event.clientX));
      this.choose(target);
    };
    this.element.addEventListener("pointerup", release);
    this.element.addEventListener("pointercancel", (event) => {
      if (this.press?.id !== event.pointerId) return;
      this.press = null;
      this.element.classList.remove("dragging");
      this.glideTo(this.index);
    });
    this.element.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0;
      const to = event.key === "Home" ? 0 : event.key === "End" ? this.steps.length - 1 : this.index + step;
      if (to === this.index || to < 0 || to >= this.steps.length || (!step && event.key !== "Home" && event.key !== "End")) return;
      event.preventDefault();
      this.choose(to);
    });
  }

  /** Shows these steps with `value` chosen; a value the steps lack keeps
   *  the nearest one there was. */
  set(steps: EffortStep[], value: string | null): void {
    if (this.resting !== null) {
      // From resting to chosen the knob is drawn anew where the choice is.
      this.resting = null;
      this.element.classList.remove("unset");
      this.steps = [];
    }
    const known = steps.findIndex((step) => step.value === value);
    const before = this.steps[this.index]?.value;
    const same = steps.length === this.steps.length && steps.every((step, index) => step.value === this.steps[index]!.value);
    this.steps = steps;
    const index = known >= 0 ? known : Math.max(0, Math.min(steps.length - 1, steps.findIndex((step) => step.value === before)));
    if (same && this.marker && !this.press) {
      // The host confirmed or moved the choice: the knob goes there.
      if (index !== this.index) {
        this.index = index;
        this.glideTo(index);
      }
      return;
    }
    cancelAnimationFrame(this.glide);
    this.index = index;
    this.at = index;
    this.drawPaper();
  }

  /** Shows these steps with none chosen: the knob rests, faint, `at` steps
   *  along, between two where what is taken instead falls between them, and
   *  `label` names it. */
  unset(steps: EffortStep[], at: number, label: string): void {
    cancelAnimationFrame(this.glide);
    const same = this.resting !== null && steps.length === this.steps.length && steps.every((step, index) => step.value === this.steps[index]!.value);
    this.steps = steps;
    this.resting = label;
    this.element.classList.add("unset");
    this.index = Math.max(0, Math.min(steps.length - 1, Math.round(at)));
    this.at = Math.max(0, Math.min(steps.length - 1, at));
    if (same && this.marker) this.place(this.at);
    else this.drawPaper();
  }

  get value(): string | null {
    return this.resting !== null ? null : (this.steps[this.index]?.value ?? null);
  }

  private x(at: number): number {
    return this.steps.length < 2 ? WIDTH / 2 : PAD + (at * (WIDTH - PAD * 2)) / (this.steps.length - 1);
  }

  /** The pointer's place, in steps, held to the line's ends. */
  private stepsAt(clientX: number): number {
    const box = this.element.getBoundingClientRect();
    const x = ((clientX - box.left) / box.width) * WIDTH;
    if (this.steps.length < 2) return 0;
    return Math.max(0, Math.min(this.steps.length - 1, ((x - PAD) * (this.steps.length - 1)) / (WIDTH - PAD * 2)));
  }

  private choose(index: number): void {
    const changed = index !== this.index || this.resting !== null;
    if (this.resting !== null) {
      this.resting = null;
      this.element.classList.remove("unset");
    }
    this.index = index;
    this.glideTo(index);
    if (changed) this.chosen(this.steps[index]!.value);
  }

  private glideTo(index: number): void {
    cancelAnimationFrame(this.glide);
    const from = this.at;
    const distance = Math.abs(index - from);
    if (distance < 0.001) {
      this.place(index);
      return;
    }
    const duration = Math.min(GLIDE_MAX_MS, GLIDE_STEP_MS * Math.max(1, distance));
    const start = performance.now();
    const frame = (now: number) => {
      const t = Math.min(1, (now - start) / duration);
      // Eased out: the pen sets off briskly and slows as it reaches the step.
      const eased = 1 - Math.pow(1 - t, 3);
      this.place(from + (index - from) * eased);
      if (t < 1) this.glide = requestAnimationFrame(frame);
    };
    this.glide = requestAnimationFrame(frame);
  }

  /** The paper's marks, the same each time for the same steps. */
  private drawPaper(): void {
    const steps = this.steps;
    this.element.setAttribute("aria-valuemin", "0");
    this.element.setAttribute("aria-valuemax", String(Math.max(0, steps.length - 1)));
    if (!steps.length) {
      this.element.innerHTML = "";
      this.marker = this.knob = null;
      this.name = null;
      return;
    }
    const random = seeded(`effort:${steps.map((s) => s.value).join(",")}`);
    const line = ruled([PAD - 4, LINE_Y], [WIDTH - PAD + 4, LINE_Y], 1.3, random);
    const dots = steps.map((_, index) => blot([this.x(index), LINE_Y], 2.2, random)).join(" ");
    this.element.innerHTML = `<svg viewBox="0 0 ${WIDTH} ${HEIGHT}" preserveAspectRatio="none" aria-hidden="true">
      <path class="effort-line" d="${line}" filter="url(#pen-rule-lite)"/>
      <path class="effort-dots" d="${dots}"/>
      <path class="effort-marker" filter="url(#pen-marker-lite)"/>
      <path class="effort-knob"/>
    </svg><span class="effort-name"></span>`;
    this.marker = this.element.querySelector<SVGPathElement>(".effort-marker");
    this.knob = this.element.querySelector<SVGPathElement>(".effort-knob");
    this.name = this.element.querySelector<HTMLElement>(".effort-name");
    this.place(this.at);
  }

  /** Draws the marker and the knob at `at` steps along. */
  private place(at: number): void {
    this.at = at;
    const nearest = Math.round(at);
    const step = this.steps[nearest];
    const resting = this.resting !== null && !this.press;
    this.element.setAttribute("aria-valuenow", String(this.index));
    this.element.setAttribute("aria-valuetext", resting ? this.resting! : (this.steps[this.index]?.label ?? ""));
    this.element.title = resting || !step || step.label === step.value ? "" : step.value;
    if (!this.marker || !this.knob || !this.name || !step) return;
    const reach = this.x(at);
    // The marker starts past the first dot at full width, so no dot shows
    // through a thin start, and thins only where it ends at the knob.
    const swash: Point[] = [];
    for (let x = PAD - 6; x < reach; x += 6) swash.push([x, LINE_Y + Math.sin(x / 17) * 0.6]);
    swash.push([reach, LINE_Y]);
    this.marker.setAttribute("d", swash.length > 1 ? outline(swash, 7, seeded(`swash:${this.steps.length}`), { pressure: 0.18, taperStart: 0, taperEnd: 0.08 }) : "");
    // Under a finger the knob is drawn larger, so it shows past the fingertip.
    this.knob.setAttribute("d", blot([reach, LINE_Y], coarse.matches ? 9.5 : 6.5, seeded(`knob:${this.steps.length}`)));
    const labelX = Math.max(18, Math.min(WIDTH - 18, reach));
    this.name.style.left = `${((labelX / WIDTH) * 100).toFixed(2)}%`;
    this.name.textContent = resting ? this.resting! : step.label;
  }
}
