// A project's overview, on one screen: what waits for the user, the work
// in it drawn as each conversation's way with the machines it stands on
// (`hub-view.ts`), the pages its residents wrote and where its things are,
// as its record has them; where project memory is on, its decisions and
// journals beside its record (`world-memory.md`). Its "⋯" shares it and
// hands it on, where the reader may (`../app/project-acts.ts`).
// Side by side where there is room, each column scrolling on its own;
// behind tabs on a narrow screen. Like the front page, every line is the
// World's record, nothing summarised.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { factRow } from "../design/fact-row";
import { redrawKeepingFocus } from "../app/focus";
import type { Address } from "../world/events";
import type { MapView, ProjectRecord, ResidentNow, WorldWho, Writer } from "../world/world";
import type { PageSummary, PageWriter } from "../world/pages";
import type { PageWords } from "../pages/words";
import type { HomeWords } from "../home/words";
import type { ProjectWords } from "./words";
import { agentName } from "../world/names";
import type { MemoryAuthor, MemoryCatalog, MemoryRevision } from "../world/memory";
import type { MemoryWords } from "./memory-words";
import type { SentinelPhase, SentinelSummary } from "../world/sentinels";
import { hubOf } from "./hub-model";
import { hubPicture, type HubViewOptions } from "./hub-view";
import "./hub.css";

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
  /** A machine's own page. */
  machineLink(machine: string): string;
  /** A watcher's own page. */
  senseLink(sense: SentinelSummary): string;
  /** How a sense stands, in a word. */
  phase(phase: SentinelPhase): string;
  /** The last thing a conversation (`resident@machine`) told that this
   *  device has shown the user. */
  seenTold(key: string): string | null;
}

type Tab = "instructions" | "specs" | "journals" | "pages" | "places";
/** The page's two views: the work now, drawn; and what the project keeps. */
type View = "now" | "reference";

/** The view a project was last read in on this device: the work now,
 *  unless its reference was asked for. */
function storedView(town: string): View {
  try {
    return localStorage.getItem(`project-view:${town}`) === "reference" ? "reference" : "now";
  } catch {
    return "now";
  }
}

function keepView(town: string, view: View): void {
  try {
    if (view === "reference") localStorage.setItem(`project-view:${town}`, view);
    else localStorage.removeItem(`project-view:${town}`);
  } catch {
    // Not kept: the project opens on its work now next time.
  }
}

export class ProjectPage {
  readonly element: HTMLElement;
  private title: HTMLElement;
  private intro: HTMLElement;
  private tabs: HTMLElement;
  private board: HTMLElement;
  private view: View;
  private record: ProjectRecord | null = null;
  private drawn = "";
  /** Its memory as last read; none while the feature is off or unread. */
  private catalog: MemoryCatalog | null = null;
  private last: { map: MapView; who: WorldWho; now: Date } | null = null;
  /** Each conversation's plan as last read, by `resident@machine`. */
  private plans: ReadonlyMap<string, string | null> = new Map();
  /** The senses of the accounts here, as last listed. */
  private senses: SentinelSummary[] = [];

