// A project's overview, on one screen: what it is, who is in it now, the
// pages its residents wrote and where its things are, as its record has
// them; where project memory is on, its decisions and journals beside its
// record (`world-memory.md`). Its "⋯" shares it and hands it on, where the
// reader may (`../app/project-acts.ts`).
// Side by side where there is room, each column scrolling on its own;
// behind tabs on a narrow screen. Like the front page, every line is the
// World's record, nothing summarised.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { factRow } from "../design/fact-row";
import { redrawKeepingFocus } from "../app/focus";
import type { Address } from "../world/events";
import { ownerResident, type MapView, type ProjectRecord, type ResidentNow, type RunningJob, type WhoFor, type WorldWho, type Writer } from "../world/world";
import type { PageSummary, PageWriter } from "../world/pages";
import type { PageWords } from "../pages/words";
import type { HomeWords } from "../home/words";
import type { ProjectWords } from "./words";
import { fileLink } from "../app/places";
import { worksFor } from "../app/store";
import { accountTag } from "../home/page";
import { agentName, titleOf } from "../world/names";
import type { MemoryAuthor, MemoryCatalog, MemoryRevision } from "../world/memory";
import type { MemoryWords } from "./memory-words";

export interface ProjectOptions {
  words: ProjectWords;
  home: HomeWords;
  /** Who a conversation is with, where it has no name of its own. */
  someone(r: ResidentNow): string;
  town: string;
  head(title: string, actions?: string): string;
  /** What the actions here are called. */
  newTalk: string;
  edit: string;
  begin(town: string): void;
  /** Opens the form for the next version of its record. */
  revise(town: string, record: ProjectRecord): void;
  link(address: Address): string;
  pageWords: PageWords;
  pageLink(town: string, page: string): string;
  /** The project's "⋯": whether it has anything for the reader, opening
   *  it, and whose the project is where it is another's. */
  more?: { label: string; shown(town: string): boolean; open(town: string, name: string, anchor: HTMLElement): void; sharedBy(town: string): string | null };
  /** The project's memory, where the feature is on: where a document is
   *  read, and writing a journal. */
  memory?: { words: MemoryWords; link(town: string, name: string): string; write(town: string): void; writer(author: MemoryAuthor): string };
}

type Tab = "here" | "instructions" | "specs" | "journals" | "pages" | "places";

export class ProjectPage {
  readonly element: HTMLElement;
  private title: HTMLElement;
  private intro: HTMLElement;
  private tabs: HTMLElement;
  private board: HTMLElement;
  private tab: Tab = "here";
  private record: ProjectRecord | null = null;
  /** What runs in the town for other accounts than the reader's. */
  private others: WhoFor[] | undefined;
  private drawn = "";
  /** Its memory as last read; none while the feature is off or unread. */
  private catalog: MemoryCatalog | null = null;
  private last: { map: MapView; who: WorldWho; now: Date } | null = null;

