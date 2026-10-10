// Every conversation, or every one in a project: the list the sidebar opens
// when it has no room for them all. One line each, as in the sidebar, the
// most recent first; a search narrows it by name, agent or machine
// (`world-gui.md`, 界面的结构).

import { escape } from "../conversation/prose";
import { agentName, titleOf } from "../world/names";
import type { ResidentNow } from "../world/world";
import { redrawKeepingFocus } from "./focus";
import type { Place } from "./places";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";
import { conversationsOf, factsByMachine, talkRow } from "./talk-row";
import type { ShellWords } from "./words";

export interface TalksWords {
  title: string;
  search: string;
  outside: string;
  none: string;
}

export const zhTalks: TalksWords = { title: "全部对话", search: "搜索对话", outside: "不在项目里", none: "没有对话" };
export const enTalks: TalksWords = { title: "All conversations", search: "Search conversations", outside: "In no project", none: "No conversations" };

export interface TalksOptions {
  store: Store;
  words: TalksWords;
  shell: ShellWords;
  head(title: string, actions?: string): string;
  /** The project whose conversations are listed; all of them when null. */
  town: string | null;
  place(): Place;
}

export class TalksPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly list: HTMLElement;
  private readonly stop: () => void;
  private query = "";
  private drawn = "";

  constructor(private readonly options: TalksOptions) {
    const { words } = options;
    const project = options.town ? (options.store.world.peek()?.towns.find((t) => t.town === options.town)?.record.name ?? options.town) : null;
    this.title = project ?? words.title;
    this.element = document.createElement("div");
    this.element.className = "talks-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(this.title)}</h1>`)}
      <div class="talks-body scroller">
        <input class="talks-search" type="search" placeholder="${escape(words.search)}" aria-label="${escape(words.search)}" autocomplete="off" spellcheck="false">
        <div class="talks-list"></div>
      </div>`;
    this.list = this.element.querySelector<HTMLElement>(".talks-list")!;
    this.element.querySelector<HTMLInputElement>(".talks-search")!.addEventListener("input", (event) => {
      this.query = (event.target as HTMLInputElement).value.trim().toLowerCase();
      this.draw();
    });
    this.stop = effect(() => {
      options.store.world.get();
      options.store.clock.get();
      this.draw();
    });
  }

  leave(): void {
    this.stop();
  }

  private matches(r: ResidentNow): boolean {
    if (!this.query) return true;
    const said = [titleOf(r) ?? "", agentName(r.about.host.agent), r.about.machine, r.town ?? ""].join(" ").toLowerCase();
    return this.query.split(/\s+/).every((word) => said.includes(word));
  }

  private draw(): void {
    const { store, words, shell, town } = this.options;
    const world = store.world.peek();
    const now = store.clock.peek();
    const place = this.options.place();
    const facts = factsByMachine(world);
    const talks = conversationsOf(world).filter((r) => (town === null || r.town === town) && this.matches(r));
    let html: string;
    if (!talks.length) html = `<p class="talks-none">${escape(words.none)}</p>`;
    else if (town !== null) html = `<ul class="talks">${talks.map((r) => talkRow(r, place, now, facts, shell)).join("")}</ul>`;
    else {
      // A project's conversations together, the project most recently
      // stirred first; those in none last.
      const known = world?.towns ?? [];
      const groups = new Map<string, ResidentNow[]>();
      for (const r of talks) groups.set(r.town ?? "", [...(groups.get(r.town ?? "") ?? []), r]);
      const order = [...groups.keys()].sort((a, b) => (a === "" ? 1 : b === "" ? -1 : 0));
      html = order
        .map((key) => {
          const name = key ? (known.find((t) => t.town === key)?.record.name ?? key) : words.outside;
          return `<section class="talks-group"><h2 class="side-group-title">${escape(name)}</h2><ul class="talks">${groups
            .get(key)!
            .map((r) => talkRow(r, place, now, facts, shell))
            .join("")}</ul></section>`;
        })
        .join("");
    }
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }
}
