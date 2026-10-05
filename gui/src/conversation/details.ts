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
import { Confirm } from "../app/confirm";
import { redrawKeepingFocus } from "../app/focus";
import { fileLink } from "../app/places";

/** What can be done to the agent beyond its conversation, as the user.
 *  Each rejects with what the World said when it was not done. */
export interface Manage {
  sleep(): Promise<void>;
  wake(): Promise<void>;
  /** Into a project, or out of any where null. */
  move(town: string | null): Promise<void>;
  remove(): Promise<void>;
  /** The projects an agent on a machine can be in: those spanning it. */
  towns(machine: string): { town: string; name: string }[];
  /** It is no longer in the World: its conversation is left. */
  removed(): void;
}

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
  /** Managing it, where the World can. */
  manage?: Manage | undefined;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

type Managing = "sleep" | "wake" | "move";

export class Details {
  readonly element: HTMLElement;
  private readonly body: HTMLElement;
  /** Kept across redraws, so what is said is announced once. */
  private readonly spoken: HTMLElement;
  private note = "";
  private manageNote = "";
  /** What is being done to it now, so the card drawn again in the
   *  meantime still says so. */
  private managing: Managing | null = null;
  /** Whether the projects to move it to are shown. */
  private moving = false;
  private confirm: Confirm | null = null;
  private shown: { detail: ResidentDetail; colleagues: ResidentNow[]; now: Date } | null = null;

