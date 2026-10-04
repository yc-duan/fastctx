// What the World knows of the agent in a conversation, opened from the
// conversation's head: who it is and where, how it came, what it is doing,
// the colleagues it asked, what it started that still runs, its latest
// actions, and what can be done to it. The same facts as `fastctx w <id>`,
// asked of the machine it lives on while the card is open.

import { escape } from "./prose";
import { glyph } from "./glyphs";
import type { Address } from "../world/events";
import type { ResidentDetail, ResidentNow } from "../world/world";
import type { DetailsWords } from "../app/words";

export interface DetailsOptions {
  words: DetailsWords;
  /** "Claude · laptop", for a resident named elsewhere. */
  someone(address: Address): string;
  link(address: Address): string;
  /** Says a span of time: "4 分钟". */
  span(ms: number): string;
  stop(): Promise<boolean>;
  /** The machine this device is, where it is one. */
  here(): string | null;
  /** Opens the agent in its own official interface on this computer;
   *  resolves when that ends. */
  attach?: (() => Promise<void>) | undefined;
  /** The conversation as Markdown, for copying out. */
  markdown?: (() => string) | undefined;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class Details {
  readonly element: HTMLElement;
  private readonly body: HTMLElement;
  private note = "";

  constructor(private readonly options: DetailsOptions) {
    this.element = document.createElement("aside");
    this.element.className = "details";
    this.element.hidden = true;
    this.element.innerHTML = `<div class="details-body"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".details-body")!;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
      if (!button || button.disabled) return;
      if (button.dataset.act === "export" && options.markdown) {
        void navigator.clipboard.writeText(options.markdown()).then(
          () => this.say(options.words.exported),
          (error: unknown) => this.say(String(error)),
        );
        return;
      }
      if (button.dataset.act === "attach" && options.attach) {
        this.say(options.words.openedOwn);
        options.attach().then(
          () => this.say(""),
          (error: unknown) => this.say(String(error)),
        );
        return;
      }
      if (button.dataset.act === "stop") {
        button.disabled = true;
        void options
          .stop()
          .then((stopped) => {
            this.note = stopped ? options.words.stopped : options.words.notRunning;
          })
          .catch((error: unknown) => {
            this.note = String(error);
          })
          .finally(() => {
            button.disabled = false;
            this.say(this.note);
          });
      }
    });
  }

  private say(text: string): void {
    this.note = text;
    const note = this.element.querySelector<HTMLElement>(".details-note");
    if (note) note.textContent = text;
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  set open(shown: boolean) {
    this.element.hidden = !shown;
  }

  /** The World could not say. */
  unknown(why: string | null): void {
    const { words } = this.options;
    this.body.innerHTML = `<p class="quiet">${escape(why ? words.cannotLook(why) : words.notFound)}</p>`;
  }

  show(detail: ResidentDetail, colleagues: ResidentNow[], now: Date): void {
    const { words, span } = this.options;
    const r = detail.resident;
    const about = r.about;
    const ago = (at: string) => span(now.getTime() - Date.parse(at));
    const state = r.now?.waiting_for_user
      ? words.waiting
      : r.now?.activity === "busy"
        ? words.busy(ago(r.now.since))
        : r.now?.activity === "asleep"
          ? words.asleep(ago(r.now.since))
          : r.now
            ? words.idle(ago(r.now.since))
            : "";
    const origin =
      about.origin.kind === "colleague"
        ? words.askedBy(this.options.someone(about.origin.asked_by), about.origin.label)
        : words.openedByYou;
    const facts: [string, string][] = [
      [words.state, state],
      [words.model, about.model ? about.model : ""],
      [words.machine, about.machine],
      [words.host, `${agentName(about.host.agent)} ${about.host.version}${about.world_started ? ` · ${words.startedByWorld}` : ""}`],
      [words.origin, origin],
      [words.here, words.since(ago(about.born_at))],
    ];
    const trouble = r.now?.trouble
      ? `<p class="details-trouble">${escape(r.now.trouble.text)} · ${escape(words.since(ago(r.now.trouble.at)))}</p>`
      : "";
    const busy = r.now?.activity === "busy";
    // Only what the World started has an interface of its own to open; a
    // session the user opened is already open where they opened it.
    const own = this.options.attach && about.world_started && about.machine === this.options.here();
    const section = (title: string, body: string) =>
      body ? `<section class="details-section"><h3 class="details-title">${escape(title)}</h3>${body}</section>` : "";
    const task = detail.asked ? `<p class="details-task">${escape(detail.asked)}</p>` : "";
    const asked = colleagues.length
      ? `<ul class="details-people">${colleagues
          .map(
            (c) =>
              `<li><a href="${this.options.link({ resident: c.about.id, machine: c.about.machine })}"><span class="mark ${c.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span><span>${escape(c.about.origin.kind === "colleague" ? c.about.origin.label : c.about.id)}</span><span class="details-faint">${escape(`${agentName(c.about.host.agent)} · ${c.about.machine}`)}</span></a></li>`,
          )
          .join("")}</ul>`
      : "";
    const running = detail.jobs.length
      ? `<ul class="details-jobs">${detail.jobs
          .map(
            (job) =>
              `<li><span class="details-job">${escape(job.job)}</span><span class="details-faint">${escape(`${job.machine} · ${ago(job.started_at)}`)}</span><code>${escape(job.command)}</code></li>`,
          )
          .join("")}</ul>`
      : "";
    const actions = detail.actions.length
      ? `<ol class="details-actions">${[...detail.actions]
          .reverse()
          .map(
            (action) =>
              `<li><time>${escape(clock(new Date(action.at)))}</time><span class="details-what"><span class="details-hand">${escape(action.hand)}</span>${action.machine !== about.machine ? `<span class="details-faint">${escape(action.machine)} · </span>` : ""}${escape(action.what)}</span><span class="details-result">${escape(action.result)}</span></li>`,
          )
          .join("")}</ol>`
      : `<p class="quiet">${escape(words.noActions)}</p>`;
    this.body.innerHTML = `
      <dl class="facts">${facts
        .filter(([, value]) => value)
        .map(([name, value]) => `<dt>${escape(name)}</dt><dd>${escape(value)}</dd>`)
        .join("")}</dl>
      ${trouble}
      <div class="details-acts">
        <button class="text-button" type="button" data-act="stop"${busy ? "" : " disabled"}>${glyph("stop", 14)}<span>${escape(words.stop)}</span></button>
        ${own ? `<button class="text-button" type="button" data-act="attach">${glyph("command", 14)}<span>${escape(words.openOwn(agentName(about.host.agent)))}</span></button>` : ""}
        ${this.options.markdown ? `<button class="text-button" type="button" data-act="export">${glyph("copy", 14)}<span>${escape(words.exportMarkdown)}</span></button>` : ""}
        <span class="details-note">${escape(this.note)}</span>
      </div>
      ${section(words.task, task)}
      ${section(words.colleagues, asked)}
      ${section(words.running, running)}
      ${section(words.actions(detail.actions.length, detail.actions_total), actions)}
      <p class="details-records"><span>${escape(words.records)}</span><code>${escape(`${about.machine}:${detail.records}`)}</code></p>`;
  }
}

const pad = (n: number) => String(n).padStart(2, "0");
const clock = (at: Date) => `${pad(at.getHours())}:${pad(at.getMinutes())}`;
