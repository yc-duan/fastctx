// Every project of the user's accounts (`world-gui.md`, 界面的结构): the place
// under 「机器」 in the sidebar where projects are managed as a whole, the
// sidebar's folders being where their conversations are. One row each, its
// name, the first line of its instructions, and how many conversations are
// in it, in clay while one of them waits on the user; a row leads to the
// project's overview, 「新建」 opens a new one. Logged in to several
// accounts, each account's are under its name. The head and the search hold
// still; however many there are, only the rows scroll.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { TownSummary } from "../world/world";
import { link } from "./places";
import { groupsOf, loginLabel } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import type { TalkActs } from "./talk-acts";
import { talksOf } from "./talk-row";

export interface ProjectsWords {
  title: string;
  add: string;
  search: string;
  none: string;
  noneFound: string;
  /** How many conversations are in it, and how many wait on the user. */
  talks(count: number, waiting: number): string;
}

export const zhProjects: ProjectsWords = {
  title: "项目",
  add: "新建",
  search: "搜索项目",
  none: "还没有项目",
  noneFound: "没有找到",
  talks: (count, waiting) => `${count} 段对话${waiting ? `，${waiting} 段在等你` : ""}`,
};

export const enProjects: ProjectsWords = {
  title: "Projects",
  add: "New",
  search: "Search projects",
  none: "No projects yet",
  noneFound: "Nothing found",
  talks: (count, waiting) => `${count} conversation${count === 1 ? "" : "s"}${waiting ? `, ${waiting} waiting for you` : ""}`,
};

export interface ProjectsPageOptions {
  store: Store;
  words: ProjectsWords;
  head(title: string, actions?: string): string;
  acts: TalkActs;
  /** Opens a new project, in `login`'s account where one is named. */
  newProject(login: Login | null): void;
}

export class ProjectsPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly stop: () => void;
  private query = "";
  private drawn = "";

  constructor(private readonly options: ProjectsPageOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "pjl-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}
      <div class="pjl-find"><input class="talks-search pjl-search" type="search" placeholder="${escape(words.search)}" aria-label="${escape(words.search)}" autocomplete="off" spellcheck="false"></div>
      <div class="pjl-body scroller"><div class="pjl-list"></div></div>`;
    this.list = this.element.querySelector<HTMLElement>(".pjl-list")!;
    this.element.querySelector<HTMLInputElement>(".pjl-search")!.addEventListener("input", (event) => {
      this.query = (event.target as HTMLInputElement).value.trim().toLowerCase();
      this.draw(options.store.world.peek());
    });
    this.list.addEventListener("click", (event) => {
      const add = (event.target as HTMLElement).closest<HTMLElement>("[data-pjl-add]");
      if (!add) return;
      const key = add.dataset.pjlAdd ?? "";
      const login = groupsOf(options.store.world.peek()).find((group) => `${group.login.hub}/${group.login.account}` === key)?.login ?? null;
      options.newProject(login);
    });
    this.stop = effect(() => {
      const world = options.store.world.get();
      options.acts.changed.get();
      this.draw(world);
    });
  }

  leave(): void {
    this.stop();
  }

  private matches(town: TownSummary): boolean {
    if (!this.query) return true;
    const said = `${town.record.name} ${town.town} ${town.record.what}`.toLowerCase();
    return this.query.split(/\s+/).every((word) => said.includes(word));
  }

  private draw(world: WorldNow | null): void {
    const { words, acts } = this.options;
    const towns = world?.towns ?? [];
    const { listed } = talksOf(world, acts);
    const groups = groupsOf(world);
    // Each account's own, under its name where there are several; one rule
    // and its 「新建」 where there is one.
    const sections = groups.length > 1 ? groups.map((group) => ({ login: group.login, towns: towns.filter((town) => group.towns.includes(town.town)) })) : [{ login: groups[0]?.login ?? null, towns }];
    const anything = towns.length > 0;
    let html = "";
    for (const section of sections) {
      const shown = section.towns.filter((town) => this.matches(town)).sort((a, b) => a.record.name.localeCompare(b.record.name));
      const key = section.login ? `${section.login.hub}/${section.login.account}` : "";
      const add = `<button class="text-button quiet-button pjl-add" type="button" data-pjl-add="${escape(key)}">${glyph("plus", 15)}<span>${escape(words.add)}</span></button>`;
      const title = groups.length > 1 && section.login ? `<span>${escape(loginLabel(section.login))}</span>` : `<span class="pjl-said">${escape(words.title)}</span>`;
      const rows = shown.map((town) => this.row(town, listed.filter((r) => r.town === town.town))).join("");
      const empty = !anything ? `<div class="pjl-none">${glyph("folder", 40)}<p class="prof-quiet">${escape(words.none)}</p></div>` : !shown.length ? `<p class="prof-quiet pjl-quiet">${escape(words.noneFound)}</p>` : "";
      html += `<section class="home-section pjl-section"><h2 class="section-title pjl-title">${title}${add}</h2>${rows ? `<ul class="pjl-rows">${rows}</ul>` : empty}</section>`;
    }
    if (html === this.drawn) return;
    this.drawn = html;
    this.list.innerHTML = html;
  }

  /** A project: its name, the first line of what it is for, and its
   *  conversations, counted. */
  private row(town: TownSummary, talks: { now?: { waiting_for_user?: boolean } | null }[]): string {
    const { words } = this.options;
    const waiting = talks.filter((r) => r.now?.waiting_for_user).length;
    const what = town.record.what.split("\n").find((line) => line.trim())?.trim() ?? "";
    const count = talks.length ? `<span class="count${waiting ? " waiting" : ""}" title="${escape(words.talks(talks.length, waiting))}">${talks.length}</span>` : "";
    return `<li><a class="pjl-row" href="${link({ kind: "project", town: town.town })}">
        <span class="pjl-mark">${glyph("folder", 18)}</span>
        <span class="pjl-name">${escape(town.record.name)}</span>
        <span class="pjl-what" title="${escape(what)}">${escape(what)}</span>
        ${count}
      </a></li>`;
  }
}
