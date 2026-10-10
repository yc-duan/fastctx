// The reasoning effort as a hand-drawn slider (`world-gui.md`, 对话: 推理强度
// 是一根手绘的滑条，一档一个刻点): a pencil line ruled across, an ink dot
// for each step, the stretch up to the chosen step gone over with a clay
// marker, a clay knob on it, and the step's name above. Dragged, clicked
// or moved with the arrow keys; a step at a time, never between. Where the
// system asks for less motion, the knob goes straight to its place.

import { blot, outline, ruled, seeded, type Point } from "../pen/shapes";
import { escape } from "./prose";

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

export class EffortSlider {
  readonly element: HTMLElement;
  private steps: EffortStep[] = [];
  private index = 0;
  private dragging = false;

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
      if (!this.steps.length) return;
      this.dragging = true;
      this.element.setPointerCapture(event.pointerId);
      this.pick(event.clientX);
    });
    this.element.addEventListener("pointermove", (event) => {
      if (this.dragging) this.pick(event.clientX);
    });
    const end = () => {
      if (!this.dragging) return;
      this.dragging = false;
      this.chosen(this.steps[this.index]!.value);
    };
    this.element.addEventListener("pointerup", end);
    this.element.addEventListener("pointercancel", end);
    this.element.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" || event.key === "ArrowUp" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowDown" ? -1 : 0;
      const to = event.key === "Home" ? 0 : event.key === "End" ? this.steps.length - 1 : this.index + step;
      if (to === this.index || to < 0 || to >= this.steps.length || (!step && event.key !== "Home" && event.key !== "End")) return;
      event.preventDefault();
      this.index = to;
      this.draw();
      this.chosen(this.steps[this.index]!.value);
    });
  }

  /** Shows these steps with `value` chosen; a value the steps lack keeps
   *  the nearest one there was. */
  set(steps: EffortStep[], value: string | null): void {
    const known = steps.findIndex((step) => step.value === value);
    const before = this.steps[this.index]?.value;
    this.steps = steps;
    this.index = known >= 0 ? known : Math.max(0, Math.min(steps.length - 1, steps.findIndex((step) => step.value === before)));
    this.draw();
  }

  get value(): string | null {
    return this.steps[this.index]?.value ?? null;
  }

  private x(index: number): number {
    return this.steps.length < 2 ? WIDTH / 2 : PAD + (index * (WIDTH - PAD * 2)) / (this.steps.length - 1);
  }

  private pick(clientX: number): void {
    const box = this.element.getBoundingClientRect();
    const at = ((clientX - box.left) / box.width) * WIDTH;
    let best = 0;
    for (let index = 1; index < this.steps.length; index += 1) if (Math.abs(this.x(index) - at) < Math.abs(this.x(best) - at)) best = index;
    if (best === this.index) return;
    this.index = best;
    this.draw();
  }

  private draw(): void {
    const steps = this.steps;
    const step = steps[this.index];
    this.element.setAttribute("aria-valuemin", "0");
    this.element.setAttribute("aria-valuemax", String(Math.max(0, steps.length - 1)));
    this.element.setAttribute("aria-valuenow", String(this.index));
    this.element.setAttribute("aria-valuetext", step?.label ?? "");
    this.element.title = step && step.label !== step.value ? step.value : "";
    if (!steps.length) {
      this.element.innerHTML = "";
      return;
    }
    // The paper's marks are the same each time for the same steps; only the
    // marker's reach and the knob follow the choice.
    const random = seeded(`effort:${steps.map((s) => s.value).join(",")}`);
    const line = ruled([PAD - 4, LINE_Y], [WIDTH - PAD + 4, LINE_Y], 1.3, random);
    const dots = steps.map((_, index) => blot([this.x(index), LINE_Y], 2.2, random)).join(" ");
    const reach = this.x(this.index);
    const swash: Point[] = [];
    for (let x = PAD - 3; x <= reach; x += 6) swash.push([x, LINE_Y + Math.sin(x / 17) * 0.6]);
    swash.push([reach, LINE_Y]);
    const marker = swash.length > 1 ? outline(swash, 7, seeded(`swash:${this.index}`), { pressure: 0.22, taper: 0.06 }) : "";
    const knob = blot([reach, LINE_Y], 6.5, seeded(`knob:${this.index}`));
    const labelX = Math.max(18, Math.min(WIDTH - 18, reach));
    this.element.innerHTML = `<svg viewBox="0 0 ${WIDTH} ${HEIGHT}" preserveAspectRatio="none" aria-hidden="true">
      <path class="effort-line" d="${line}" filter="url(#pen-rule-lite)"/>
      ${marker ? `<path class="effort-marker" d="${marker}" filter="url(#pen-marker-lite)"/>` : ""}
      <path class="effort-dots" d="${dots}"/>
      <path class="effort-knob" d="${knob}" style="--knob-x:${reach}px"/>
    </svg><span class="effort-name" style="left:${((labelX / WIDTH) * 100).toFixed(2)}%">${escape(step!.label)}</span>`;
  }
}
