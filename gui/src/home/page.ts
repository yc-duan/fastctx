// The World's front page: what the reader finds on coming back. A drawing
// of the World as it is, the day and how long they were away, who is
// waiting on them, who is at work, what happened meanwhile, and the
// projects and machines as an index. Every line is a fact the World holds;
// nothing is summarised by a model.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { ResidentNow, RunningJob } from "../world/world";
import type { Happening, WorldState } from "./model";
import { drawFrontispiece } from "./frontispiece";
import type { HomeWords } from "./words";

export interface HomeOptions {
  words: HomeWords;
  /** The conversation words, for how an ending is said. */
  ended(how: string): string;
  world: WorldState;
  now: Date;
  lastVisit: Date | null;
  /** Where a resident's conversation opens. */
  link(address: Address): string;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class HomePage {
  readonly element: HTMLElement;

  constructor(private readonly options: HomeOptions) {
    this.element = document.createElement("div");
    this.element.className = "page home-page";
    this.draw();
  }

  private draw(): void {
    const { world, words } = this.options;
    const residents = this.residents();
    const jobs = world.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
    const waiting = residents.filter((r) => r.now?.waiting_for_user);
    const busy = residents.filter((r) => r.now?.activity === "busy" && !r.now.waiting_for_user);
    const idle = residents.filter((r) => r.here && r.now?.activity !== "busy");
    const present = new Set(residents.filter((r) => r.here).map((r) => `${r.about.id}@${r.about.machine}`));
    const unattended = jobs.filter((job) => !present.has(`${job.owner.resident}@${job.owner.machine}`));
    const online = world.nodes.filter((node) => node.online).length;

    this.element.innerHTML = `
      <div class="home">
        <figure class="frontispiece" aria-hidden="true"></figure>
        <header class="dateline">
          <h1 class="dateline-day">${escape(words.dateline(this.options.now))}</h1>
          <p class="dateline-facts">${[
            this.options.lastVisit
              ? words.lastHere(words.ago(this.options.now.getTime() - this.options.lastVisit.getTime()))
              : words.firstTime,
            words.machinesOnline(online, world.nodes.length),
            words.agentsWorking(busy.length + waiting.length),
          ]
            // A separator keeps to the fact after it, so no line starts with one.
            .map((fact, index) => `<span class="fact">${index ? `<span class="sep">·</span>` : ""}${escape(fact)}</span> `)
            .join("")}</p>
        </header>
        ${waiting.length ? this.section("waiting", words.waiting, `<ul class="people">${waiting.map((r) => this.someone(r, true)).join("")}</ul>`) : ""}
        ${this.section(
          "now",
          words.now,
          busy.length
            ? `<ul class="people">${busy.map((r) => this.someone(r, false)).join("")}</ul>${idle.length ? `<p class="idle">${escape(words.idle(idle.map((r) => this.name(r)).join("、")))}</p>` : ""}`
            : `<p class="quiet">${escape(words.nobodyWorking)}</p>`,
        )}
        ${this.section("away", words.away, this.chronicle())}
        ${unattended.length ? this.section("unattended", words.unattended, `<ul class="jobs">${unattended.map((job) => this.job(job)).join("")}</ul>`) : ""}
        ${this.section("projects", words.projects, `<ul class="projects">${world.towns.map((town) => this.project(town, residents)).join("")}</ul>`)}
        ${this.section("machines", words.machines, `<ul class="machines">${world.nodes.map((node) => this.machine(node, residents, jobs)).join("")}</ul>`)}
      </div>`;
    drawFrontispiece(this.element.querySelector<HTMLElement>(".frontispiece")!, world, residents);
  }

  private residents(): ResidentNow[] {
    return this.options.world.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []));
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
    return this.options.words.lasted(this.options.now.getTime() - Date.parse(at));
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
      ? `<span class="someone-trouble">${escape(r.now.trouble.text)} · ${escape(words.ago(this.options.now.getTime() - Date.parse(r.now.trouble.at)))}</span>`
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

  private chronicle(): string {
    const { words, now } = this.options;
    const since = this.options.lastVisit?.getTime() ?? 0;
    const events = this.options.world.happenings
      .filter((event) => Date.parse(event.at) >= since)
      .sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
    if (!events.length) return `<p class="quiet">${escape(words.nothingAway)}</p>`;
    let day = "";
    const items: string[] = [];
    for (const event of events) {
      const at = new Date(event.at);
      const thisDay = words.day(at);
      if (thisDay !== day && (day || thisDay !== words.day(now))) items.push(`<li class="day">${escape(thisDay)}</li>`);
      day = thisDay;
      items.push(this.happening(event, at));
    }
    return `<ol class="chronicle">${items.join("")}</ol>`;
  }

  private happening(event: Happening, at: Date): string {
    const { words } = this.options;
    const time = `<time datetime="${escape(event.at)}">${escape(words.clock(at))}</time>`;
    const town = "town" in event && event.town ? `<span class="tag">${escape(event.town)}</span>` : "";
    let mark = "colleague";
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
        detail = `<q class="said entry">${escape(event.first)}</q>${event.by ? `<span class="detail">${escape(this.nameOf(event.by))}</span>` : ""}`;
        mark = "read";
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
    return `<li class="event${tone}">${time}<span class="rail">${glyph(mark as Parameters<typeof glyph>[0], 18)}</span><div class="event-body"><span class="event-line">${escape(line)}${town}</span>${detail}</div></li>`;
  }

  private job(job: RunningJob): string {
    const { words } = this.options;
    return `<li class="job"><span class="job-id">${escape(job.job)}</span><span class="job-where">${escape(job.machine)} · ${escape(this.since(job.started_at))}</span><code class="job-command">${escape(job.command)}</code><span class="detail">${escape(words.startedBy(this.nameOf(job.owner)))}</span></li>`;
  }

  private project(town: WorldState["towns"][number], residents: ResidentNow[]): string {
    const { words } = this.options;
    const here = residents.filter((r) => r.town === town.town && r.here).length;
    const facts = [town.record.machines.join(" · "), words.here(here), words.journals(town.journals)];
    return `<li><a class="project" href="#/project/${encodeURIComponent(town.town)}">
      <span class="project-name">${escape(town.record.name)}</span>
      <span class="project-what">${escape(town.record.what)}</span>
      <span class="project-facts">${facts.map(escape).join("<span class=\"sep\">·</span>")}</span>
    </a></li>`;
  }

  private machine(node: WorldState["nodes"][number], residents: ResidentNow[], jobs: RunningJob[]): string {
    const { words, now } = this.options;
    const os = "Ok" in node.facts ? node.facts.Ok.os : "";
    const state =
      node.online === true
        ? words.online
        : node.online === false
          ? words.offlineSince(node.since ? words.ago(now.getTime() - Date.parse(node.since)) : "")
          : words.unknownLink;
    const on = residents.filter((r) => r.about.machine === node.name && r.here).length;
    const running = jobs.filter((job) => job.machine === node.name).length;
    const facts = [os, on ? words.residents(on) : "", running ? words.jobsRunning(running) : ""].filter(Boolean);
    const here = node.name === this.options.world.here ? " here" : "";
    return `<li class="machine${node.online ? " online" : " offline"}${here}"><span class="dot" aria-hidden="true"></span><span class="machine-name">${escape(node.name)}</span><span class="machine-state">${escape(state)}</span><span class="machine-facts">${facts.map(escape).join("<span class=\"sep\">·</span>")}</span></li>`;
  }
}
