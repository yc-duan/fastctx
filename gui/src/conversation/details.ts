// What the World knows of the agent in a conversation, opened from the
// conversation's head: who it is and where it lives and works, which login
// of its agent it uses there, whom it works for, how it came, what it is
// doing, the models it called, what it started that still runs, its latest
// actions, and what can be done to it. The same facts as `fastctx w <id>`,
// asked of the machine it lives on while the card is open. Which agents may
// call which models is the model collaboration feature's, never a single
// conversation's (`world-gui.md`, 功能).

import { escape } from "./prose";
import { glyph } from "./glyphs";
import type { Address } from "../world/events";
import type { ResidentDetail, ResidentNow } from "../world/world";
import type { DetailsWords } from "../app/words";
import { Confirm } from "../app/confirm";
import { redrawKeepingFocus } from "../app/focus";
import { fileLink } from "../app/places";
import { agentName, titleOf } from "../world/names";

/** What can be done to the agent beyond its conversation, as the user.
 *  Each rejects with what the World said when it was not done. */
/** What an act did not fully do, said stage by stage; empty when all of
 *  it was done. */
export type Unfinished = string[];

export interface Manage {
  /** Into a project, or out of any where null. */
  move(town: string | null): Promise<Unfinished>;
  remove(): Promise<Unfinished>;
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
  /** "hangzhou · ada" for each account it works for; empty for this
   *  computer's logins. */
  actingFor(resident: ResidentNow): string[];
  /** Who lent it, for an agent another account lent the user; null for
   *  one of the user's own. */
  lentBy?: ((resident: ResidentNow) => string | null) | undefined;
  /** The machines, as written, it may work on, and setting it: a machine,
   *  or with null the one it lives on. None for a colleague. */
  worksOn?: { places(): string[]; set(worksOn: string | null): Promise<void> } | undefined;
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

type Managing = "move" | "works";

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
  /** Whether the machines it may work on are shown. */
  private placing = false;
  private worksNote = "";
  private confirm: Confirm | null = null;
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
          return;
        case "works":
          this.placing = !this.placing;
          this.redraw();
          return;
        case "works-on": {
          const works = options.worksOn;
          if (!works || !this.shown) return;
          const place = button.dataset.place || null;
          const lives = this.shown.detail.resident.about.machine;
          this.placing = false;
          this.managing = "works";
          this.worksNote = "";
          this.redraw();
          works
            .set(place)
            .then(() => {
              const about = this.shown?.detail.resident.about;
              if (about && place) about.works_on = place;
              else if (about) delete about.works_on;
              this.worksNote = words.worksOnSet(place ?? lives, !place);
            })
            .catch((error: unknown) => {
              this.worksNote = words.failed(error instanceof Error ? error.message : String(error));
            })
            .finally(() => {
              this.managing = null;
              this.spoken.textContent = this.worksNote;
              this.redraw();
            });
          return;
        }
      }
    });
  }

  private say(text: string): void {
    this.note = text;
    this.spoken.textContent = text;
    const note = this.element.querySelector<HTMLElement>(".details-note");
    if (note) note.textContent = text;
  }

  private manageWith(what: Managing, act: () => Promise<Unfinished>, done: string): void {
    const { words } = this.options;
    this.managing = what;
    this.manageNote = "";
    this.redraw();
    act()
      .then((unfinished) => {
        this.manageNote = unfinished.length ? unfinished.join("；") : done;
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
    let unfinished: Unfinished = [];
    this.confirm.ask({
      title: words.removeTitle(who),
      what: words.removeWhat,
      go: words.removeGo,
      act: async () => {
        unfinished = await manage.remove();
      },
      // Removed from the World, but a step of it not done (a window the
      // World cannot close): that is said here before the conversation is
      // left behind.
      done: () => {
        if (!unfinished.length) manage.removed();
        else {
          this.manageNote = `${words.removedButNot}${unfinished.join("；")}`;
          this.spoken.textContent = this.manageNote;
          this.redraw();
        }
      },
    });
  }

  get open(): boolean {
    return !this.element.hidden;
  }

  set open(shown: boolean) {
    this.element.hidden = !shown;
    if (!shown) {
      this.moving = false;
      this.placing = false;
    }
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
    const lentBy = this.options.lentBy?.(r) ?? null;
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
      [words.lentBy, lentBy ?? ""],
      [words.actingFor, acting.join(words.separator)],
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
              `<li><a href="${this.options.link({ resident: c.about.id, machine: c.about.machine })}"><span class="mark ${c.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span><span>${escape(titleOf(c) ?? c.about.id)}</span><span class="details-faint">${escape(`${agentName(c.about.host.agent)} · ${c.about.machine}`)}</span></a></li>`,
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
      ${section(words.worksOn, this.worksBody(r))}
      ${section(words.task, task)}
      ${section(words.colleagues, asked)}
      ${section(words.running, running)}
      ${section(words.actions(detail.actions.length, detail.actions_total), actions)}
      <p class="details-records"><span>${escape(words.records)}</span><code>${escape(`${about.machine}:${detail.records}`)}</code></p>`;
  }

  /** Where its commands start, with the machines to choose from when
   *  asked; a colleague works where it lives. */
  private worksBody(r: ResidentNow): string {
    const { words, worksOn } = this.options;
    const about = r.about;
    if (this.options.lentBy?.(r)) return `<p class="details-colleagues"><span>${escape(words.worksLent)}</span></p>`;
    const elsewhere = about.works_on && about.works_on !== about.machine ? about.works_on : null;
    // Only a conversation the World opened chose where it works.
    const changeable = !!worksOn && !r.removed && about.origin.kind !== "colleague" && about.world_started;
    const doing = this.managing === "works";
    const said = `<p class="details-colleagues"><span>${escape(elsewhere ? words.worksOnThere(elsewhere) : words.worksOnLives)}</span>${
      changeable
        ? `<button class="text-button" type="button" data-act="works" aria-expanded="${this.placing}"${doing ? " disabled" : ""}>${glyph("machine", 14)}<span>${escape(doing ? words.working : words.changeWorksOn)}</span></button>`
        : ""
    }</p>`;
    const choices =
      this.placing && worksOn
        ? `<div class="details-move" role="group" aria-label="${escape(words.worksOn)}"><span class="details-faint">${escape(words.worksOn)}</span><div class="choice-options">${[
            `<button type="button" class="choice${elsewhere ? "" : " chosen"}" data-act="works-on" data-place=""${elsewhere ? "" : ` disabled aria-current="true"`}>${escape(words.livesHere(about.machine))}</button>`,
            ...worksOn
              .places()
              .filter((machine) => machine !== about.machine)
              .map(
                (machine) =>
                  `<button type="button" class="choice${machine === elsewhere ? " chosen" : ""}" data-act="works-on" data-place="${escape(machine)}"${machine === elsewhere ? ` disabled aria-current="true"` : ""}>${escape(machine)}</button>`,
              ),
          ].join("")}</div></div>`
        : "";
    return `${said}${choices}${this.worksNote ? `<p class="details-note">${escape(this.worksNote)}</p>` : ""}`;
  }

  /** Move to a project, remove: what the World can do to it for the user,
   *  and the projects to move it to when asked. */
  private manageBody(r: ResidentNow): string {
    const { words, manage } = this.options;
    if (!manage || r.removed) return "";
    const doing = this.managing;
    const button = (act: string, icon: Parameters<typeof glyph>[0], label: string, disabled: boolean, extra = "") =>
      `<button class="text-button" type="button" data-act="${act}"${disabled ? " disabled" : ""}${extra}>${glyph(icon, 14)}<span>${escape(label)}</span></button>`;
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
    return `<div class="details-acts">${move}${remove}</div>${projects}<p class="details-note details-manage-note">${escape(this.manageNote)}</p>`;
  }
}

const pad = (n: number) => String(n).padStart(2, "0");
const dirOf = (dir: string) => dir.split(/[\\/]+/).filter(Boolean).pop() ?? dir;
const clock = (at: Date) => `${pad(at.getHours())}:${pad(at.getMinutes())}`;
