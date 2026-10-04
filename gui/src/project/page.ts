// A project's overview: what it is, who is in it now, its journals, its
// decisions and where its things are, as its map has them. Like the front
// page, every line is the World's record, nothing summarised.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import type { MapView, ResidentNow, WorldWho, Writer } from "../world/world";
import type { HomeWords } from "../home/words";
import type { ProjectWords } from "./words";

export interface ProjectOptions {
  words: ProjectWords;
  home: HomeWords;
  back: { href: string; label: string };
  link(address: Address): string;
  journalLink(town: string, journal: string): string;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class ProjectPage {
  readonly element: HTMLElement;
  private body: HTMLElement;
  private drawn = "";

  constructor(private readonly options: ProjectOptions) {
    this.element = document.createElement("div");
    this.element.className = "page project-page";
    this.element.innerHTML = `
      <header class="bar">
        <div class="bar-inner">
          <a class="bar-back" href="${escape(options.back.href)}" aria-label="${escape(options.back.label)}">${glyph("chevron", 20)}</a>
          <div class="who"><div class="who-state">${escape(options.words.overview)}</div></div>
        </div>
      </header>
      <main class="home overview"></main>`;
    this.body = this.element.querySelector<HTMLElement>(".overview")!;
  }

  update(map: MapView, who: WorldWho, now: Date): void {
    const { words } = this.options;
    const residents = who.machines.flatMap((machine) => (machine.kind === "answered" ? machine.reply.residents : []));
    const here = residents.filter((r) => r.here);
    const record = map.record;
    const html = `
      <header class="project-head">
        <h1 class="project-title">${escape(record.name)}</h1>
        <p class="project-lede">${escape(record.what)}</p>
        <p class="project-facts">${[
          record.machines.join(" · "),
          words.opened(this.writer(map.opened.by), map.opened.on, words.day(new Date(map.opened.at))),
        ]
          .map((fact) => `<span class="fact">${escape(fact)}</span>`)
          .join(`<span class="sep">·</span>`)}</p>
      </header>
      ${this.section(
        words.here,
        here.length
          ? `<ul class="people">${here.map((r) => this.someone(r, now)).join("")}</ul>`
          : `<p class="quiet">${escape(words.nobodyHere)}</p>`,
      )}
      ${this.section(
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
      )}
      ${this.section(words.decisions, this.canon(map))}
      ${record.places.length ? this.section(words.places, this.places(map)) : ""}`;
    if (html !== this.drawn) {
      this.drawn = html;
      this.body.innerHTML = html;
    }
    const state = this.element.querySelector<HTMLElement>(".who-state")!;
    state.textContent = words.overview;
    state.classList.remove("gone");
  }

  /** The project could not be asked: said in the bar, with what was last
   *  shown left as it was. */
  unreachable(why: string): void {
    const state = this.element.querySelector<HTMLElement>(".who-state")!;
    state.textContent = this.options.home.unreachable(why);
    state.classList.add("gone");
  }

  private section(title: string, body: string): string {
    return `<section class="home-section"><h2 class="section-title">${escape(title)}</h2>${body}</section>`;
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
    const since = r.now?.since ?? r.about.born_at;
    return `<li><a class="someone${waiting ? " waiting" : ""}" href="${escape(this.options.link({ resident: r.about.id, machine: r.about.machine }))}">
      <span class="rail">${mark}</span>
      <span class="someone-body">
        <span class="someone-line"><span class="someone-name">${escape(home.someone(agentName(r.about.host.agent), r.about.machine))}</span><span class="someone-since">${escape(busy || waiting ? home.for(home.lasted(now.getTime() - Date.parse(since))) : home.ago(now.getTime() - Date.parse(since)))}</span></span>
        ${doing ? `<span class="someone-doing">${escape(doing)}</span>` : ""}
      </span>
      <span class="someone-go">${glyph("chevron", 16)}</span>
    </a></li>`;
  }

  private canon(map: MapView): string {
    const { words } = this.options;
    if (!map.canon) return `<p class="quiet">${escape(words.noDecisions)}</p>`;
    if ("Err" in map.canon && map.canon.Err !== undefined) {
      return `<p class="quiet caution">${escape(words.decisionsUnreadable(map.canon.Err))}</p>`;
    }
    const heads = map.canon.Ok ?? [];
    const where = map.record.canon ? `${map.record.canon.machine} · ${map.record.canon.path}` : "";
    return `<ul class="canon">${heads
      .map(
        (head) =>
          `<li class="canon-doc"><span class="canon-title">${escape(head.title)}</span><span class="canon-scope">${escape(head.scope)}</span></li>`,
      )
      .join("")}</ul>${where ? `<p class="canon-where">${escape(where)}</p>` : ""}`;
  }

  private places(map: MapView): string {
    const { words } = this.options;
    return `<div class="prose"><table><thead><tr><th>${escape(words.what)}</th><th>${escape(words.machine)}</th><th>${escape(words.path)}</th></tr></thead><tbody>${map.record.places
      .map(
        (place) =>
          `<tr><td>${escape(place.what)}</td><td>${escape(place.machine)}</td><td><code>${escape(place.path)}</code></td></tr>`,
      )
      .join("")}</tbody></table></div>`;
  }
}
