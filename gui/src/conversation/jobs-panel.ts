// What the resident started that still runs: a mark in the input's row with
// how many, quietly moving, and over it a small panel of each, where it runs
// and for how long. A row pressed turns the panel to its output, the row
// at the top leading back, read from the machine running it a screen at a
// time and following as it grows; a row's stop asks once, and the agent is
// told the user stopped it, so it does not take it for a failure. A row
// whose output is open stays until it is closed, also after its command
// ends. The panel keeps the size it opened at, as tall as an output needs,
// so turning to an output and back moves nothing (`../app/steady`).

import { Popover } from "./popover";
import { Steady } from "../app/steady";
import { LineViewport, type LineSource } from "../longview";
import { glyph } from "./glyphs";
import { escape } from "./prose";

export interface Job {
  job: string;
  machine: string;
  started_at: string;
  command: string;
  output: string;
  /** A sentinel the conversation left, in place of a command: how it stands
   *  in a word, how it watches, and what it saw last. */
  sentinel?: { phase: string; plan: string; seen: string | null; wakes?: number[] } | undefined;
}

export interface JobsWords {
  title: string;
  stop: string;
  stopAsk: string;
  /** Withdrawing a sentinel, and asking before it. */
  withdraw: string;
  withdrawAsk: string;
  notStopped: string;
  cancel: string;
  /** When a sentinel next wakes the conversation, short; every time it
   *  will, when pointed at. */
  wakeAt(at: number, now: number): string;
  wakes(times: string[]): string;
  output(command: string): string;
  loading: string;
  failed(why: string): string;
  /** The way to the output broke; it comes back once it is open. */
  interrupted: string;
  ended: string;
  lasted(ms: number): string;
}

/** A job's output as lines, from the machine holding it; closed when the
 *  row is. */
export type OutputLines = LineSource & { close(): Promise<void> };

export interface JobsPanelOptions {
  words: JobsWords;
  /** Stops a job by the World's own way; absent where it cannot be. */
  stop?: ((job: string, machine: string) => Promise<void>) | undefined;
  /** Opens a job's output; absent where it cannot be read from here. */
  output?: ((job: Job) => Promise<OutputLines>) | undefined;
  /** The World's words for why a stop failed, for the pointer resting on
   *  it. */
  detail(error: unknown): string;
}

interface Row {
  job: Job;
  element: HTMLElement;
  open: { element: HTMLElement; viewport: LineViewport | null; lines: OutputLines | null; closed: boolean } | null;
  ended: boolean;
}

export class JobsPanel {
  private readonly rows = new Map<string, Row>();
  private readonly popover: Popover;
  private readonly list: HTMLUListElement;
  private readonly steady: Steady;

  constructor(
    private readonly button: HTMLButtonElement,
    private readonly pop: HTMLElement,
    private readonly options: JobsPanelOptions,
  ) {
    pop.innerHTML = `<p class="jobs-title">${escape(options.words.title)}</p><ul class="jobs-list"></ul>`;
    this.list = pop.querySelector("ul")!;
    this.steady = new Steady(pop);
    // Closed, rows of ended commands are let go with their output.
    this.popover = new Popover(pop, button, () => {
      for (const row of [...this.rows.values()]) if (row.ended) this.remove(row);
      this.steady.free();
    });
    button.addEventListener("click", () => this.show(!this.popover.open));
    pop.addEventListener("click", (event) => void this.act(event));
  }

  private show(shown: boolean): void {
    if (!shown) return this.popover.hide();
    this.popover.show();
    this.steady.hold();
  }