  constructor(private readonly options: DetailsOptions) {
    this.element = document.createElement("aside");
    this.element.className = "details";
    this.element.hidden = true;
    this.element.innerHTML = `<div class="details-body"></div><p class="spoken" role="status"></p>`;
    this.body = this.element.querySelector<HTMLElement>(".details-body")!;
    this.spoken = this.element.querySelector<HTMLElement>(".spoken")!;
    this.element.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
      if (!button || button.disabled) return;
      const { words, manage } = options;
      switch (button.dataset.act) {
        case "export":
          if (!options.markdown) return;
          void navigator.clipboard.writeText(options.markdown()).then(
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
        case "sleep":
          if (manage) this.manageWith("sleep", () => manage.sleep(), words.slept);
          return;
        case "wake":
          if (manage) this.manageWith("wake", () => manage.wake(), words.woke);
          return;
        case "move":
          this.moving = !this.moving;
          this.redraw();
          return;
        case "move-to": {
          if (!manage || !this.shown) return;
          const town = button.dataset.town || null;
          const name = manage.towns(this.shown.detail.resident.about.machine).find((t) => t.town === town)?.name ?? town;
          this.moving = false;
          this.manageWith("move", () => manage.move(town), name ? words.moved(name) : words.left);
          return;
        }
        case "remove":
          if (manage && this.shown) this.askRemove(manage, this.shown.detail.resident);
      }
    });
  }

  private say(text: string): void {
    this.note = text;
    this.spoken.textContent = text;
    const note = this.element.querySelector<HTMLElement>(".details-note");
    if (note) note.textContent = text;
  }

  private manageWith(what: Managing, act: () => Promise<void>, done: string): void {
    const { words } = this.options;
    this.managing = what;
    this.manageNote = "";
    this.redraw();
    act()
      .then(() => {
        this.manageNote = done;
      })
      .catch((error: unknown) => {
        this.manageNote = words.failed(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        this.managing = null;
        this.spoken.textContent = this.manageNote;
        this.redraw();
      });
  }

  /** Asks once more before it is removed: what removing means is said. */
  private askRemove(manage: Manage, resident: ResidentNow): void {
    const { words } = this.options;
    const about = resident.about;
    this.confirm ??= new Confirm({ close: words.cancel, working: words.working, failed: words.failed });
    const who =
      about.origin.kind === "colleague" ? about.origin.label : this.options.someone({ resident: about.id, machine: about.machine });
    this.confirm.ask({
      title: words.removeTitle(who),
      what: words.removeWhat,
      go: words.removeGo,
      act: () => manage.remove(),
      done: () => manage.removed(),
    });
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  set open(shown: boolean) {
    this.element.hidden = !shown;
    if (!shown) this.moving = false;
  }

  /** The World could not say. */
  unknown(why: string | null): void {
    const { words } = this.options;
    this.shown = null;
    this.body.innerHTML = `<p class="quiet">${escape(why ? words.cannotLook(why) : words.notFound)}</p>`;
  }

  private redraw(): void {
    if (this.shown) this.show(this.shown.detail, this.shown.colleagues, this.shown.now);
  }

  show(detail: ResidentDetail, colleagues: ResidentNow[], now: Date): void {
    this.shown = { detail, colleagues, now };
    redrawKeepingFocus(this.body, () => this.draw(detail, colleagues, now));
  }

  private draw(detail: ResidentDetail, colleagues: ResidentNow[], now: Date): void {
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
    const section = (title: string, body: string, kind = "") =>
      body ? `<section class="details-section${kind}"><h3 class="details-title">${escape(title)}</h3>${body}</section>` : "";
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
              `<li><a class="details-job" href="${escape(fileLink(job.machine, job.output, true))}">${escape(job.job)}</a><span class="details-faint">${escape(`${job.machine} · ${ago(job.started_at)}`)}</span><code>${escape(job.command)}</code></li>`,
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
      ${section(words.manage, this.manageBody(r), " details-manage")}
      ${section(words.task, task)}
      ${section(words.colleagues, asked)}
      ${section(words.running, running)}
      ${section(words.actions(detail.actions.length, detail.actions_total), actions)}
      <p class="details-records"><span>${escape(words.records)}</span><code>${escape(`${about.machine}:${detail.records}`)}</code></p>`;
  }

  /** Sleep or wake, move to a project, remove: what the World can do to
   *  it for the user, and the projects to move it to when asked. */
  private manageBody(r: ResidentNow): string {
    const { words, manage } = this.options;
    if (!manage) return "";
    const doing = this.managing;
    const busy = r.now?.activity === "busy";
    const button = (act: string, icon: Parameters<typeof glyph>[0], label: string, disabled: boolean, extra = "") =>
      `<button class="text-button" type="button" data-act="${act}"${disabled ? " disabled" : ""}${extra}>${glyph(icon, 14)}<span>${escape(label)}</span></button>`;
    const rest =
      r.now?.activity === "asleep"
        ? button("wake", "wake", doing === "wake" ? words.working : words.wake, doing !== null)
        : button(
            "sleep",
            "sleep",
            doing === "sleep" ? words.working : words.sleep,
            doing !== null || busy,
            busy ? ` title="${escape(words.cannotSleepBusy)}"` : "",
          );
    const move = button(
      "move",
      "move",
      doing === "move" ? words.working : words.move,
      doing !== null,
      ` aria-expanded="${this.moving}"`,
    );
    const remove = `<button class="text-button details-remove" type="button" data-act="remove"${doing !== null ? " disabled" : ""}>${glyph("archive", 14)}<span>${escape(words.remove)}</span></button>`;
    let projects = "";
    if (this.moving) {
      const towns = manage.towns(r.about.machine);
      const choices = towns.map(
        (t) =>
          `<button type="button" class="choice${t.town === r.town ? " chosen" : ""}" data-act="move-to" data-town="${escape(t.town)}"${t.town === r.town ? ` disabled aria-current="true"` : ""}>${escape(t.name)}${t.town === r.town ? `<span class="choice-note">${escape(words.inHere)}</span>` : ""}</button>`,
      );
      if (r.town) choices.push(`<button type="button" class="choice" data-act="move-to" data-town="">${escape(words.leaveProject)}</button>`);
      const elsewhere = towns.some((t) => t.town !== r.town) || r.town;
      projects = `<div class="details-move" role="group" aria-label="${escape(words.moveTo)}"><span class="details-faint">${escape(words.moveTo)}</span>${
        elsewhere ? `<div class="choice-options">${choices.join("")}</div>` : `<span class="choice-none">${escape(words.noOtherProject)}</span>`
      }</div>`;
    }
    return `<div class="details-acts">${rest}${move}${remove}</div>${projects}<p class="details-note details-manage-note">${escape(this.manageNote)}</p>`;
  }
}

const pad = (n: number) => String(n).padStart(2, "0");
const clock = (at: Date) => `${pad(at.getHours())}:${pad(at.getMinutes())}`;
