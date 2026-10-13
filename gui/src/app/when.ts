// A moment picked by hand (`world-gui.md`, 协助): a month of days and a
// column each of hours and minutes, on this device's clock, never in the
// past. It may also be no moment at all, for what has no end; then the
// calendar waits dimmed, and picking a day in it sets one.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";

export interface WhenWords {
  /** The choice of no moment. */
  none: string;
  /** The choice of one, before it is picked. */
  pick: string;
  month(year: number, month: number): string;
  /** Monday first. */
  weekdays: readonly string[];
  earlier: string;
  later: string;
  hour: string;
  minute: string;
  /** The moment in full: day, weekday, time. */
  at(at: Date, now: Date): string;
  /** How long until it. */
  left(at: Date, now: Date): string;
}

/** Minutes are picked in steps of five. */
const STEP = 5;

export class WhenPicker {
  readonly element: HTMLElement;
  private chosen: Date | null;
  /** The month shown: its first day. */
  private shown: Date;

  constructor(
    private readonly words: WhenWords,
    value: Date | null,
    private readonly changed: (value: Date | null) => void = () => {},
  ) {
    this.chosen = value;
    this.shown = firstOf(value ?? new Date());
    this.element = document.createElement("div");
    this.element.className = "when";
    this.element.innerHTML = this.frame();
    this.element.addEventListener("click", (event) => this.click(event));
    this.refresh();
  }

  get value(): Date | null {
    return this.chosen;
  }

  /** Brings the chosen hour and minute into view in their columns. */
  reveal(): void {
    for (const chosen of this.element.querySelectorAll<HTMLElement>(".when-col .chosen")) {
      const column = chosen.parentElement!;
      const cell = chosen.getBoundingClientRect();
      const box = column.getBoundingClientRect();
      if (column.scrollWidth > column.clientWidth) column.scrollLeft += cell.left - box.left - (box.width - cell.width) / 2;
      else column.scrollTop += cell.top - box.top - (box.height - cell.height) / 2;
    }
  }

  private frame(): string {
    const { words } = this;
    const hours = Array.from({ length: 24 }, (_, hour) => `<button type="button" class="when-cell" data-hour="${hour}">${pad(hour)}</button>`).join("");
    const minutes = Array.from({ length: 60 / STEP }, (_, index) => `<button type="button" class="when-cell" data-minute="${index * STEP}">${pad(index * STEP)}</button>`).join("");
    return `<div class="when-choice choice-options" role="radiogroup">
        <button type="button" class="choice" role="radio" data-when="none">${escape(words.none)}</button>
        <button type="button" class="choice" role="radio" data-when="set">${glyph("history", 14)}<span class="when-set-said"></span></button>
      </div>
      <p class="when-said"></p>
      <div class="when-pick">
        <div class="when-calendar">
          <div class="when-month">
            <button type="button" class="icon-button when-earlier" data-month="-1" aria-label="${escape(words.earlier)}" title="${escape(words.earlier)}">${glyph("chevron", 16)}</button>
            <span class="when-month-said"></span>
            <button type="button" class="icon-button when-later" data-month="1" aria-label="${escape(words.later)}" title="${escape(words.later)}">${glyph("chevron", 16)}</button>
          </div>
          <div class="when-days" role="grid"></div>
        </div>
        <div class="when-time">
          <div class="when-col" role="listbox" aria-label="${escape(words.hour)}">${hours}</div>
          <span class="when-colon" aria-hidden="true">:</span>
          <div class="when-col" role="listbox" aria-label="${escape(words.minute)}">${minutes}</div>
        </div>
      </div>`;
  }

  private click(event: Event): void {
    const target = event.target as HTMLElement;
    const button = target.closest<HTMLButtonElement>("button");
    if (!button || button.disabled) return;
    const now = new Date();
    if (button.dataset.when === "none") return this.set(null);
    if (button.dataset.when === "set") {
      if (!this.chosen) this.set(this.earliest(nextHour(addDays(now, 1)), now));
      return;
    }
    if (button.dataset.month) {
      this.shown = new Date(this.shown.getFullYear(), this.shown.getMonth() + Number(button.dataset.month), 1);
      return this.refresh();
    }
    const base = this.chosen ?? nextHour(addDays(now, 1));
    if (button.dataset.day) {
      const [year, month, day] = button.dataset.day.split("-").map(Number) as [number, number, number];
      return this.set(this.earliest(new Date(year, month, day, base.getHours(), base.getMinutes()), now));
    }
    if (button.dataset.hour) {
      const at = new Date(base);
      at.setHours(Number(button.dataset.hour));
      return this.set(this.earliest(at, now));
    }
    if (button.dataset.minute) {
      const at = new Date(base);
      at.setMinutes(Number(button.dataset.minute));
      return this.set(this.earliest(at, now));
    }
  }

