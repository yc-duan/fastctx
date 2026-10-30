// Every conversation, or every one in a project, or every one archived: the
// lists the sidebar opens when it has no room for them all, and the archived
// ones, which Settings' data page leads to. One line each, as in the
// sidebar, the most recent first, with the same acts on each row; the
// archived say when each was archived, and can all be deleted at once. A
// search narrows it by name, agent or machine (`world-gui.md`, 界面的结构).
// The head and the search hold still; thousands of rows are drawn as far as
// they are read, scrolling on their own.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { agentName, titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { redrawKeepingFocus } from "./focus";
import { link, type Place } from "./places";
import { readOn, whenNear } from "./read-on";
import type { Page } from "./shell";
import { effect } from "./signals";
import { assistedBy, isUnreached, type Store } from "./store";
import type { TalkActs } from "./talk-acts";
import { TalkList } from "./talk-list";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import { factsByMachine, talkRow, talksOf } from "./talk-row";
import type { Confirm } from "./confirm";
import type { ShellWords } from "./words";

export interface TalksWords {
  title: string;
  archived: string;
  search: string;
  outside: string;
  none: string;
  noneArchived: string;
  /** When one was archived: the time today, else the day. */
  archivedAt(at: Date, now: Date): string;
  deleteAll: string;
  deleteAllTitle(count: number): string;
  deleteAllWhat: string;
}

const clock = (at: Date) => `${String(at.getHours()).padStart(2, "0")}:${String(at.getMinutes()).padStart(2, "0")}`;
const sameDay = (a: Date, b: Date) => a.toDateString() === b.toDateString();

export const zhTalks: TalksWords = {
  title: "全部对话",
  archived: "已归档",
  search: "搜索对话",
  outside: "不在项目里",
  none: "没有对话",
  noneArchived: "没有归档的对话",
  archivedAt: (at, now) =>
    sameDay(at, now) ? clock(at) : `${at.getFullYear() === now.getFullYear() ? "" : `${at.getFullYear()}年`}${at.getMonth() + 1}月${at.getDate()}日`,
  deleteAll: "全部删除",
  deleteAllTitle: (count) => `删除全部 ${count} 段已归档的对话？`,
  deleteAllWhat: "所有机器与设备上的都会删掉，拿不回来。",
};
export const enTalks: TalksWords = {
  title: "All conversations",
  archived: "Archived",
  search: "Search conversations",
  outside: "In no project",
  none: "No conversations",
  noneArchived: "No archived conversations",
  archivedAt: (at, now) =>
    sameDay(at, now) ? clock(at) : at.toLocaleDateString("en", at.getFullYear() === now.getFullYear() ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" }),
  deleteAll: "Delete all",
  deleteAllTitle: (count) => `Delete all ${count} archived conversations?`,
  deleteAllWhat: "They go from every machine and device, for good.",
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
  /** Asks before deleting every archived conversation, then does. */
  confirm?: Confirm;
}

/** The archived, the one archived last first. */
function byArchived(archived: ResidentNow[]): ResidentNow[] {
  return archived
    .map((r) => ({ r, at: Date.parse(r.removed ?? "") || 0 }))
    .sort((a, b) => b.at - a.at)
    .map(({ r }) => r);
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
    const clear = options.archived && options.confirm ? `<button class="text-button danger-text talks-clear" type="button" hidden>${escape(words.deleteAll)}</button>` : "";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`, clear)}
      <div class="talks-find"><input class="talks-search" type="search" placeholder="${escape(words.search)}" aria-label="${escape(words.search)}" autocomplete="off" spellcheck="false"></div>
      <div class="talks-body scroller">
        <div class="talks-list"></div>
      </div>`;
    this.element.querySelector(".talks-clear")?.addEventListener("click", () => this.deleteAll());
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

  /** Deletes every archived conversation, once asked: each as its "⋯" would. */
  private deleteAll(): void {
    const { words, store, acts, confirm } = this.options;
    if (!confirm) return;
    const { archived } = talksOf(store.world.peek(), acts);
    const deletable = archived.filter((r) => !acts.deleting({ resident: r.about.id, machine: r.about.machine }));
    if (!deletable.length) return;
    confirm.ask({
      title: words.deleteAllTitle(deletable.length),
      what: words.deleteAllWhat,
      go: words.deleteAll,
      act: async () => {
        for (const r of deletable) acts.delete(r);
      },
      done: () => {},
    });
  }

  private matches(r: ResidentNow): boolean {
    if (!this.query) return true;
    const said = [titleOf(r) ?? "", agentName(r.about.host.agent), assistedBy(this.options.store.world.peek(), r) ?? r.about.machine, r.town ?? ""].join(" ").toLowerCase();
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
    const talks = (archivedOnly ? byArchived(archived) : listed).filter((r) => (town === null || r.town === town) && this.matches(r));
    this.talkList.drawing(talks);
    // An archived one says when it was archived, where another says where it lives.
    const row = (r: ResidentNow) =>
      talkRow(r, place, now, facts, shell, this.talkList.rowActs(r), assistedBy(this.options.store.world.peek(), r), archivedOnly && r.removed ? words.archivedAt(new Date(r.removed), now) : null);
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
    const clear = this.element.querySelector<HTMLElement>(".talks-clear");
    if (clear) clear.hidden = !archived.length;
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