  constructor(private readonly options: ProjectOptions) {
    this.element = document.createElement("div");
    this.element.className = "project-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title"></h1>`,
        `<button class="text-button revise" type="button" disabled>${glyph("edit", 16)}<span>${escape(options.edit)}</span></button><button class="text-button begin-here" type="button">${glyph("plus", 16)}<span>${escape(options.newTalk)}</span></button>${options.more ? `<button class="icon-button project-more" type="button" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(options.more.label)}" title="${escape(options.more.label)}" hidden>${glyph("more", 18)}</button>` : ""}`,
      )}
      <header class="project-intro"></header>
      <div class="tabs" role="tablist"><button class="tab" type="button" role="tab" data-view="now">${escape(options.words.nowTab)}</button><button class="tab" type="button" role="tab" data-view="reference">${escape(options.words.referenceTab)}</button></div>
      <div class="project-board"></div>`;
    this.title = this.element.querySelector<HTMLElement>(".page-title")!;
    this.title.textContent = options.town;
    this.intro = this.element.querySelector<HTMLElement>(".project-intro")!;
    this.tabs = this.element.querySelector<HTMLElement>(".tabs")!;
    this.board = this.element.querySelector<HTMLElement>(".project-board")!;
    this.view = storedView(options.town);
    this.element.querySelector(".begin-here")!.addEventListener("click", () => options.begin(options.town));
    this.element.querySelector(".project-more")?.addEventListener("click", (event) => {
      options.more?.open(options.town, this.record?.name ?? options.town, event.currentTarget as HTMLElement);
    });
    this.element.querySelector(".revise")!.addEventListener("click", () => {
      if (this.record) options.revise(options.town, this.record);
    });
    this.element.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-write-journal]")) options.memory?.write(options.town);
      // A watcher's mark, in its conversation's row, opens its own page.
      const sense = (event.target as HTMLElement).closest<HTMLElement>("[data-sense]");
      if (sense) {
        event.preventDefault();
        location.hash = sense.dataset.sense!;
      }
    });
    this.tabs.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-view]");
      if (!button) return;
      this.view = button.dataset.view as View;
      keepView(options.town, this.view);
      this.choose();
    });
    // A conversation pointed at lights the machines its work stands on.
    const light = (event: Event, on: boolean) => {
      const row = (event.target as HTMLElement).closest<HTMLElement>(".ph-row");
      if (!row) return;
      const machines = (row.dataset.machines ?? "").split(" ");
      for (const place of this.board.querySelectorAll<HTMLElement>(".ph-place")) place.classList.toggle("lit", on && machines.includes(place.dataset.machine ?? ""));
    };
    this.board.addEventListener("pointerover", (event) => light(event, true));
    this.board.addEventListener("pointerout", (event) => light(event, false));
    this.board.addEventListener("focusin", (event) => light(event, true));
    this.board.addEventListener("focusout", (event) => light(event, false));
    this.choose();
  }

  /** The project's memory, read again; null where it is off. */
  showMemory(catalog: MemoryCatalog | null): void {
    this.catalog = catalog;
    if (this.last) this.update(this.last.map, this.last.who, this.last.now);
  }

  /** The plans of its conversations, read again. */
  showPlans(plans: ReadonlyMap<string, string | null>): void {
    this.plans = plans;
    if (this.last) this.update(this.last.map, this.last.who, this.last.now);
  }

  /** The senses its conversations keep, listed again. */
  showSenses(senses: SentinelSummary[]): void {
    this.senses = senses;
    if (this.last) this.update(this.last.map, this.last.who, this.last.now);
  }

  update(map: MapView, who: WorldWho, now: Date): void {
    this.last = { map, who, now };
    const { words } = this.options;
    // Every conversation of the project, while it exists (walkthrough items 20, 22).
    const residents = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : [])).filter((r) => !r.removed);
    const jobs = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.jobs : []));
    const record = map.record;
    this.record = record;
    this.element.querySelector<HTMLButtonElement>(".revise")!.disabled = false;
    const more = this.element.querySelector<HTMLButtonElement>(".project-more");
    if (more) more.hidden = !this.options.more?.shown(map.town);
    this.title.textContent = record.name;
    const sharedBy = this.options.more?.sharedBy(map.town) ?? null;
    const facts = `<p class="project-facts">${factRow(
      [...record.machines, words.opened(this.writer(map.opened.by), map.opened.on, words.day(new Date(map.opened.at))), ...(sharedBy ? [sharedBy] : [])].map(escape),
    )}</p>`;
    const hub = hubOf(residents, jobs, record.machines, this.plans, this.senses, (key) => this.options.seenTold(key));
    const sections: [Tab, string, string][] = [
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
    // What the project keeps: what it is and what was decided beside, what
    // was written along the way and where its things are on the other side.
    const first: Tab[] = ["instructions", "specs"];
    const reference = `<div class="ph-reference">${facts}<div class="board-column">${sections
      .filter(([tab]) => first.includes(tab))
      .map(section)
      .join("")}</div><div class="board-column">${sections
      .filter(([tab]) => !first.includes(tab))
      .map(section)
      .join("")}</div></div>`;
    const board = `<div class="ph-now">${hubPicture(hub, map.town, this.hubView(now))}</div>${reference}`;
    if (board !== this.drawn) {
      this.drawn = board;
      redrawKeepingFocus(this.element, () => {
        this.intro.innerHTML = "";
        this.board.innerHTML = board;
        this.choose();
      });
    }
  }

  private hubView(now: Date): HubViewOptions {
    const { words, home, someone, link, machineLink, senseLink, phase } = this.options;
    return { words, home, someone, link, machineLink, senseLink, phase, now };
  }

  /** The project could not be asked: said under its name, with what was
   *  last shown left as it was. */
  unreachable(why: string): void {
    this.intro.innerHTML = `<p class="project-facts unreachable">${escape(this.options.home.unreachable(why))}</p>`;
    this.drawn = "";
  }

  private choose(): void {
    this.board.dataset.view = this.view;
    for (const button of this.tabs.querySelectorAll<HTMLElement>("[data-view]")) {
      const chosen = button.dataset.view === this.view;
      button.classList.toggle("chosen", chosen);
      button.setAttribute("aria-selected", String(chosen));
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
        return `<li><a class="page-link${current.withdrawn ? " withdrawn" : ""}" href="${escape(this.options.pageLink(town, page))}" title="${escape(facts.join("　"))}">
          <span class="page-link-glyph">${glyph("page", 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(current.title || page)}${current.withdrawn ? `<span class="tag">${escape(words.withdrawnMark)}</span>` : ""}</span>
            ${current.description ? `<span class="page-link-about">${escape(current.description)}</span>` : ""}
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
          return `<li><a class="page-link" href="${escape(memory.link(town, one.name))}" title="${escape(facts.join("　"))}">
          <span class="page-link-glyph">${glyph(icon, 18)}</span>
          <span class="page-link-body">
            <span class="page-link-title">${escape(one.name)}</span>
            <span class="page-link-about">${escape(one.description)}</span>
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

  private places(map: MapView): string {
    return `<ul class="places">${map.record.places
      .map(
        (place) =>
          `<li class="place-line"><span class="place-what">${escape(place.what)}</span><span class="place-where"><span class="place-machine">${escape(place.machine)}</span><code>${escape(place.path)}</code></span></li>`,
      )
      .join("")}</ul>`;
  }
}