  /** `at`, or the first step after `now` when `at` is already past. */
  private earliest(at: Date, now: Date): Date {
    if (at > now) return at;
    const next = new Date(now);
    next.setSeconds(0, 0);
    next.setMinutes(Math.floor(next.getMinutes() / STEP) * STEP + STEP);
    return next;
  }

  private set(value: Date | null): void {
    this.chosen = value;
    if (value) this.shown = firstOf(value);
    this.refresh();
    this.reveal();
    this.changed(value);
  }

  private refresh(): void {
    const { words, element, chosen, shown } = this;
    const now = new Date();
    element.classList.toggle("open", !!chosen);
    for (const choice of element.querySelectorAll<HTMLButtonElement>("[data-when]")) {
      const on = (choice.dataset.when === "set") === !!chosen;
      choice.classList.toggle("chosen", on);
      choice.setAttribute("aria-checked", String(on));
    }
    element.querySelector(".when-set-said")!.textContent = chosen ? words.at(chosen, now) : words.pick;
    const said = element.querySelector<HTMLElement>(".when-said")!;
    said.textContent = chosen ? words.left(chosen, now) : "";
    said.hidden = !chosen;
    element.querySelector(".when-month-said")!.textContent = words.month(shown.getFullYear(), shown.getMonth());
    const thisMonth = firstOf(now);
    element.querySelector<HTMLButtonElement>(".when-earlier")!.disabled = shown <= thisMonth;
    element.querySelector(".when-days")!.innerHTML = this.days(now);
    const today = chosen !== null && sameDay(chosen, now);
    for (const cell of element.querySelectorAll<HTMLButtonElement>("[data-hour]")) {
      const hour = Number(cell.dataset.hour);
      cell.classList.toggle("chosen", chosen?.getHours() === hour);
      cell.disabled = today && hour < now.getHours();
    }
    const thisHour = today && chosen!.getHours() === now.getHours();
    for (const cell of element.querySelectorAll<HTMLButtonElement>("[data-minute]")) {
      const minute = Number(cell.dataset.minute);
      cell.classList.toggle("chosen", chosen?.getMinutes() === minute);
      cell.disabled = thisHour && minute <= now.getMinutes();
    }
  }

  /** The shown month's days under their weekdays, Monday first; the days
   *  already gone cannot be picked. */
  private days(now: Date): string {
    const { words, chosen, shown } = this;
    const heads = words.weekdays.map((day) => `<span class="when-weekday" role="columnheader">${escape(day)}</span>`).join("");
    const lead = (shown.getDay() + 6) % 7;
    const count = new Date(shown.getFullYear(), shown.getMonth() + 1, 0).getDate();
    const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const cells: string[] = Array.from({ length: lead }, () => `<span class="when-blank"></span>`);
    for (let day = 1; day <= count; day += 1) {
      const date = new Date(shown.getFullYear(), shown.getMonth(), day);
      const classes = ["when-day", chosen && sameDay(chosen, date) ? "chosen" : "", sameDay(date, today) ? "today" : ""].filter(Boolean).join(" ");
      const key = `${date.getFullYear()}-${date.getMonth()}-${day}`;
      cells.push(`<button type="button" class="${classes}" data-day="${key}"${date < today ? " disabled" : ""}>${day}</button>`);
    }
    return heads + cells.join("");
  }
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

function firstOf(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), 1);
}

function addDays(at: Date, days: number): Date {
  const next = new Date(at);
  next.setDate(next.getDate() + days);
  return next;
}

/** The next full hour after `at`. */
function nextHour(at: Date): Date {
  const next = new Date(at);
  next.setMinutes(0, 0, 0);
  next.setHours(next.getHours() + 1);
  return next;
}

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}
