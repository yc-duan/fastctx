// The days a login's requests are narrowed to, picked by hand (`world-gui.md`,
// 智能体页): the weeks of the days its machine keeps them, Monday first, each
// month's first day named by its month; a press on the first day and one on
// the last, the days between washed as one stretch. A single day is two
// presses on it, or one and done.

import { escape } from "../../conversation/prose";
import { Sheet } from "../confirm";

export interface RangeWords {
  title: string;
  /** In a month's first day, for its number. */
  month(month: number): string;
  /** Monday first. */
  weekdays: readonly string[];
  /** Under the months: before a day is pressed, and while the last waits. */
  first: string;
  last: string;
  /** The days chosen, as the filter says them. */
  span(from: Date, to: Date): string;
  clear: string;
  done: string;
  close: string;
}

/** A stretch of days, each the local midnight that begins it. */
export interface DaySpan {
  from: Date;
  to: Date;
}

export class RangeSheet {
  private readonly sheet = new Sheet("ucr-sheet");
  private from: Date | null = null;
  private to: Date | null = null;
  private first = midnight(new Date());
  private last = midnight(new Date());
  private chosen: (span: DaySpan | null) => void = () => {};

  constructor(private readonly words: RangeWords) {
    this.sheet.body.addEventListener("click", (event) => this.click(event));
  }

  /** Opens on `span`, any of the `days` days up to `now`'s to choose from. */
  open(span: DaySpan | null, days: number, now: number, chosen: (span: DaySpan | null) => void): void {
    this.last = midnight(new Date(now));
    this.first = addDays(this.last, -(days - 1));
    this.from = span?.from ?? null;
    this.to = span?.to ?? null;
    this.chosen = chosen;
    this.draw();
    this.sheet.show();
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-day], [data-ucr]");
    if (!target) return;
    if (target.dataset.day) {
      const day = new Date(Number(target.dataset.day));
      if (!this.from || this.to) {
        this.from = day;
        this.to = null;
      } else if (day < this.from) {
        this.to = this.from;
        this.from = day;
      } else this.to = day;
      this.draw();
      return;
    }
    if (target.dataset.ucr === "clear") {
      this.sheet.close();
      this.chosen(null);
    } else if (target.dataset.ucr === "done" && this.from) {
      this.sheet.close();
      this.chosen({ from: this.from, to: this.to ?? this.from });
    }
  }

  private draw(): void {
    const { words, from, to } = this;
    const said = from ? words.span(from, to ?? from) : words.first;
    const waiting = from && !to ? `　${words.last}` : "";
    this.sheet.body.innerHTML = `${this.sheet.head(words.title, words.close)}
      <div class="ucr-days" role="grid">${this.days()}</div>
      <p class="ucr-said">${escape(said)}<span class="ucr-wait">${escape(waiting)}</span></p>
      <footer class="sheet-foot"><button class="text-button quiet-button" type="button" data-ucr="clear">${escape(words.clear)}</button><button class="sheet-go" type="button" data-ucr="done"${from ? "" : " disabled"}>${escape(words.done)}</button></footer>`;
  }

  /** The weeks of the days kept under their weekdays; the days of those
   *  weeks its machine keeps no requests of cannot be pressed. */
  private days(): string {
    const { words, from, first, last } = this;
    const to = this.to ?? from;
    const heads = words.weekdays.map((day) => `<span class="when-weekday" role="columnheader">${escape(day)}</span>`).join("");
    const cells: string[] = [];
    const start = addDays(first, -((first.getDay() + 6) % 7));
    const end = addDays(last, 6 - ((last.getDay() + 6) % 7));
    for (let date = start; date <= end; date = addDays(date, 1)) {
      const day = date.getDate();
      const kept = date >= first && date <= last;
      const end = !!from && (date.getTime() === from.getTime() || (!!to && date.getTime() === to.getTime()));
      const within = !!from && !!to && date > from && date < to;
      const classes = ["when-day", end ? "chosen" : "", within ? "ucr-in" : "", day === 1 ? "ucr-first" : "", date.getTime() === last.getTime() ? "today" : ""].filter(Boolean).join(" ");
      cells.push(`<button type="button" class="${classes}" data-day="${date.getTime()}"${kept ? "" : " disabled"}>${escape(day === 1 ? words.month(date.getMonth()) : String(day))}</button>`);
    }
    return heads + cells.join("");
  }
}

export function midnight(at: Date): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate());
}

export function addDays(at: Date, days: number): Date {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate() + days);
}
