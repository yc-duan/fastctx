// Every conversation, or every one in a project, or every one archived: the
// lists the sidebar opens when it has no room for them all. One line each, as
// in the sidebar, the most recent first, with the same acts on each row; a
// search narrows it by name, agent or machine (`world-gui.md`, 界面的结构).
// Thousands of them are drawn as far as they are read.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { agentName, titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { redrawKeepingFocus } from "./focus";
import { link, type Place } from "./places";
import { assistingOwner } from "./reach";
import { readOn, whenNear } from "./read-on";
import type { Page } from "./shell";
import { effect } from "./signals";
import { isUnreached, type Store } from "./store";
import type { TalkActs } from "./talk-acts";
import { TalkList } from "./talk-list";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import { factsByMachine, talkRow, talksOf } from "./talk-row";
import type { ShellWords } from "./words";

export interface TalksWords {
  title: string;
  archived: string;
  search: string;
  outside: string;
  none: string;
  noneArchived: string;
}

export const zhTalks: TalksWords = {
  title: "全部对话",
  archived: "已归档",
  search: "搜索对话",
  outside: "不在项目里",
  none: "没有对话",
  noneArchived: "没有归档的对话",
};
export const enTalks: TalksWords = {
  title: "All conversations",
  archived: "Archived",
  search: "Search conversations",
  outside: "In no project",
  none: "No conversations",
  noneArchived: "No archived conversations",
};

export interface TalksOptions {
  store: Store;
  words: TalksWords;
  shell: ShellWords;
  head(title: string, actions?: string): string;
  /** The project whose conversations are listed; all of them when null. */
  town: string | null;
  /** The archived ones are listed instead. */
  archived: boolean;
  place(): Place;
  acts: TalkActs;
  menu: TalkMenu;
  menuWords: TalkMenuWords;
}

/** How many rows are drawn at first, and how many more each time the end of
 *  those comes near: a list of thousands is drawn as far as it is read. */
const ROWS = 120;

export class TalksPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly stop: () => void;
  /** The rows' menus, presses and renaming. */
  private readonly talkList: TalkList;
  private query = "";
  private drawn = "";
  /** How many rows are drawn now, and what stops watching the end of those
   *  coming near. */
  private reach = ROWS;
  private unwatch = () => {};

  constructor(private readonly options: TalksOptions) {
    const { words } = options;
    const project = options.town ? (options.store.world.peek()?.towns.find((t) => t.town === options.town)?.record.name ?? options.town) : null;
    this.title = options.archived ? words.archived : (project ?? words.title);
    this.element = document.createElement("div");
    this.element.className = "talks-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}
      <div class="talks-body scroller">
        <input class="talks-search" type="search" placeholder="${escape(words.search)}" aria-label="${escape(words.search)}" autocomplete="off" spellcheck="false">
        <div class="talks-list"></div>
      </div>`;
    this.list = this.element.querySelector<HTMLElement>(".talks-list")!;
    this.talkList = new TalkList(this.list, {
      acts: options.acts,
      menu: options.menu,
      menuWords: options.menuWords,
      reachable: (r) => !isUnreached(r),
      redraw: () => {
        this.drawn = "";
        this.draw();
      },
    });
    this.element.querySelector<HTMLInputElement>(".talks-search")!.addEventListener("input", (event) => {
      this.query = (event.target as HTMLInputElement).value.trim().toLowerCase();
      this.reach = ROWS;
      this.draw();
    });
    this.stop = effect(() => {
      options.store.world.get();
      options.store.clock.get();
      options.acts.changed.get();
      this.draw();
    });
  }

  leave(): void {
    this.stop();
    this.unwatch();
  }

  private matches(r: ResidentNow): boolean {
    if (!this.query) return true;
    const said = [titleOf(r) ?? "", agentName(r.about.host.agent), assistingOwner(this.options.store.world.peek(), r.about.machine) ?? r.about.machine, r.town ?? ""].join(" ").toLowerCase();
    return this.query.split(/\s+/).every((word) => said.includes(word));
  }

  private draw(): void {
    // A name being typed is kept until it is done.
    if (this.talkList.held) return;
    const { store, words, shell, town, archived: archivedOnly, acts } = this.options;
    const world = store.world.peek();
    const now = store.clock.peek();
    const place = this.options.place();
    const facts = factsByMachine(world);
    const { listed, archived } = talksOf(world, acts);
    const talks = (archivedOnly ? archived : listed).filter((r) => (town === null || r.town === town) && this.matches(r));
    this.talkList.drawing(talks);
    const row = (r: ResidentNow) => talkRow(r, place, now, facts, shell, this.talkList.rowActs(r), assistingOwner(this.options.store.world.peek(), r.about.machine));
    let html = "";
    if (!talks.length) html = `<p class="talks-none">${escape(archivedOnly ? words.noneArchived : words.none)}</p>`;
    else {
      // A project's conversations together, the project most recently
      // stirred first; those in none last. One project's are one list.
      const sections: { name: string | null; rows: ResidentNow[] }[] = [];
      if (town !== null) sections.push({ name: null, rows: talks });
      else {
        const known = world?.towns ?? [];
        const groups = new Map<string, ResidentNow[]>();
        for (const r of talks) {
          const group = groups.get(r.town ?? "");
          if (group) group.push(r);
          else groups.set(r.town ?? "", [r]);
        }
        for (const key of [...groups.keys()].sort((a, b) => (a === "" ? 1 : b === "" ? -1 : 0))) {
          sections.push({ name: key ? (known.find((t) => t.town === key)?.record.name ?? key) : words.outside, rows: groups.get(key)! });
        }
      }
      // Drawn as far as it is read.
      let left = this.reach;
      for (const section of sections) {
        if (left <= 0) break;
        const rows = section.rows.slice(0, left);
        left -= rows.length;
        const list = `<ul class="talks${archivedOnly ? " archived-talks" : ""}">${rows.map(row).join("")}</ul>`;
        html += section.name === null ? list : `<section class="talks-group"><h2 class="side-group-title">${escape(section.name)}</h2>${list}</section>`;
      }
    }
    const more = talks.length > this.reach;
    if (more) html += readOn;
    // Every conversation leads on to the archived ones, where there are any.
    const shelved = archivedOnly || town !== null ? 0 : archived.length;
    if (shelved && !more)
      html += `<a class="talks-archived" href="${link({ kind: "archived" })}">${glyph("archive", 16)}<span>${escape(words.archived)}</span><span class="count">${shelved}</span>${glyph("chevron", 14)}</a>`;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
    this.unwatch();
    this.unwatch = whenNear(this.list, this.list.parentElement, () => {
      this.reach += ROWS;
      this.draw();
    });
  }
}