  update(jobs: Job[], now: Date): void {
    const { words } = this.options;
    const running = new Set(jobs.map((job) => job.job));
    for (const row of [...this.rows.values()]) {
      if (running.has(row.job.job)) continue;
      if (row.open) this.end(row);
      else this.remove(row);
    }
    for (const job of jobs) {
      let row = this.rows.get(job.job);
      if (!row) {
        row = { job, element: this.draw(job), open: null, ended: false };
        this.rows.set(job.job, row);
        this.list.append(row.element);
      }
      row.job = job;
      const where = job.sentinel ? this.whereWatching(job, now.getTime()) : escape(`${job.machine} ${words.lasted(now.getTime() - Date.parse(job.started_at))}`);
      const shown = row.element.querySelector<HTMLElement>(".job-where")!;
      if (shown.dataset.drawn !== where) {
        shown.dataset.drawn = where;
        shown.innerHTML = where;
      }
      if (job.sentinel) this.watching(row.element, job);
    }
    const count = jobs.length;
    this.button.hidden = this.rows.size === 0;
    if (!this.rows.size) this.show(false);
    this.button.title = words.title;
    this.button.setAttribute("aria-label", `${words.title} ${count}`);
    const html = `${glyph("terminal", 16)}<span class="jobs-count">${count}</span>`;
    if (this.button.innerHTML !== html) this.button.innerHTML = html;
  }

  /** Where a sentinel lives and how it stands, with a clock and the next
   *  time it wakes the conversation where it will. */
  private whereWatching(job: Job, now: number): string {
    const { words } = this.options;
    const sentinel = job.sentinel!;
    const coming = (sentinel.wakes ?? []).filter((at) => at > now).sort((a, b) => a - b);
    const wake = coming.length
      ? `<span class="job-wake" title="${escape(words.wakes(coming.map((at) => words.wakeAt(at, now))))}">${glyph("clock", 12)}${escape(words.wakeAt(coming[0]!, now))}</span>`
      : "";
    return `${escape(`${job.machine} ${sentinel.phase}`)}${wake}`;
  }

  /** A sentinel's sentence, how it watches beneath, and what it saw last
   *  when pointed at. */
  private watching(element: HTMLElement, job: Job): void {
    const plan = element.querySelector<HTMLElement>(".job-plan")!;
    if (plan.textContent !== job.sentinel!.plan) plan.textContent = job.sentinel!.plan;
    const said = [job.command, job.sentinel!.plan, job.sentinel!.seen ?? ""].filter(Boolean).join("\n\n");
    const what = element.querySelector<HTMLElement>(".job-what")!;
    if (what.title !== said) what.title = said;
  }

  private draw(job: Job): HTMLElement {
    const { words } = this.options;
    const element = document.createElement("li");
    element.className = "job-item";
    element.dataset.job = job.job;
    if (job.sentinel) {
      element.classList.add("watching");
      const stop = this.options.stop
        ? `<button class="icon-button job-stop" type="button" data-stop title="${escape(words.withdraw)}" aria-label="${escape(words.withdraw)}">${glyph("stop", 14)}</button>`
        : "";
      element.innerHTML = `<div class="job-row"><span class="job-mark" aria-hidden="true">${glyph("sentinel", 12)}</span><span class="job-what"><span class="job-sentence">${escape(job.command)}</span><span class="job-where"></span><span class="job-plan"></span></span>${stop}</div>`;
      this.watching(element, job);
      return element;
    }
    const command = job.command.split("\n")[0] ?? "";
    const what = `<span class="job-back" aria-hidden="true">${glyph("chevron", 14)}</span><code class="job-command">${escape(command)}</code><span class="job-where"></span>`;
    element.innerHTML = `<div class="job-row"><span class="pulse" aria-hidden="true"></span>${
      this.options.output
        ? `<button class="job-what" type="button" data-open aria-expanded="false" title="${escape(job.command)}">${what}</button>`
        : `<span class="job-what" title="${escape(job.command)}">${what}</span>`
    }${this.options.stop ? `<button class="icon-button job-stop" type="button" data-stop title="${escape(words.stop)}" aria-label="${escape(words.stop)}">${glyph("stop", 14)}</button>` : ""}</div>`;
    return element;
  }

