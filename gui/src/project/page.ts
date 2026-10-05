// A project's overview, on one screen: what it is, who is in it now, its
// journals, its decisions and where its things are, as its map has them.
// Side by side where there is room, each column scrolling on its own;
// behind tabs on a narrow screen. Like the front page, every line is the
// World's record, nothing summarised.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { MapView, ProjectRecord, ResidentNow, RunningJob, WorldWho, Writer } from "../world/world";
import type { HomeWords } from "../home/words";
import type { ProjectWords } from "./words";
import { fileLink } from "../app/places";

export interface ProjectOptions {
  words: ProjectWords;
  home: HomeWords;
  town: string;
  head(title: string, actions?: string): string;
  /** What the actions here are called. */
  newTalk: string;
  edit: string;
  begin(town: string): void;
  /** Opens the form for the next version of its record. */
  revise(town: string, record: ProjectRecord): void;
  /** Opens the sheet that starts a journal. */
  startJournal(town: string): void;
  link(address: Address): string;
  journalLink(town: string, journal: string): string;
}

type Tab = "here" | "journals" | "decisions" | "places";

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class ProjectPage {
  readonly element: HTMLElement;
  private title: HTMLElement;
  private intro: HTMLElement;
  private tabs: HTMLElement;
  private board: HTMLElement;
  private tab: Tab = "here";
  private record: ProjectRecord | null = null;
  private drawn = "";

  constructor(private readonly options: ProjectOptions) {
    this.element = document.createElement("div");
    this.element.className = "project-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title"></h1>`,
        `<button class="text-button revise" type="button" disabled>${glyph("edit", 16)}<span>${escape(options.edit)}</span></button><button class="text-button begin-here" type="button">${glyph("plus", 16)}<span>${escape(options.newTalk)}</span></button>`,
      )}
      <header class="project-intro"></header>
      <div class="tabs" role="tablist"></div>
      <div class="project-board"></div>`;
    this.title = this.element.querySelector<HTMLElement>(".page-title")!;
    this.title.textContent = options.town;
    this.intro = this.element.querySelector<HTMLElement>(".project-intro")!;
    this.tabs = this.element.querySelector<HTMLElement>(".tabs")!;
    this.board = this.element.querySelector<HTMLElement>(".project-board")!;
    this.element.querySelector(".begin-here")!.addEventListener("click", () => options.begin(options.town));
    this.element.querySelector(".revise")!.addEventListener("click", () => {
      if (this.record) options.revise(options.town, this.record);
    });
    this.board.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".start-journal")) options.startJournal(options.town);
    });
    this.tabs.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-tab]");
      if (!button) return;
      this.tab = button.dataset.tab as Tab;
      this.choose();
    });
  }

  update(map: MapView, who: WorldWho, now: Date): void {
    const { words } = this.options;
    const residents = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []));
    const here = residents.filter((r) => r.here);
    const jobs = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
    const record = map.record;
    this.record = record;
    this.element.querySelector<HTMLButtonElement>(".revise")!.disabled = false;
    this.title.textContent = record.name;
    const intro = `
      <p class="project-lede">${escape(record.what)}</p>
      <p class="project-facts">${[
        record.machines.join(" · "),
        words.opened(this.writer(map.opened.by), map.opened.on, words.day(new Date(map.opened.at))),
      ]
        .map((fact) => `<span class="fact">${escape(fact)}</span>`)
        .join(`<span class="sep">·</span>`)}</p>`;
    const sections: [Tab, string, string][] = [
      [
        "here",
        words.here,
        (here.length
          ? `<ul class="people">${here.map((r) => this.someone(r, now)).join("")}</ul>`
          : `<p class="quiet">${escape(words.nobodyHere)}</p>`) +
          (jobs.length
            ? `<h3 class="project-subtitle">${escape(words.running)}</h3><ul class="jobs">${jobs
                .map((job) => this.job(job, residents, now))
                .join("")}</ul>`
            : ""),
      ],
      [
        "journals",
        words.journals,
        map.journals.length
          ? `<ul class="journals">${map.journals
              .map(
                (journal) => `<li><a class="journal" href="${escape(this.options.journalLink(map.town, journal.journal))}">
                  <span class="journal-name">${escape(journal.journal)}</span>
                  ${journal.description ? `<span class="journal-about">${escape(journal.description.text)}</span>` : ""}
                  <span class="journal-facts">${[
                    words.entries(journal.entries),
                    words.writers(journal.writers),
                    journal.latest
                      ? words.latest(this.writer(journal.latest.by), this.options.home.ago(now.getTime() - Date.parse(journal.latest.at)))
                      : "",
                  ]
                    .filter(Boolean)
                    .map(escape)
                    .join(`<span class="sep">·</span>`)}</span>
                </a></li>`,
              )
              .join("")}</ul>`
          : `<p class="quiet">${escape(words.noJournals)}</p>`,
      ],
      ["decisions", words.decisions, this.canon(map)],
      ["places", words.places, record.places.length ? this.places(map) : `<p class="quiet">${escape(words.noPlaces)}</p>`],
    ];
    const startJournal = `<button class="text-button start-journal" type="button">${glyph("plus", 16)}<span>${escape(words.newJournal)}</span></button>`;
    const section = ([tab, title, body]: [Tab, string, string]) =>
      `<section class="home-section" data-tab="${tab}"><h2 class="section-title">${escape(title)}</h2>${body}${tab === "journals" ? startJournal : ""}</section>`;
    const board = `<div class="board-column scroller">${section(sections[0])}</div><div class="board-column scroller">${sections
      .slice(1)
      .map(section)
      .join("")}</div>`;
    const tabs = sections
      .map(([tab, title]) => `<button class="tab" type="button" role="tab" data-tab="${tab}">${escape(title)}</button>`)
      .join("");
    const html = intro + tabs + board;
    if (html !== this.drawn) {
      this.drawn = html;
      this.intro.innerHTML = intro;
      this.tabs.innerHTML = tabs;
      this.board.innerHTML = board;
      this.choose();
    }
  }

  /** The project could not be asked: said under its name, with what was
   *  last shown left as it was. */
  unreachable(why: string): void {
    this.intro.innerHTML = `<p class="project-facts unreachable">${escape(this.options.home.unreachable(why))}</p>`;
    this.drawn = "";
  }

  private choose(): void {
    for (const element of this.element.querySelectorAll<HTMLElement>("[data-tab]")) {
      const chosen = element.dataset.tab === this.tab;
      element.classList.toggle("chosen", chosen);
      if (element.getAttribute("role") === "tab") element.setAttribute("aria-selected", String(chosen));
    }
  }

  private writer(writer: Writer): string {
    if (writer.kind === "user") return this.options.words.you;
    if (writer.kind === "resident") return `${writer.address.resident} · ${writer.address.machine}`;
    return agentName(writer.agent);
  }

  private someone(r: ResidentNow, now: Date): string {
    const home = this.options.home;
    const busy = r.now?.activity === "busy";
    const waiting = r.now?.waiting_for_user ?? false;
    const mark = waiting
      ? `<span class="ask" aria-hidden="true"></span>`
      : busy
        ? `<span class="pulse" aria-hidden="true"></span>`
        : `<span class="still" aria-hidden="true"></span>`;
    const doing = waiting
      ? home.waitingFor
      : r.last_action
        ? `${r.last_action.machine !== r.about.machine ? `${r.last_action.machine} · ` : ""}${r.last_action.what}`
        : "";
    const origin =
      r.about.origin.kind === "colleague" ? `<span class="someone-origin">${escape(r.about.origin.label)}</span>` : "";
    const since = r.now?.since ?? r.about.born_at;
    return `<li><a class="someone${waiting ? " waiting" : ""}" href="${escape(this.options.link({ resident: r.about.id, machine: r.about.machine }))}">
      <span class="rail">${mark}</span>
      <span class="someone-body">
        <span class="someone-line"><span class="someone-name">${escape(home.someone(agentName(r.about.host.agent), r.about.machine))}</span><span class="someone-since">${escape(busy || waiting ? home.for(home.lasted(now.getTime() - Date.parse(since))) : home.ago(now.getTime() - Date.parse(since)))}</span></span>
        ${origin}${doing ? `<span class="someone-doing">${escape(doing)}</span>` : ""}
      </span>
      <span class="someone-go">${glyph("chevron", 16)}</span>
    </a></li>`;
  }

  /** Something still running in the town: where, for how long, what, and
   *  who started it, said when they have gone. */
  private job(job: RunningJob, residents: ResidentNow[], now: Date): string {
    const { words, home } = this.options;
    const owner = residents.find((r) => r.about.id === job.owner.resident && r.about.machine === job.owner.machine);
    const name = owner ? home.someone(agentName(owner.about.host.agent), owner.about.machine) : `${job.owner.resident} · ${job.owner.machine}`;
    const by = owner?.here ? home.startedBy(name) : `${home.startedBy(name)} · ${words.ownerLeft}`;
    return `<li class="job"><a class="job-id" href="${escape(fileLink(job.machine, job.output, true))}">${escape(job.job)}</a><span class="job-where">${escape(job.machine)} · ${escape(home.for(home.lasted(now.getTime() - Date.parse(job.started_at))))}</span><code class="job-command">${escape(job.command)}</code><span class="detail">${escape(by)}</span></li>`;
  }

  private canon(map: MapView): string {
    const { words } = this.options;
    if (!map.canon) return `<p class="quiet">${escape(words.noDecisions)}</p>`;
    if ("Err" in map.canon && map.canon.Err !== undefined) {
      return `<p class="quiet caution">${escape(words.decisionsUnreadable(map.canon.Err))}</p>`;
    }
    const heads = map.canon.Ok ?? [];
    const canon = map.record.canon;
    const where = canon ? `${canon.machine} · ${canon.path}` : "";
    const docLink = (file: string) => (canon ? fileLink(canon.machine, `${canon.path.replace(/[\\/]+$/, "")}/${file}`) : null);
    return `<ul class="canon">${heads
      .map((head) => {
        const inner = `<span class="canon-title">${escape(head.title)}</span><span class="canon-scope">${escape(head.scope)}</span>`;
        const href = docLink(head.file);
        return `<li class="canon-doc">${href ? `<a class="canon-open" href="${escape(href)}">${inner}</a>` : inner}</li>`;
      })
      .join("")}</ul>${where ? `<p class="canon-where">${escape(where)}</p>` : ""}`;
  }

  private places(map: MapView): string {
    return `<ul class="places">${map.record.places
      .map(
        (place) =>
          `<li class="place-line"><span class="place-what">${escape(place.what)}</span><span class="place-where"><span class="place-machine">${escape(place.machine)}</span><code>${escape(place.path)}</code></span></li>`,
      )
      .join("")}</ul>`;
  }
}
