// What the World knows of the agent in a conversation, opened from the
// conversation's head: who it is and where it lives, which login
// of its agent it uses there, whom it works for, how it came, what it is
// doing, the models it called, what it started that still runs, its latest
// actions. The same facts as `fastctx w <id>`, asked of the machine it
// lives on while the card is open. Moving, renaming, archiving and deleting
// it are its "⋯"'s (`app/talk-menu.ts`); which agents may call which models
// is the model collaboration feature's, never a single conversation's
// (`world-gui.md`, 功能).

import { escape } from "./prose";
import { glyph } from "./glyphs";
import type { Address } from "../world/events";
import type { ResidentDetail, ResidentNow } from "../world/world";
import type { DetailsWords } from "../app/words";
import { redrawKeepingFocus } from "../app/focus";
import { fileLink } from "../app/places";
import { agentName, titleOf } from "../world/names";

export interface DetailsOptions {
  words: DetailsWords;
  /** "Claude laptop", for a resident named elsewhere. */
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
  markdown?: (() => Promise<string>) | undefined;
  /** "home ada" for each account it works for; empty for this
   *  computer's logins. */
  actingFor(resident: ResidentNow): string[];
  /** Whose agent it is, for one of another account assisting the user;
   *  null for one of the user's own. */
  assistFrom?: ((resident: ResidentNow) => string | null) | undefined;
  /** The logins of an agent on a machine (named or as written), as that
   *  machine says them: each home's directory, "" for the default one,
   *  with who is logged in there. */
  logins?: ((machine: string, agent: string) => Promise<Login[]>) | undefined;
}

export interface Login {
  value: string;
  label: string;
  note?: string | undefined;
  disabled?: boolean | undefined;
}

export class Details {
  readonly element: HTMLElement;
  private readonly body: HTMLElement;
  /** Kept across redraws, so what is said is announced once. */
  private readonly spoken: HTMLElement;
  private note = "";
  /** What each machine said of an agent's logins, by `machine/agent`,
   *  asked once while the card is about it. */
  private logins = new Map<string, Login[] | "asking">();
  private shown: { detail: ResidentDetail; colleagues: ResidentNow[]; now: Date } | null = null;