  /** A command that ended while its output was open: its row stays,
   *  still, saying so. */
  private end(row: Row): void {
    if (row.ended) return;
    row.ended = true;
    row.element.classList.add("ended");
    row.element.querySelector(".job-stop")?.remove();
    row.element.querySelector(".jobs-confirm")?.remove();
    row.element.querySelector<HTMLElement>(".job-where")!.textContent = `${row.job.machine} ${this.options.words.ended}`;
  }

  private remove(row: Row): void {
    this.close(row);
    row.element.remove();
    this.rows.delete(row.job.job);
  }

  private async act(event: Event): Promise<void> {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    const element = target?.closest<HTMLElement>(".job-item");
    const row = element ? this.rows.get(element.dataset.job!) : undefined;
    if (!target || !row) return;
    const { words } = this.options;
    if (target.dataset.open !== undefined) {
      if (row.open) {
        this.close(row);
        if (row.ended) this.remove(row);
      } else {
        // One output at a time: the panel turns to it.
        for (const other of this.rows.values()) if (other !== row && other.open) this.close(other);
        void this.openOutput(row);
      }
      return;
    }
    if (target.dataset.stop !== undefined) {
      if (row.element.querySelector(".jobs-confirm")) return;
      row.element
        .querySelector(".job-row")!
        .insertAdjacentHTML(
          "afterend",
          `<div class="jobs-confirm"><span>${escape(row.job.sentinel ? words.withdrawAsk : words.stopAsk)}</span><button class="ask-button" type="button" data-cancel>${escape(words.cancel)}</button><button class="ask-button ask-primary" type="button" data-confirm>${escape(row.job.sentinel ? words.withdraw : words.stop)}</button></div>`,
        );
      return;
    }
    const confirm = target.closest<HTMLElement>(".jobs-confirm");
    if (!confirm) return;
    if (target.dataset.cancel !== undefined) {
      confirm.remove();
      return;
    }
    if (target.dataset.confirm !== undefined && this.options.stop) {
      target.disabled = true;
      try {
        await this.options.stop(row.job.job, row.job.machine);
        confirm.remove();
      } catch (error) {
        // Why, in its own words, beside it: the user decides what next.
        const why = this.options.detail(error);
        const said = confirm.querySelector("span")!;
        said.textContent = `${words.notStopped}: ${why}`;
        said.title = why;
        target.disabled = false;
      }
    }
  }

  private async openOutput(row: Row): Promise<void> {
    const { words, output } = this.options;
    if (!output) return;
    const element = document.createElement("div");
    element.className = "job-output";
    element.textContent = words.loading;
    row.element.append(element);
    row.element.querySelector(".job-what")?.setAttribute("aria-expanded", "true");
    const open: NonNullable<Row["open"]> = { element, viewport: null, lines: null, closed: false };
    row.open = open;
    row.element.classList.add("reading");
    this.pop.classList.add("reading");
    try {
      const lines = await output(row.job);
      if (open.closed) {
        void lines.close();
        return;
      }
      open.lines = lines;
      const command = row.job.command.split("\n")[0] ?? "";
      open.viewport = new LineViewport(lines, {
        label: words.output(command),
        follow: true,
        loading: words.loading,
        failed: (error) => words.failed(error instanceof Error ? error.message : String(error)),
        interrupted: words.interrupted,
      });
      element.textContent = "";
      element.append(open.viewport.element);
    } catch (error) {
      element.textContent = words.failed(error instanceof Error ? error.message : String(error));
      element.classList.add("failed");
    }
  }

  private close(row: Row): void {
    const open = row.open;
    if (!open) return;
    open.closed = true;
    open.viewport?.destroy();
    if (open.lines) void open.lines.close().catch((error: unknown) => console.warn("a job's output could not be let go", error));
    open.element.remove();
    row.open = null;
    row.element.classList.remove("reading");
    this.pop.classList.remove("reading");
    row.element.querySelector(".job-what")?.setAttribute("aria-expanded", "false");
  }
}