  constructor(private readonly options: ProjectOptions) {
    this.element = document.createElement("div");
    this.element.className = "project-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title"></h1>`,
        `<button class="text-button revise" type="button" disabled>${glyph("edit", 16)}<span>${escape(options.edit)}</span></button><button class="text-button begin-here" type="button">${glyph("plus", 16)}<span>${escape(options.newTalk)}</span></button>${options.more ? `<button class="icon-button project-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(options.more.label)}" title="${escape(options.more.label)}" hidden>${glyph("more", 18)}</button>` : ""}`,
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
    this.element.querySelector(".project-more")?.addEventListener("click", (event) => {
      options.more?.open(options.town, this.record?.name ?? options.town, event.currentTarget as HTMLElement);
    });
    this.element.querySelector(".revise")!.addEventListener("click", () => {
      if (this.record) options.revise(options.town, this.record);
    });
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-write-journal]")) options.memory?.write(options.town);
    });
    this.tabs.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-tab]");
      if (!button) return;
      this.tab = button.dataset.tab as Tab;
      this.choose();
    });
  }

  /** The project's memory, read again; null where it is off. */
  showMemory(catalog: MemoryCatalog | null): void {
    this.catalog = catalog;
    if (this.last) this.update(this.last.map, this.last.who, this.last.now);
  }

  update(map: MapView, who: WorldWho, now: Date): void {
    this.last = { map, who, now };
    const { words } = this.options;
    // Archived ones are still named where something of theirs runs on.
    const known = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []));
    const residents = known.filter((r) => !r.removed);
    // Every conversation of the project, while it exists (walkthrough items 20, 22).
    const here = residents;
    const jobs = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
    this.others = who.others;
    const record = map.record;
    this.record = record;
    this.element.querySelector<HTMLButtonElement>(".revise")!.disabled = false;
    const more = this.element.querySelector<HTMLButtonElement>(".project-more");
    if (more) more.hidden = !this.options.more?.shown(map.town);
    this.title.textContent = record.name;
    const sharedBy = this.options.more?.sharedBy(map.town) ?? null;
    const intro = `
      <p class="project-facts">${factRow(
        [...record.machines, words.opened(this.writer(map.opened.by), map.opened.on, words.day(new Date(map.opened.at))), ...(sharedBy ? [sharedBy] : [])].map(escape),
      )}</p>`;
    const sections: [Tab, string, string][] = [
      [
        "here",
        words.here,
        (here.length
          ? `<ul class="people">${here.map((r) => this.someone(r, now)).join("")}</ul>`
          : `<p class="quiet">${escape(words.nobodyHere)}</p>`) +
          (jobs.length
            ? `<h3 class="project-subtitle">${escape(words.running)}</h3><ul class="jobs">${jobs
                .map((job) => this.job(job, known, now))
                .join("")}</ul>`
            : ""),
      ],
      [
        "instructions",
        words.instructions,
        record.what.trim()
          ? `<p class="project-instructions">${escape(record.what.trim())}</p>`
          : `<p class="quiet">${escape(words.noInstructions)}</p>`,
      ],
      ...this.memorySections(map.town, now),
      ["pages", this.options.pageWords.pages, this.pages(map.town, map.pages, now)],
      ["places", words.places, record.places.length ? this.places(map) : `<p class="quiet">${escape(words.noPlaces)}</p>`],
    ];
    const section = ([tab, title, body]: [Tab, string, string]) =>
      `<section class="home-section" data-tab="${tab}"><h2 class="section-title">${escape(title)}</h2>${body}</section>`;
    const board = `<div class="board-column scroller">${section(sections[0])}</div><div class="board-column scroller">${sections
      .slice(1)
      .map(section)
      .join("")}</div>`;
    const short: Partial<Record<Tab, string>> = { here: words.hereTab, places: words.placesTab };
    const tabs = sections
      .map(([tab, title]) => `<button class="tab" type="button" role="tab" data-tab="${tab}">${escape(short[tab] ?? title)}</button>`)
      .join("");
    const html = intro + tabs + board;
    if (html !== this.drawn) {
      this.drawn = html;
      redrawKeepingFocus(this.element, () => {
        this.intro.innerHTML = intro;
        this.tabs.innerHTML = tabs;
        this.board.innerHTML = board;
        this.choose();
      });
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

  private writer(writer: Writer | PageWriter): string {
    if (writer.kind === "user") return this.options.words.you;
    if (writer.kind === "resident") return `${writer.address.resident} ${writer.address.machine}`;
    if (writer.kind === "page") return writer.page.page;
    return agentName(writer.agent);
  }

  /** The pages, the latest changed first; a page taken down stays, faint,
   *  so it can be brought back. */
  private pages(town: string, pages: PageSummary[], now: Date): string {
    const { pageWords: words, home } = this.options;
    if (!pages.length) return `<p class="quiet">${escape(words.noPages)}</p>`;
    const sorted = [...pages].sort((a, b) => Date.parse(b.current.at) - Date.parse(a.current.at));
    return `<ul class="page-links">${sorted
      .map(({ page, current }) => {
        const facts = [words.version(current.version), words.written(this.writer(current.by), home.ago(now.getTime() - Date.parse(current.at)))];
        return `<li><a class="page-link${current.withdrawn ? " withdrawn" : ""}" href="${escape(this.options.pageLink(town, page))}">
          <span class="page-link-glyph">${glyph("page", 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(current.title || page)}${current.withdrawn ? `<span class="tag">${escape(words.withdrawnMark)}</span>` : ""}</span>
            ${current.description ? `<span class="page-link-about">${escape(current.description)}</span>` : ""}
            <span class="page-link-facts">${facts.map(escape).join(`<span class="sep"></span>`)}</span>
          </span>
        </a></li>`;
      })
      .join("")}</ul>`;
  }

  /** Its decisions, then its journals, the latest written first. */
  private memorySections(town: string, now: Date): [Tab, string, string][] {
    const memory = this.options.memory;
    const catalog = this.catalog;
    if (!memory || !catalog) return [];
    const words = memory.words;
    const list = (revisions: MemoryRevision[], icon: "spec" | "journal", empty: string) => {
      if (!revisions.length) return `<p class="quiet">${escape(empty)}</p>`;
      const sorted = [...revisions].sort((a, b) => Date.parse(b.author.at) - Date.parse(a.author.at));
      return `<ul class="page-links">${sorted
        .map((one) => {
          const facts = [
            ...(one.kind === "spec" ? [words.version(one.version)] : []),
            this.options.pageWords.written(memory.writer(one.author), this.options.home.ago(now.getTime() - Date.parse(one.author.at))),
          ];
          return `<li><a class="page-link" href="${escape(memory.link(town, one.name))}">
          <span class="page-link-glyph">${glyph(icon, 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(one.name)}</span>
            <span class="page-link-about">${escape(one.description)}</span>
            <span class="page-link-facts">${facts.map(escape).join(`<span class="sep"></span>`)}</span>
          </span>
        </a></li>`;
        })
        .join("")}</ul>`;
    };
    const write = `<button class="text-button start-journal" type="button" data-write-journal>${glyph("plus", 16)}<span>${escape(words.newJournal)}</span></button>`;
    return [
      ["specs", words.specs, list(catalog.specs, "spec", words.noSpecs)],
      ["journals", words.journals, list(catalog.journals, "journal", words.noJournals) + write],
    ];
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
        ? `${r.last_action.machine !== r.about.machine ? `${r.last_action.machine} ` : ""}${r.last_action.what}`
        : "";
    const who = this.options.someone(r);
    const title = titleOf(r);
    const origin = title ? `<span class="someone-origin">${escape(who)}</span>` : "";
    const since = r.now?.since ?? r.about.born_at;
    return `<li><a class="someone${waiting ? " waiting" : ""}" href="${escape(this.options.link({ resident: r.about.id, machine: r.about.machine }))}">
      <span class="rail">${mark}</span>
      <span class="someone-body">
        <span class="someone-line"><span class="someone-name">${escape(title ?? who)}</span><span class="someone-since">${escape(busy || waiting ? home.for(home.lasted(now.getTime() - Date.parse(since))) : home.ago(now.getTime() - Date.parse(since)))}</span></span>
        ${origin}${doing ? `<span class="someone-doing">${escape(doing)}</span>` : ""}
      </span>
      <span class="someone-go">${glyph("chevron", 16)}</span>
    </a></li>`;
  }

  /** Something still running in the town: where, for how long, what, and
   *  who started it, said when they have gone. */
  private job(job: RunningJob, known: ResidentNow[], now: Date): string {
    const { words, home } = this.options;
    const address = ownerResident(job.owner);
    if (!address) {
      // A page started it, while the user had it open: no one is there to
      // leave.
      return this.jobLine(job, now, home.startedBy(home.aPage("page" in job.owner ? job.owner.page.page : "")));
    }
    const owner = known.find((r) => r.about.id === address.resident && r.about.machine === address.machine);
    const name = owner ? this.options.someone(owner) : `${address.resident} ${address.machine}`;
    const by = owner?.here && !owner.removed ? home.startedBy(name) : `${home.startedBy(name)} ${words.ownerLeft}`;
    return this.jobLine(job, now, by);
  }

  private jobLine(job: RunningJob, now: Date, by: string): string {
    const { home } = this.options;
    const whose = accountTag(worksFor(this.others, job.machine, job.job), home.forAccounts);
    return `<li class="job"><a class="job-id" href="${escape(fileLink(job.machine, job.output, true))}">${escape(job.job)}</a><span class="job-where">${escape(job.machine)} ${escape(home.for(home.lasted(now.getTime() - Date.parse(job.started_at))))}${whose}</span><code class="job-command">${escape(job.command)}</code><span class="detail">${escape(by)}</span></li>`;
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