  constructor(private readonly options: DetailsOptions) {
    this.element = document.createElement("aside");
    this.element.className = "details";
    this.element.hidden = true;
    // Focused as a whole when it opens, so Tab goes on into it.
    this.element.tabIndex = -1;
    this.element.setAttribute("aria-label", options.words.open);
    this.element.innerHTML = `<div class="details-body"></div><p class="spoken" role="status"></p>`;
    this.body = this.element.querySelector<HTMLElement>(".details-body")!;
    this.spoken = this.element.querySelector<HTMLElement>(".spoken")!;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
      if (!button || button.disabled) return;
      const { words } = options;
      switch (button.dataset.act) {
        case "export":
          if (!options.markdown) return;
          void options.markdown().then((text) => navigator.clipboard.writeText(text)).then(
            () => this.say(words.exported),
            (error: unknown) => this.say(String(error)),
          );
          return;
        case "attach":
          if (!options.attach) return;
          this.say(words.openedOwn);
          options.attach().then(
            () => this.say(""),
            (error: unknown) => this.say(String(error)),
          );
          return;
        case "stop":
          button.disabled = true;
          void options
            .stop()
            .then((stopped) => {
              this.note = stopped ? words.stopped : words.notRunning;
            })
            .catch((error: unknown) => {
              this.note = String(error);
            })
            .finally(() => {
              button.disabled = false;
              this.say(this.note);
            });
          return;
      }
    });
  }

  private say(text: string): void {
    this.note = text;
    this.spoken.textContent = text;
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
    this.shown = null;
    this.body.innerHTML = `<p class="quiet"${why ? ` title="${escape(why)}"` : ""}>${escape(why ? words.cannotLook(why) : words.notFound)}</p>`;
  }

  private redraw(): void {
    if (this.shown) this.show(this.shown.detail, this.shown.colleagues, this.shown.now);
  }

  show(detail: ResidentDetail, colleagues: ResidentNow[], now: Date): void {
    if (this.shown?.detail.resident.about.id !== detail.resident.about.id) this.logins.clear();
    this.shown = { detail, colleagues, now };
    redrawKeepingFocus(this.body, () => this.draw(detail, colleagues, now));
  }

  /** The logins of `agent` on `machine`, once its answer is in; asked the
   *  first time, and the card drawn again when it answers. */
  private loginsOn(machine: string, agent: string): Login[] | null {
    const key = `${machine}/${agent}`;
    const known = this.logins.get(key);
    if (known === "asking") return null;
    if (known) return known;
    if (!this.options.logins) return null;
    this.logins.set(key, "asking");
    this.options.logins(machine, agent).then(
      (logins) => {
        this.logins.set(key, logins);
        this.redraw();
      },
      () => this.logins.set(key, []),
    );
    return null;
  }

  private draw(detail: ResidentDetail, colleagues: ResidentNow[], now: Date): void {
    const { words, span } = this.options;
    const r = detail.resident;
    const about = r.about;
    const ago = (at: string) => span(now.getTime() - Date.parse(at));
    const state = r.removed
      ? words.removed(ago(r.removed))
      : r.now?.waiting_for_user
      ? words.waiting
      : r.now?.activity === "busy"
        ? words.busy(ago(r.now.since))
        : r.now
          ? words.idle(ago(r.now.since))
          : "";
    const origin =
      about.origin.kind === "colleague"
        ? words.askedBy(this.options.someone(about.origin.asked_by), about.origin.label)
        : words.openedByYou;
    const acting = this.options.actingFor(r);
    const assistFrom = this.options.assistFrom?.(r) ?? null;
    // Which login it uses, where it is not its machine's default one: who
    // is logged in there, as that machine says, else the directory.
    const login = about.home
      ? (this.loginsOn(about.machine, about.host.agent)?.find((one) => one.value === about.home)?.label ?? dirOf(about.home))
      : "";
    const facts: [string, string][] = [
      [words.state, state],
      [words.model, about.model ? about.model : ""],
      [words.machine, about.machine],
      [words.login, login],
      [words.assistFrom, assistFrom ?? ""],
      [words.actingFor, acting.join(words.separator)],
      [words.host, `${agentName(about.host.agent)} ${about.host.version}${about.world_started ? ` ${words.startedByWorld}` : ""}`],
      [words.origin, origin],
      [words.here, words.since(ago(about.born_at))],
    ];
    const trouble = r.now?.trouble
      ? `<p class="details-trouble">${escape(r.now.trouble.text)} ${escape(words.since(ago(r.now.trouble.at)))}</p>`
      : "";
    const busy = r.now?.activity === "busy";
    // Only what the World started has an interface of its own to open; a
    // session the user opened is already open where they opened it.
    const own = this.options.attach && about.world_started && about.machine === this.options.here();
    const section = (title: string, body: string, kind = "") =>
      body ? `<section class="details-section${kind}"><h3 class="details-title">${escape(title)}</h3>${body}</section>` : "";
    const task = detail.asked ? `<p class="details-task">${escape(detail.asked)}</p>` : "";
    const asked = colleagues.length
      ? `<ul class="details-people">${colleagues
          .map(
            (c) =>
              `<li><a href="${this.options.link({ resident: c.about.id, machine: c.about.machine })}"><span class="mark ${c.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span><span>${escape(titleOf(c) ?? c.about.id)}</span><span class="details-faint">${escape(`${agentName(c.about.host.agent)} ${c.about.machine}`)}</span></a></li>`,
          )
          .join("")}</ul>`
      : "";
    const running = detail.jobs.length
      ? `<ul class="details-jobs">${detail.jobs
          .map(
            (job) =>
              `<li><a class="details-job" href="${escape(fileLink(job.machine, job.output, true))}">${escape(job.job)}</a><span class="details-faint">${escape(`${job.machine} ${ago(job.started_at)}`)}</span><code>${escape(job.command)}</code></li>`,
          )
          .join("")}</ul>`
      : "";
    const actions = detail.actions.length
      ? `<ol class="details-actions">${[...detail.actions]
          .reverse()
          .map(
            (action) =>
              `<li><time>${escape(clock(new Date(action.at)))}</time><span class="details-what"><span class="details-hand">${escape(action.hand)}</span>${action.machine !== about.machine ? `<span class="details-faint">${escape(action.machine)} </span>` : ""}${escape(action.what)}</span><span class="details-result">${escape(action.result)}</span></li>`,
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
const dirOf = (dir: string) => dir.split(/[\\/]+/).filter(Boolean).pop() ?? dir;
const clock = (at: Date) => `${pad(at.getHours())}:${pad(at.getMinutes())}`;
