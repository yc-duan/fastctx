// The World's front page: what the reader finds on coming back, on one
// screen. A drawing of the World as it is; the day and how long they were
// away; then, side by side where there is room, who is waiting on them and
// who is at work, and what happened meanwhile. Each of those scrolls on its
// own; the drawing and the day stay. Every line is a fact the World holds;
// nothing is summarised by a model. Projects and machines are in the
// sidebar, not here.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { ResidentNow, RunningJob, Writer } from "../world/world";
import { Frontispiece } from "./frontispiece";
import type { Away, Happening, WorldState } from "./model";
import type { HomeWords } from "./words";

export interface HomeOptions {
  words: HomeWords;
  /** The conversation words, for how an ending is said. */
  ended(how: string): string;
  /** When the reader last looked at this page, when known. */
  lastVisit: Date | null;
  /** Where a resident's conversation opens. */
  link(address: Address): string;
  /** The head every page has, with the button that brings back the sidebar. */
  head: string;
  /** Begins a conversation. */
  newTalk(): void;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class HomePage {
  readonly element: HTMLElement;
  private world: WorldState | null = null;
  private now = new Date();
  private drawn = "";
  private frontispiece: Frontispiece;
  private day: HTMLElement;
  private facts: HTMLElement;
  private present: HTMLElement;
  private away: HTMLElement;

  constructor(private readonly options: HomeOptions) {
    this.element = document.createElement("div");
    this.element.className = "home-page";
    this.element.innerHTML = `
      ${options.head}
      <figure class="frontispiece" aria-hidden="true"></figure>
      <header class="dateline">
        <h1 class="dateline-day"></h1>
        <p class="dateline-facts"></p>
      </header>
      <div class="board">
        <div class="board-column present scroller"></div>
        <div class="board-column away scroller"></div>
      </div>`;
    this.frontispiece = new Frontispiece(this.element.querySelector<HTMLElement>(".frontispiece")!);
    this.day = this.element.querySelector<HTMLElement>(".dateline-day")!;
    this.facts = this.element.querySelector<HTMLElement>(".dateline-facts")!;
    this.present = this.element.querySelector<HTMLElement>(".present")!;
    this.away = this.element.querySelector<HTMLElement>(".away")!;
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".begin")) options.newTalk();
    });
  }

  /** Shows the World as it now stands; when nothing shown has changed, the
   *  page is left as it is. */
  update(world: WorldState, now: Date): void {
    this.world = world;
    this.now = now;
    const { words } = this.options;
    this.day.textContent = words.dateline(now);

    const residents = this.residents();
    const jobs = world.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
    const waiting = residents.filter((r) => r.now?.waiting_for_user);
    const busy = residents.filter((r) => r.now?.activity === "busy" && !r.now.waiting_for_user);
    const idle = residents.filter((r) => r.here && r.now?.activity !== "busy");
    const present = new Set(residents.filter((r) => r.here).map((r) => `${r.about.id}@${r.about.machine}`));
    const unattended = jobs.filter((job) => !present.has(`${job.owner.resident}@${job.owner.machine}`));
    // Machines are what the World works on; a phone is not counted among them.
    const machines = world.nodes.filter((node) => !node.device);
    const online = machines.filter((node) => node.online).length;
    const idleLine = idle.length
      ? `<p class="idle">${escape(words.idle(idle.map((r) => this.name(r)).join(words.listSeparator)))}</p>`
      : "";

    const facts = [
      this.options.lastVisit ? words.lastHere(words.ago(now.getTime() - this.options.lastVisit.getTime())) : words.firstTime,
      words.machinesOnline(online, machines.length),
      words.agentsWorking(busy.length + waiting.length),
    ];
    // A separator keeps to the fact before it, so no line starts with one.
    const factsHtml = facts
      .map(
        (fact, index) =>
          `<span class="fact">${escape(fact)}${index < facts.length - 1 ? `<span class="sep">·</span>` : ""}</span> `,
      )
      .join("");

    const presentHtml = [
      waiting.length
        ? this.section("waiting", words.waiting, `<ul class="people">${waiting.map((r) => this.someone(r, true)).join("")}</ul>`)
        : "",
      this.section(
        "now",
        words.now,
        busy.length
          ? `<ul class="people">${busy.map((r) => this.someone(r, false)).join("")}</ul>${idleLine}`
          : `<p class="quiet">${escape(words.nobodyWorking)} <button class="begin" type="button">${escape(words.begin)}</button></p>${idleLine}`,
      ),
      unattended.length
        ? this.section("unattended", words.unattended, `<ul class="jobs">${unattended.map((job) => this.job(job)).join("")}</ul>`)
        : "",
    ].join("");
    // What happened meanwhile is shown only where the World can say it.
    const awayHtml = world.away ? this.section("away", words.away, this.chronicle(world.away)) : "";

    const drawn = factsHtml + presentHtml + awayHtml;
    if (drawn !== this.drawn) {
      this.drawn = drawn;
      this.facts.innerHTML = factsHtml;
      this.present.innerHTML = presentHtml;
      this.away.innerHTML = awayHtml;
      this.away.hidden = !awayHtml;
    }
    // The drawing is of the machines the World works on.
    this.frontispiece.update({ ...world, nodes: world.nodes.filter((node) => !node.device) }, residents);
  }

  /** The World could not be asked: said where the facts would be, with
   *  what was last shown left as it was. */
  unreachable(why: string, now: Date): void {
    this.day.textContent = this.options.words.dateline(now);
    this.facts.innerHTML = `<span class="fact unreachable">${escape(this.options.words.unreachable(why))}</span>`;
    this.drawn = "";
  }

  private residents(): ResidentNow[] {
    return (this.world?.machines ?? []).flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []));
  }

  private section(kind: string, title: string, body: string): string {
    return `<section class="home-section section-${kind}"><h2 class="section-title">${escape(title)}</h2>${body}</section>`;
  }

  private name(r: ResidentNow): string {
    return this.options.words.someone(agentName(r.about.host.agent), r.about.machine);
  }

  private nameOf(address: Address): string {
    const known = this.residents().find((r) => r.about.id === address.resident && r.about.machine === address.machine);
    return known ? this.name(known) : `${address.resident} · ${address.machine}`;
  }

  private since(at: string): string {
    return this.options.words.lasted(this.now.getTime() - Date.parse(at));
  }

  private someone(r: ResidentNow, waiting: boolean): string {
    const { words } = this.options;
    const origin =
      r.about.origin.kind === "colleague"
        ? `<span class="someone-origin">${escape(words.colleagueOf(this.nameOf(r.about.origin.asked_by)))} · ${escape(r.about.origin.label)}</span>`
        : "";
    const action = r.last_action;
    const doing = waiting
      ? `<span class="someone-doing">${escape(words.waitingFor)}</span>`
      : action
        ? `<span class="someone-doing">${action.machine !== r.about.machine ? `<span class="where">${escape(action.machine)}</span>` : ""}<span class="what">${escape(action.what)}</span></span>`
        : "";
    const trouble = r.now?.trouble
      ? `<span class="someone-trouble">${escape(r.now.trouble.text)} · ${escape(words.ago(this.now.getTime() - Date.parse(r.now.trouble.at)))}</span>`
      : "";
    const mark = waiting ? `<span class="ask" aria-hidden="true"></span>` : `<span class="pulse" aria-hidden="true"></span>`;
    return `<li><a class="someone${waiting ? " waiting" : ""}" href="${this.options.link({ machine: r.about.machine, resident: r.about.id })}">
      <span class="rail">${mark}</span>
      <span class="someone-body">
        <span class="someone-line"><span class="someone-name">${escape(this.name(r))}</span>${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}<span class="someone-since">${escape(words.for(this.since(r.now?.since ?? r.about.born_at)))}</span></span>
        ${origin}${doing}${trouble}
      </span>
      <span class="someone-go">${glyph("chevron", 16)}</span>
    </a></li>`;
  }

  private chronicle(away: Away): string {
    const { words } = this.options;
    const since = this.options.lastVisit?.getTime() ?? 0;
    const events = away.happenings
      .filter((event) => Date.parse(event.at) >= since)
      .sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
    // What could not be asked is said under what was.
    const missing = away.trouble
      ? `<p class="away-missing wrong">${escape(words.awayTrouble(away.trouble))}</p>`
      : away.unasked.length
        ? `<p class="away-missing">${escape(words.unasked(away.unasked.map((u) => (u.offline ? words.unaskedOffline(u.machine) : words.unaskedWhy(u.machine, u.why ?? ""))).join(words.listSeparator)))}</p>`
        : "";
    if (!events.length) return `<p class="quiet">${escape(away.trouble ? "" : words.nothingAway)}</p>${missing}`;
    let day = "";
    const items: string[] = [];
    for (const event of events) {
      const at = new Date(event.at);
      const thisDay = words.day(at);
      if (thisDay !== day && (day || thisDay !== words.day(this.now))) items.push(`<li class="day">${escape(thisDay)}</li>`);
      day = thisDay;
      items.push(this.happening(event, at));
    }
    return `<ol class="chronicle">${items.join("")}</ol>${missing}`;
  }

  /** Who wrote a record: the reader, an agent, or a session by its
   *  program. */
  private writer(by: Writer): string {
    switch (by.kind) {
      case "user":
        return this.options.words.you;
      case "resident":
        return this.nameOf(by.address);
      case "session":
        return by.agent;
    }
  }

  private happening(event: Happening, at: Date): string {
    const { words } = this.options;
    const time = `<time datetime="${escape(event.at)}">${escape(words.clock(at))}</time>`;
    const town = "town" in event && event.town ? `<span class="tag">${escape(event.town)}</span>` : "";
    let mark: Parameters<typeof glyph>[0] = "colleague";
    let line: string;
    let detail = "";
    let tone = "";
    switch (event.kind) {
      case "turn_ended":
        if (event.how === "completed") {
          line = words.turnEnded(this.nameOf(event.who));
          if (event.said) detail = `<q class="said">${escape(event.said)}</q>`;
        } else {
          line = words.turnFailed(this.nameOf(event.who), event.reason ?? this.options.ended(`turn ${event.how}`));
          tone = " wrong";
        }
        mark = "thinking";
        break;
      case "job_ended": {
        line = words.jobDone(event.job, event.machine);
        const failed = !/^exited 0$/.test(event.how);
        detail = `<span class="detail${failed ? " wrong" : ""}">${escape(this.options.ended(event.how))} · ${escape(words.startedBy(this.nameOf(event.owner)))}</span>`;
        mark = "command";
        break;
      }
      case "journal":
        line = words.journalAdded(event.journal);
        detail = `${event.first ? `<q class="said entry">${escape(event.first)}</q>` : ""}<span class="detail">${escape(this.writer(event.by))}</span>`;
        mark = "read";
        break;
      case "arrived":
        line = words.arrived(this.nameOf(event.who));
        mark = "plus";
        break;
      case "moved":
        line =
          event.from && event.to
            ? words.movedTo(this.nameOf(event.who), event.from, event.to)
            : event.to
              ? words.entered(this.nameOf(event.who), event.to)
              : words.leftProject(this.nameOf(event.who), event.from ?? "");
        mark = "colleague";
        break;
      case "project":
        line = event.version === 1 ? words.projectOpened(event.name) : words.projectEdited(event.name, event.version);
        detail = `<span class="detail">${escape(this.writer(event.by))}</span>`;
        mark = "edit";
        break;
      case "uncertain":
        line = words.uncertain(this.nameOf(event.who));
        detail = `<span class="detail">${escape(event.why)}</span>`;
        mark = "thinking";
        tone = " caution";
        break;
      case "undelivered":
        line = words.undelivered(this.nameOf(event.who), this.nameOf(event.to));
        detail = `<span class="detail">${escape(event.why)}</span>`;
        mark = "colleague";
        tone = " caution";
        break;
      case "asked":
        line = words.asked(this.nameOf(event.who), event.colleague.resident);
        detail = `<span class="detail">${escape(event.label)}</span>`;
        mark = "colleague";
        break;
      case "machine":
        line = event.online ? words.machineCame(event.machine) : words.machineLeft(event.machine);
        mark = "web";
        tone = event.online ? "" : " caution";
        break;
    }
    const href = "who" in event ? this.options.link(event.who) : null;
    const body = `${time}<span class="rail">${glyph(mark, 18)}</span><div class="event-body"><span class="event-line">${escape(line)}${town}</span>${detail}</div>`;
    return href
      ? `<li class="event${tone}"><a class="event-link" href="${href}">${body}</a></li>`
      : `<li class="event${tone}">${body}</li>`;
  }

  private job(job: RunningJob): string {
    const { words } = this.options;
    return `<li class="job"><span class="job-id">${escape(job.job)}</span><span class="job-where">${escape(job.machine)} · ${escape(this.since(job.started_at))}</span><code class="job-command">${escape(job.command)}</code><span class="detail">${escape(words.startedBy(this.nameOf(job.owner)))}</span></li>`;
  }
}
