// The few marks every page of the ledger draws the same way.

import { ActsMenu } from "../app/acts-menu";
import { meter } from "../app/usage/draw";
import { agentGlyph, glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { machines, NOW, type Login, type Machine } from "./data";

export { escape };

/** One menu for the whole page, as the app has. */
export const menu = new ActsMenu();

export const machineOf = (login: Login): Machine => machines.find((machine) => machine.id === login.machine)!;
export const familyMark = (login: Login, size = 18) => agentGlyph(login.family, size);

export interface Gauge {
  /** Where it may go no further, of a hundred, drawn as a line across. */
  stop?: number | undefined;
  /** At its end or near it, whatever its length. */
  near?: boolean;
  idle?: boolean;
  /** Said under it. */
  under?: string;
  /** Said when the pointer rests on it. */
  said?: string;
  /** Said in place of the percent. */
  figure?: string;
  /** How thick the stroke is; 14 where not said. */
  tall?: number;
}

/** How full: the marker's stroke along a ruled line, then its figure. */
export function gauge(seed: string, used: number, how: Gauge = {}): string {
  const level = how.idle ? " idle" : used >= 100 ? " spent" : used >= 90 || how.near ? " high" : "";
  const stop = how.stop === undefined ? "" : `<i class="lg-stop" style="left:${how.stop}%"></i>`;
  return `<span class="lg-cell"${how.said ? ` title="${how.said}"` : ""}><span class="lg-meter${level}"><span class="lg-ink"${how.tall ? ` style="height:${how.tall}px"` : ""}>${meter(used / 100, seed, [], false, 150, how.tall ?? 14)}${stop}</span><b>${how.figure ?? `${used}%`}</b></span>${how.under === undefined ? "" : `<small>${how.under || "&nbsp;"}</small>`}</span>`;
}

/** A switch as the app draws it: its word, then the track. */
export function toggle(act: string, on: boolean, word: string, more = ""): string {
  return `<button class="lg-switch" type="button" role="switch" aria-checked="${on}" data-act="${act}" ${more}>${word ? `<span>${word}</span>` : ""}<span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`;
}

/** A few words to choose one of: the chosen one washed. */
export function choices(act: string, options: [string, string][], chosen: string, more = ""): string {
  return `<div class="choice-options">${options
    .map(([value, word]) => `<button type="button" class="choice${value === chosen ? " chosen" : ""}" data-act="${act}" data-value="${escape(value)}" ${more}>${escape(word)}</button>`)
    .join("")}</div>`;
}

/** A menu under the field that opens it, their left edges in line. */
export function menuUnder(anchor: HTMLElement, title: string, entries: Parameters<typeof menu.open>[2], chosen: (act: string) => void): void {
  const box = anchor.getBoundingClientRect();
  menu.openAt([box.left, box.bottom + 6], title, entries, chosen);
}

/** A month to pick a day in, today or after: the day chosen is washed. */
export function dayPicker(picked: (day: Date) => void, chosen: Date | null = null): HTMLElement {
  const element = document.createElement("div");
  element.className = "lg-cal";
  const today = new Date(NOW.getFullYear(), NOW.getMonth(), NOW.getDate());
  let month = new Date((chosen ?? today).getFullYear(), (chosen ?? today).getMonth(), 1);
  let at = chosen;
  const paint = () => {
    const lead = (month.getDay() + 6) % 7;
    const length = new Date(month.getFullYear(), month.getMonth() + 1, 0).getDate();
    const days = Array.from({ length }, (_, index) => {
      const day = new Date(month.getFullYear(), month.getMonth(), index + 1);
      return `<button type="button" data-day="${index + 1}" class="${at && day.getTime() === at.getTime() ? "chosen" : ""}"${day < today ? " disabled" : ""}>${index + 1}</button>`;
    });
    element.innerHTML = `<header><button type="button" data-month="-1" aria-label="上个月">${glyph("back", 14)}</button><b>${month.getFullYear()} 年 ${month.getMonth() + 1} 月</b><button type="button" class="lg-next" data-month="1" aria-label="下个月">${glyph("back", 14)}</button></header>
      <div class="lg-cal-grid">${["一", "二", "三", "四", "五", "六", "日"].map((name) => `<i>${name}</i>`).join("")}${"<span></span>".repeat(lead)}${days.join("")}</div>`;
  };
  element.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLElement>("button");
    if (!target) return;
    if (target.dataset.month) month = new Date(month.getFullYear(), month.getMonth() + Number(target.dataset.month), 1);
    else if (target.dataset.day) {
      at = new Date(month.getFullYear(), month.getMonth(), Number(target.dataset.day));
      picked(at);
    }
    paint();
  });
  paint();
  return element;
}

export const chevron = () => `<span class="lg-chevron">${glyph("chevron", 12)}</span>`;
